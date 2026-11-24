//! Remove GPU MAX: FLUX.2 Klein 4B (Black Forest Labs, Apache-2.0) with fal's object-remove LoRA
//! (Apache-2.0) fused in, as ONNX (transformer 8-bit MatMulNBits, VAE fp32), run by ONNX Runtime in
//! this process on the graphics card (WebGPU on Linux and Apple Silicon (Metal), DirectML on Windows:
//! AMD, Intel and NVIDIA alike), as RORem is (`rorem.rs`, whose session setup this shares).
//!
//! The selection is filled flat grey and Klein is asked, with one fixed instruction whose text-encoder
//! output ships as `remove_prompt.f32` (no text encoder at run time), to remove the grey shape. It
//! paints the whole picture; the caller blends it in (the selection plus what it changed next to it).
//! Built with tools in ~/Work/flux3 (export_klein_transformer.py, export_klein_vae.py, klein_stage1.py).
//!
//! Sampling, as diffusers' Flux2KleinPipeline: 4 Euler steps of flow matching; sigmas 1, 0.75, 0.5,
//! 0.25 shifted by exp(mu) / (exp(mu) + 1/sigma - 1), mu from the number of picture tokens
//! (`empirical_mu`). Tokens are the 16x16-pixel latent cells (128 channels), the picture's first and
//! then the reference photo's, with position ids (t, h, w, l): t = 0 for the picture, 10 for the photo.
use image::RgbImage;
use std::path::PathBuf;
use std::sync::Mutex;

pub const STEPS: usize = 4;
/// Long edge GPU works at (1408: 12.5 s on an RTX 5070 Ti, 21 Nov), within MAX_CELLS.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub const SIDE: u32 = 1408;
/// On a Mac 1024 (24 Nov, M1 Max, fp16 weights: 768x496 in 27 s but soft, 1024x672 in 54 s; 1408
/// would be well over a minute; newer Macs are quicker). Chris chose 1024.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const SIDE: u32 = 1024;
/// At most this many 16 px latent cells (~1.3 MP): memory grows with the area. A big area (a couple
/// filling the frame) at 1408x1200 peaked at 15.4 of 16 GB; at 1216x1040 (4940 cells) 11.5 (21 Nov).
/// Scaled with SIDE (the same shape limit on a Mac).
pub const MAX_CELLS: f32 = 5000.0 * (SIDE as f32 / 1408.0) * (SIDE as f32 / 1408.0);

/// The working size for a crop cw x ch: long edge SIDE, area within MAX_CELLS, sides multiples of 16.
pub fn work_size(cw: f32, ch: f32) -> (u32, u32) {
    let mut s = SIDE as f32 / cw.max(ch);
    let cells = (cw * s / 16.0) * (ch * s / 16.0);
    if cells > MAX_CELLS {
        s *= (MAX_CELLS / cells).sqrt();
    }
    let r = |v: f32| (((v * s) / 16.0).round().max(1.0) as u32) * 16;
    (r(cw), r(ch))
}
const TXT_LEN: usize = 512;
const TXT_DIM: usize = 7680;
const CH: usize = 128;

struct Sessions {
    tr: ort::session::Session,
    enc: ort::session::Session,
    dec: ort::session::Session,
    prompt: Vec<f32>,
    gpu: bool,
}

static LOADED: Mutex<Option<Sessions>> = Mutex::new(None);

/// The folder holding GPU MAX's files, if they're all there.
pub fn folder() -> Option<PathBuf> {
    crate::mask::model_dirs().into_iter().map(|d| d.join(crate::remover_download::gpumax_model().folder)).find(|d| {
        ["transformer.onnx", "transformer.onnx.data", "vae_encoder.onnx", "vae_decoder.onnx", "remove_prompt.f32"].iter().all(|f| d.join(f).is_file())
    })
}

pub fn available() -> bool {
    folder().is_some()
}

/// Windows: the transformer's graph with every Reshape's `allowzero` set to 0. DirectML fails on
/// `allowzero = 1` ("Non-zero status code returned while running Reshape node 'node_Reshape_4814'",
/// Master-Control, RTX 5070 Ti, 24 Nov), which the exporter set on 231 Reshapes. None of their
/// shapes holds a 0, so 0 means the same here. Written once beside the downloaded files as
/// `transformer.dml.onnx` (4 MB; its weights stay in `transformer.onnx.data`, which it names
/// relatively), so the download and its SHA-256s are unchanged on every platform.
#[cfg(windows)]
fn dml_graph(dir: &std::path::Path) -> Result<PathBuf, String> {
    let src = dir.join("transformer.onnx");
    let out = dir.join("transformer.dml.onnx");
    let mut bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let n = clear_allowzero(&mut bytes);
    if n == 0 {
        return Ok(src);
    }
    if std::fs::read(&out).ok().as_deref() != Some(&bytes[..]) {
        let tmp = dir.join("transformer.dml.onnx.part");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &out).map_err(|e| format!("{}: {e}", out.display()))?;
        eprintln!("gpumax: allowzero cleared on {n} Reshapes for DirectML ({})", out.display());
    }
    Ok(out)
}

/// Sets every `allowzero: 1` attribute in a serialized ONNX graph to 0, in place (same length:
/// the attribute's name, field 1, then its `i`, field 3 varint 1 → 0). Returns how many it changed.
#[cfg_attr(not(windows), allow(dead_code))]
fn clear_allowzero(bytes: &mut [u8]) -> usize {
    const PAT: &[u8] = b"\x0a\x09allowzero\x18\x01";
    let mut n = 0;
    let mut i = 0;
    while i + PAT.len() <= bytes.len() {
        if &bytes[i..i + PAT.len()] == PAT {
            bytes[i + PAT.len() - 1] = 0;
            n += 1;
            i += PAT.len();
        } else {
            i += 1;
        }
    }
    n
}

fn load(slot: &mut Option<Sessions>) -> Result<&mut Sessions, String> {
    if slot.is_none() {
        let dir = folder().ok_or("Remove GPU MAX isn't installed")?;
        let env = crate::mask::ort_env().ok_or("ONNX Runtime didn't start")?;
        let cpu = std::env::var("RAPIDONE_REMOVE_CPU").is_ok();
        // RAPIDONE_GPUMAX_VAE=cpu: the VAE on the CPU (testing).
        let vae_cpu = cpu || std::env::var("RAPIDONE_GPUMAX_VAE").as_deref() == Ok("cpu");
        let t0 = std::time::Instant::now();
        // WebGPU buffers "bucket": reused within a fill (lowest peak). "lazy" freed them on unload but
        // doesn't reuse within a fill: a 880x1408 head crop ran out of 16 GB (21 Nov). Unloading frees
        // the card because `unload` also drops Denoise's session, ending ONNX Runtime's context.
        let cache = Some(("bucket", "bucket"));
        #[cfg(windows)]
        let transformer = dml_graph(&dir)?;
        #[cfg(not(windows))]
        let transformer = dir.join("transformer.onnx");
        let (tr, gpu) = crate::rorem::build_cache(env, &transformer, cpu, cache)?;
        let t_tr = t0.elapsed().as_secs_f32();
        let (enc, _) = crate::rorem::build_cache(env, &dir.join("vae_encoder.onnx"), vae_cpu || !gpu, cache)?;
        let (dec, _) = crate::rorem::build_cache(env, &dir.join("vae_decoder.onnx"), vae_cpu || !gpu, cache)?;
        let bytes = std::fs::read(dir.join("remove_prompt.f32")).map_err(|e| e.to_string())?;
        let prompt: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        if prompt.len() != TXT_LEN * TXT_DIM {
            return Err("GPU MAX's remove_prompt.f32 is the wrong size".into());
        }
        eprintln!("gpumax: loaded in {:.1} s (transformer {t_tr:.1} s) on the {}", t0.elapsed().as_secs_f32(), if gpu { "graphics card" } else { "CPU" });
        *slot = Some(Sessions { tr, enc, dec, prompt, gpu });
    }
    Ok(slot.as_mut().unwrap())
}

/// Frees GPU (Smarter)'s graphics memory until it's next used (measured 21 Nov: back to idle at once).
pub fn unload() {
    let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        *g = None;
        // ONNX Runtime keeps a WebGPU context (and its buffer cache) while any session uses it:
        // Denoise's session goes too, so the card is actually freed (it reloads when next needed).
        crate::denoise::release();
        eprintln!("gpumax: unloaded");
    }
}

/// Whether GPU (Smarter) is on the graphics card now (the Remove bar then offers "GPU OFF").
pub fn loaded() -> bool {
    LOADED.try_lock().map(|g| g.is_some()).unwrap_or(true)
}

/// Unloads after IDLE_AFTER without a removal (as RORem did): ~9 GB of graphics memory shouldn't
/// stay taken after the work is done. Each fill restarts the clock.
const IDLE_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn unload_later() {
    let g = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(IDLE_AFTER);
        if GENERATION.load(std::sync::atomic::Ordering::SeqCst) == g {
            eprintln!("gpumax: unused for {} min", IDLE_AFTER.as_secs() / 60);
            unload();
        }
    });
}

/// diffusers' `compute_empirical_mu` (Flux2KleinPipeline), for the schedule's shift.
fn empirical_mu(seq: usize, steps: usize) -> f64 {
    let (a1, b1) = (8.73809524e-05, 1.89833333);
    let (a2, b2) = (0.00016927, 0.45666666);
    let s = seq as f64;
    if seq > 4300 {
        return a2 * s + b2;
    }
    let m200 = a2 * s + b2;
    let m10 = a1 * s + b1;
    let a = (m200 - m10) / 190.0;
    let b = m200 - 200.0 * a;
    a * steps as f64 + b
}

/// The shifted sigmas, one per step, then 0.
fn schedule(seq: usize, steps: usize) -> Vec<f32> {
    let mu = empirical_mu(seq, steps).exp();
    let mut s: Vec<f32> = (0..steps)
        .map(|i| {
            let sig = 1.0 - i as f64 * (1.0 - 1.0 / steps as f64) / (steps - 1).max(1) as f64;
            (mu / (mu + (1.0 / sig - 1.0))) as f32
        })
        .collect();
    s.push(0.0);
    s
}

/// Timings of the last fill, seconds: encode, each step, decode.
pub static LAST_TIMES: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());

/// Klein's picture of `picture` (sides multiples of 16) with the `hole` (true = remove; one per pixel)
/// filled grey and removed. The whole picture as Klein paints it; the caller blends it in.
pub fn fill(picture: &RgbImage, hole: &[bool], seed: u32) -> Result<RgbImage, String> {
    let (w, h) = picture.dimensions();
    if w % 16 != 0 || h % 16 != 0 {
        return Err("GPU MAX: the picture's sides must be multiples of 16".into());
    }
    if hole.len() != (w * h) as usize {
        return Err("GPU MAX: the hole isn't the picture's size".into());
    }
    let (wu, hu) = (w as usize, h as usize);
    let plane = wu * hu;
    // The picture, the selection flat grey: Klein's reference. Shown at half size (Klein paints at
    // full size): half the work at every step is the reference's tokens, a quarter as many at half
    // size; same look (couple and moor, 21 Nov), 1.5-1.6x faster. Klein takes a reference of any
    // size (its own position grid). The KV cache that diffusers offers needs a model trained for it
    // (FLUX.2-klein-9b-kv): with 4B it ignored the photo. RAPIDONE_GPUMAX_REF=1: full size (testing).
    let greyed = RgbImage::from_fn(w, h, |xx, yy| {
        let i = yy as usize * wu + xx as usize;
        if hole[i] { image::Rgb([128, 128, 128]) } else { *picture.get_pixel(xx, yy) }
    });
    let full_ref = std::env::var("RAPIDONE_GPUMAX_REF").as_deref() == Ok("1");
    let (rw, rh) = if full_ref { (w, h) } else { ((w / 32 * 16).max(16), (h / 32 * 16).max(16)) };
    let reference = if (rw, rh) == (w, h) { greyed } else { image::imageops::resize(&greyed, rw, rh, image::imageops::FilterType::Lanczos3) };
    let (rwu, rhu) = (rw as usize, rh as usize);
    let mut x = vec![0f32; 3 * rwu * rhu];
    for (i, p) in reference.pixels().enumerate() {
        for c in 0..3 {
            x[c * rwu * rhu + i] = p[c] as f32 / 255.0 * 2.0 - 1.0;
        }
    }
    let (lw, lh) = (wu / 16, hu / 16);
    let n = lw * lh;
    let mut times = Vec::new();
    let mut guard = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    crate::gpu::release(guard.is_none());
    let s = load(&mut guard)?;
    let res = run(s, x, rhu, rwu, n, lh, lw, seed, &mut times);
    eprintln!("gpumax: fill {w}x{h}: {}", times.iter().map(|(k, v)| format!("{k} {v:.1} s")).collect::<Vec<_>>().join(", "));
    let gpu = s.gpu;
    if res.is_err() {
        // A WebGPU failure leaves the session unusable; dropped, so the next try starts afresh.
        *guard = None;
    }
    drop(guard);
    unload_later();
    let img = res.map_err(|e| {
        if gpu && (e.contains("WebGPU") || e.contains("memory") || e.contains("Memory")) {
            format!("The graphics card ran out of memory for GPU (Smarter). Close other programs using it and restart RapidOne, or switch Remove to CPU. ({e})")
        } else {
            e
        }
    })?;
    *LAST_TIMES.lock().unwrap_or_else(|e| e.into_inner()) = times;
    if img.iter().any(|v| !v.is_finite()) {
        return Err(format!("GPU (Smarter) gave an empty result (NaN) on the {}", if gpu { "graphics card" } else { "CPU" }));
    }
    Ok(RgbImage::from_fn(w, h, |xx, yy| {
        let i = yy as usize * wu + xx as usize;
        image::Rgb(std::array::from_fn(|c| ((img[c * plane + i].clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8))
    }))
}

/// The sampling itself, on loaded sessions: the decoded picture (channels first, -1..1).
#[allow(clippy::too_many_arguments)]
/// `x`: the reference picture (channels first, -1..1), `rhu` x `rwu`; the picture painted is
/// `lh` x `lw` latent cells (`n` of them).
fn run(s: &mut Sessions, x: Vec<f32>, rhu: usize, rwu: usize, n: usize, lh: usize, lw: usize, seed: u32, times: &mut Vec<(String, f32)>) -> Result<Vec<f32>, String> {
    let t = std::time::Instant::now();
    let refl = encode_tiled(&mut s.enc, &x, rhu, rwu)?; // [1,128,rlh,rlw]
    times.push(("encode".to_string(), t.elapsed().as_secs_f32()));
    let (rlh, rlw) = (rhu / 16, rwu / 16);
    let nr = rlh * rlw;
    // Tokens: (h, w) row by row, 128 channels each.
    let pack = |v: &[f32], n: usize| -> Vec<f32> {
        let mut o = vec![0f32; n * CH];
        for c in 0..CH {
            for i in 0..n {
                o[i * CH + c] = v[c * n + i];
            }
        }
        o
    };
    let ref_tok = pack(&refl, nr);
    let mut lat = pack(&crate::rorem::noise(CH * n, seed), n);
    // Position ids (t, h, w, l): the picture's grid at t = 0, the reference's own grid at t = 10.
    let mut ids: Vec<i64> = Vec::with_capacity((n + nr) * 4);
    for (t, gh, gw) in [(0i64, lh, lw), (10, rlh, rlw)] {
        for yy in 0..gh {
            for xx in 0..gw {
                ids.extend_from_slice(&[t, yy as i64, xx as i64, 0]);
            }
        }
    }
    let txt_ids: Vec<i64> = (0..TXT_LEN).flat_map(|l| [0, 0, 0, l as i64]).collect();
    let sig = schedule(n, STEPS);
    let tensor_f = |shape: Vec<usize>, v: Vec<f32>| ort::value::Tensor::from_array((shape, v.into_boxed_slice())).map_err(|e| e.to_string());
    let tensor_i = |shape: Vec<usize>, v: Vec<i64>| ort::value::Tensor::from_array((shape, v.into_boxed_slice())).map_err(|e| e.to_string());
    for i in 0..STEPS {
        let t = std::time::Instant::now();
        let mut hs = Vec::with_capacity((n + nr) * CH);
        hs.extend_from_slice(&lat);
        hs.extend_from_slice(&ref_tok);
        let eps = {
            let out = s
                .tr
                .run(ort::inputs![
                    "hidden_states" => tensor_f(vec![1, n + nr, CH], hs)?,
                    "encoder_hidden_states" => tensor_f(vec![1, TXT_LEN, TXT_DIM], s.prompt.clone())?,
                    "timestep" => tensor_f(vec![1], vec![sig[i]])?,
                    "img_ids" => tensor_i(vec![1, n + nr, 4], ids.clone())?,
                    "txt_ids" => tensor_i(vec![1, TXT_LEN, 4], txt_ids.clone())?,
                ])
                .map_err(|e| e.to_string())?;
            let (_, o) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            o[..n * CH].to_vec()
        };
        let d = sig[i + 1] - sig[i];
        lat.iter_mut().zip(&eps).for_each(|(l, e)| *l += e * d);
        times.push((format!("step {}", i + 1), t.elapsed().as_secs_f32()));
    }
    // Back to [1,128,lh,lw] and decoded.
    let mut z = vec![0f32; CH * n];
    for i in 0..n {
        for c in 0..CH {
            z[c * n + i] = lat[i * CH + c];
        }
    }
    let t = std::time::Instant::now();
    let img = decode_tiled(&mut s.dec, &z, lh, lw)?;
    times.push(("decode".to_string(), t.elapsed().as_secs_f32()));
    Ok(img)
}

/// The patch: Klein's picture blended over `picture` where it removed the object (`hole`) and
/// wherever it changed things next to it (its shadow, a reflection): those changes are kept by
/// following them out from the hole, so the shadow doesn't stop in a hard line at the selection's
/// edge (21 Nov, P1368513: the couple's shadow on the paving), while differences elsewhere (Klein
/// redraws the whole picture slightly differently) stay the photo's. Soft-edged; 0 on `protect`.
pub fn composite_changed(picture: &RgbImage, filled: &RgbImage, hole: &[bool], protect: Option<&[bool]>) -> image::RgbaImage {
    let (w, h) = picture.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let blur = |v: &[f32], r: usize| -> Vec<f32> {
        let pass = |src: &[f32], horiz: bool| -> Vec<f32> {
            let mut out = vec![0f32; src.len()];
            let (n, m) = if horiz { (hu, wu) } else { (wu, hu) };
            for line in 0..n {
                let at = |k: usize| if horiz { line * wu + k } else { k * wu + line };
                let mut pre = vec![0f32; m + 1];
                for k in 0..m {
                    pre[k + 1] = pre[k] + src[at(k)];
                }
                for k in 0..m {
                    let (a, b) = (k.saturating_sub(r), (k + r + 1).min(m));
                    out[at(k)] = (pre[b] - pre[a]) / (b - a) as f32;
                }
            }
            out
        };
        pass(&pass(v, true), false)
    };
    // Klein's repaint as a whole comes out off the photo's tone (12-15% darker on P1368513, its own
    // and the ONNX version alike; a plain lit wall it paints then reads lighter than the wall round
    // it, 21 Nov). Per channel, a gain and offset fitted away from the hole bring it back to the
    // photo before anything else. Fitted first (24 Nov): at the Mac's 768 px the whole repaint drifted
    // past the 5% test below, so "what it changed" spread over most of the crop, the fit had too few
    // pixels left to run, and the patch came out off-tone with a hard line at the crop's edge.
    let long = wu.max(hu);
    let away = crate::removal::dilate(hole, wu, hu, (long / 12).max(8));
    let mut fit = [(1f32, 0f32); 3];
    for (c, f) in fit.iter_mut().enumerate() {
        let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for (i, (p, q)) in picture.pixels().zip(filled.pixels()).enumerate() {
            if away[i] {
                continue;
            }
            let (x, y) = (q[c] as f64, p[c] as f64);
            n += 1.0; sx += x; sy += y; sxx += x * x; sxy += x * y;
        }
        if n > 1000.0 {
            let var = sxx / n - (sx / n).powi(2);
            let gain = if var > 1.0 { ((sxy / n - sx / n * sy / n) / var).clamp(0.7, 1.4) } else { 1.0 };
            *f = (gain as f32, (sy / n - gain * sx / n) as f32);
        }
    }
    let filled = &RgbImage::from_fn(w, h, |x, y| {
        let q = filled.get_pixel(x, y);
        image::Rgb(std::array::from_fn(|c| (q[c] as f32 * fit[c].0 + fit[c].1).round().clamp(0.0, 255.0) as u8))
    });
    // Where Klein (tone-matched) changed the picture: both blurred (noise and resampling aside), any
    // channel off by 5%.
    let mut changed = vec![false; wu * hu];
    for c in 0..3 {
        let a: Vec<f32> = picture.pixels().map(|p| p[c] as f32).collect();
        let b: Vec<f32> = filled.pixels().map(|p| p[c] as f32).collect();
        let (a, b) = (blur(&a, 4), blur(&b, 4));
        for i in 0..wu * hu {
            changed[i] |= (a[i] - b[i]).abs() > 0.05 * 255.0;
        }
    }
    // Followed out from the hole through changed pixels (8 neighbours), but only so far (a shadow or
    // reflection next to it, not a redraw of the whole crop).
    let reachable = crate::removal::dilate(hole, wu, hu, (long / 5).max(16));
    let mut reach: Vec<bool> = hole.to_vec();
    let mut queue: std::collections::VecDeque<usize> = (0..wu * hu).filter(|&i| hole[i]).collect();
    while let Some(i) = queue.pop_front() {
        let (x, y) = ((i % wu) as i64, (i / wu) as i64);
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx < 0 || yy < 0 || xx >= wu as i64 || yy >= hu as i64 {
                continue;
            }
            let j = yy as usize * wu + xx as usize;
            if !reach[j] && changed[j] && reachable[j] {
                reach[j] = true;
                queue.push_back(j);
            }
        }
    }
    let grown = crate::removal::dilate(&reach, wu, hu, 3);
    // Then the brightness locally (24 Nov, the glass: a little darker than the shirt round it after the
    // whole-crop fit): the photo-minus-Klein difference where the photo is kept, spread smoothly into
    // what's replaced (normalised blur), added to Klein's picture.
    let keep: Vec<f32> = grown.iter().map(|&g| if g { 0.0 } else { 1.0 }).collect();
    let r = (long / 10).max(8);
    let smooth = |v: &[f32]| blur(&blur(v, r), r);
    let den = smooth(&keep);
    let mut corr: Vec<[f32; 3]> = vec![[0.0; 3]; wu * hu];
    for c in 0..3 {
        let d: Vec<f32> = picture.pixels().zip(filled.pixels()).zip(&keep).map(|((p, q), k)| (p[c] as f32 - q[c] as f32) * k).collect();
        let num = smooth(&d);
        for i in 0..wu * hu {
            corr[i][c] = if den[i] > 0.02 { num[i] / den[i] } else { 0.0 };
        }
    }
    let filled = &RgbImage::from_fn(w, h, |x, y| {
        let i = y as usize * wu + x as usize;
        let q = filled.get_pixel(x, y);
        image::Rgb(std::array::from_fn(|c| (q[c] as f32 + corr[i][c]).round().clamp(0.0, 255.0) as u8))
    });
    let m: Vec<f32> = grown.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let mut m = blur(&blur(&m, 10), 10);
    // Faded out towards the crop's sides, so the patch never ends in a straight line where the crop
    // does (24 Nov, the glass: a line down the hand). Not on a side the hole itself touches (an
    // object at the photo's edge, where the crop is the photo's edge too).
    let fade = (long / 16).max(8) as f32;
    let touches = |f: &dyn Fn(usize) -> bool| (0..wu * hu).any(|i| hole[i] && f(i));
    let (left, right) = (!touches(&|i| i % wu < 2), !touches(&|i| i % wu >= wu - 2));
    let (top, bottom) = (!touches(&|i| i / wu < 2), !touches(&|i| i / wu >= hu - 2));
    for (i, v) in m.iter_mut().enumerate() {
        let (x, y) = ((i % wu) as f32, (i / wu) as f32);
        let mut d = f32::MAX;
        if left { d = d.min(x); }
        if right { d = d.min(wu as f32 - 1.0 - x); }
        if top { d = d.min(y); }
        if bottom { d = d.min(hu as f32 - 1.0 - y); }
        *v *= (d / fade).clamp(0.0, 1.0);
    }
    if let Some(p) = protect {
        m.iter_mut().zip(p).for_each(|(v, &p)| if p { *v = 0.0 });
    }
    image::RgbaImage::from_fn(w, h, |x, y| {
        let i = y as usize * wu + x as usize;
        let a = (m[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        let p = if a > 0 { filled.get_pixel(x, y).0 } else { picture.get_pixel(x, y).0 };
        image::Rgba([p[0], p[1], p[2], a])
    })
}

/// Size of a VAE tile in latent cells (16 px), and their overlap. On WebGPU the VAE's convolutions
/// build full-resolution temporary buffers: at 1408x656 the fp16 VAE ran out of 16 GB in one piece
/// (21 Nov), so it works on overlapping 512 px tiles, blended linearly across the overlap.
/// RAPIDONE_GPUMAX_VAE_TILE: another tile size (cells; testing).
fn vae_tile() -> usize {
    std::env::var("RAPIDONE_GPUMAX_VAE_TILE").ok().and_then(|v| v.parse().ok()).unwrap_or(32)
}
/// 8 cells (128 px), a quarter of the tile as diffusers' tiled VAE: with 4 a seam showed on a dark,
/// smooth floor (P1368513, 21 Nov) where neighbouring tiles' brightness differed slightly.
const VAE_OVERLAP: usize = 8;

/// Tile starts covering `n` cells with tiles of `t` overlapping by VAE_OVERLAP.
fn tile_starts(n: usize, t: usize) -> Vec<usize> {
    if n <= t {
        return vec![0];
    }
    let mut v: Vec<usize> = (0..).map(|i| i * (t - VAE_OVERLAP)).take_while(|&p| p + t < n).collect();
    v.push(n - t);
    v
}

/// Weight of cell `i` of a tile `len` long starting at `start` in `n`: 1, ramping down over the
/// overlap towards a neighbouring tile (never at the picture's own edge).
fn ramp(i: usize, start: usize, len: usize, n: usize) -> f32 {
    let lo = if start == 0 { f32::MAX } else { (i as f32 + 0.5) / VAE_OVERLAP as f32 };
    let hi = if start + len >= n { f32::MAX } else { (len - i) as f32 / VAE_OVERLAP as f32 - 0.5 / VAE_OVERLAP as f32 };
    lo.min(hi).clamp(0.0, 1.0).max(1e-3)
}

/// The VAE encoder over overlapping tiles: image [3,hu,wu] (-1..1) -> latents [128, hu/16, wu/16].
fn encode_tiled(enc: &mut ort::session::Session, x: &[f32], hu: usize, wu: usize) -> Result<Vec<f32>, String> {
    let (lh, lw, t) = (hu / 16, wu / 16, vae_tile());
    let mut acc = vec![0f32; CH * lh * lw];
    let mut wsum = vec![0f32; lh * lw];
    for &y0 in &tile_starts(lh, t) {
        for &x0 in &tile_starts(lw, t) {
            let (th, tw) = (t.min(lh), t.min(lw));
            let (ph, pw) = (th * 16, tw * 16);
            let mut tile = vec![0f32; 3 * ph * pw];
            for c in 0..3 {
                for yy in 0..ph {
                    let src = c * hu * wu + (y0 * 16 + yy) * wu + x0 * 16;
                    tile[c * ph * pw + yy * pw..c * ph * pw + (yy + 1) * pw].copy_from_slice(&x[src..src + pw]);
                }
            }
            let l = run1(enc, "image", vec![1, 3, ph, pw], tile)?;
            for yy in 0..th {
                for xx in 0..tw {
                    let wgt = ramp(yy, y0, th, lh) * ramp(xx, x0, tw, lw);
                    let gi = (y0 + yy) * lw + x0 + xx;
                    wsum[gi] += wgt;
                    for c in 0..CH {
                        acc[c * lh * lw + gi] += l[c * th * tw + yy * tw + xx] * wgt;
                    }
                }
            }
        }
    }
    for c in 0..CH {
        for i in 0..lh * lw {
            acc[c * lh * lw + i] /= wsum[i];
        }
    }
    Ok(acc)
}

/// The VAE decoder over overlapping tiles: latents [128,lh,lw] -> image [3, 16lh, 16lw] (-1..1).
fn decode_tiled(dec: &mut ort::session::Session, z: &[f32], lh: usize, lw: usize) -> Result<Vec<f32>, String> {
    let t = vae_tile();
    let (hu, wu) = (lh * 16, lw * 16);
    let mut acc = vec![0f32; 3 * hu * wu];
    let mut wsum = vec![0f32; hu * wu];
    for &y0 in &tile_starts(lh, t) {
        for &x0 in &tile_starts(lw, t) {
            let (th, tw) = (t.min(lh), t.min(lw));
            let mut tile = vec![0f32; CH * th * tw];
            for c in 0..CH {
                for yy in 0..th {
                    let src = c * lh * lw + (y0 + yy) * lw + x0;
                    tile[c * th * tw + yy * tw..c * th * tw + (yy + 1) * tw].copy_from_slice(&z[src..src + tw]);
                }
            }
            let img = run1(dec, "latents", vec![1, CH, th, tw], tile)?;
            let (ph, pw) = (th * 16, tw * 16);
            for yy in 0..ph {
                for xx in 0..pw {
                    // The ramp in latent cells, at pixel resolution.
                    let wy = ramp(yy / 16, y0, th, lh) * 0.5 + ramp((yy + 15) / 16, y0, th, lh).min(1.0) * 0.5;
                    let wx = ramp(xx / 16, x0, tw, lw) * 0.5 + ramp(((xx + 15) / 16).min(tw - 1), x0, tw, lw).min(1.0) * 0.5;
                    let wgt = wy * wx;
                    let gi = (y0 * 16 + yy) * wu + x0 * 16 + xx;
                    wsum[gi] += wgt;
                    for c in 0..3 {
                        acc[c * hu * wu + gi] += img[c * ph * pw + yy * pw + xx] * wgt;
                    }
                }
            }
        }
    }
    for c in 0..3 {
        for i in 0..hu * wu {
            acc[c * hu * wu + i] /= wsum[i];
        }
    }
    Ok(acc)
}

fn run1(s: &mut ort::session::Session, name: &'static str, shape: Vec<usize>, data: Vec<f32>) -> Result<Vec<f32>, String> {
    let out = s
        .run(ort::inputs![name => ort::value::Tensor::from_array((shape, data.into_boxed_slice())).map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?;
    let (_, o) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    Ok(o.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_matches_diffusers() {
        // diffusers 0.43, FlowMatchEulerDiscreteScheduler with Klein's config, set_timesteps(sigmas =
        // linspace(1, 1/4, 4), mu = compute_empirical_mu(seq, 4)) (21 Nov).
        for (seq, want) in [
            (1056, [1.0, 0.958194, 0.88426, 0.718046, 0.0]),
            (3608, [1.0, 0.966053, 0.904633, 0.759728, 0.0]),
            (6600, [1.0, 0.935382, 0.828332, 0.616622, 0.0]),
        ] {
            let s = schedule(seq, 4);
            assert!(s.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-5), "{seq}: {s:?}");
        }
    }

    #[test]
    fn allowzero_cleared_in_place() {
        // Two Reshape attributes (allowzero 1, then 0 already) and an unrelated int attribute.
        let mut g = b"..\x0a\x09allowzero\x18\x01\xa0\x01\x02..\x0a\x09allowzero\x18\x00..\x0a\x04axis\x18\x01".to_vec();
        let len = g.len();
        assert_eq!(clear_allowzero(&mut g), 1);
        assert_eq!(g.len(), len);
        assert_eq!(&g[..15], b"..\x0a\x09allowzero\x18\x00");
        assert_eq!(clear_allowzero(&mut g), 0);
        assert!(g.ends_with(b"axis\x18\x01"));
    }
}
