//! Remove "GPU": RORem (an SDXL inpainter trained only to remove what's masked;
//! github.com/leeruibin/RORem) run through ONNX Runtime on the graphics card
//! (Windows: DirectML; Linux: WebGPU; Apple Silicon: CoreML), no Python.
//!
//! The model files (converted from LetsThink/RORem by ~/Work/rorem-onnx/scripts,
//! in the models folder's `rorem/`):
//! - `unet.onnx` + `unet.onnx.data`: the UNet, 4-bit MatMul weights (MatMulNBits, block 64,
//!   accuracy level 4; ~1.9 GB) and fp16 convolutions, f32 inputs/outputs:
//!   sample [1,12,h,w] (scaled latent, mask, masked picture's latent, 3 zero channels), timestep [1],
//!   encoder_hidden_states [1,77,2048], text_embeds [1,1280], time_ids [1,6];
//! - `vae_encoder.onnx` / `vae_decoder.onnx`: SDXL's VAE in fp32 (it gives NaNs in fp16);
//! - `empty_prompt.f32`: the empty prompt's embeddings (77x2048 then 1280). RORem runs
//!   without words (guidance 1), so the text encoders are never needed.
//!
//! The weights carry SDXL's CreativeML OpenRAIL++-M use restrictions (RORem is a
//! fine-tune of SDXL inpainting), so they're an optional download, never bundled.
//!
//! The sampler is diffusers' EulerDiscreteScheduler as RORem ships it (scaled_linear
//! 0.00085-0.012, "leading" spacing, steps_offset 1) with the inpainting pipeline's
//! strength 0.99; checked against diffusers' StableDiffusionXLInpaintPipeline
//! (~/Work/rorem-onnx/scripts/loop.py).
use image::{Rgb, RgbImage};
use std::path::PathBuf;
use std::sync::Mutex;

/// Long edge RORem works at. `LetsThink/RORem` is the authors' RORem-mixed (trained at 512 and
/// 1024; its weights match their RORem_mixed release to fp16 rounding, checked 21 Nov), so it
/// works at 1024 (21 Nov, Chris: at 512 a big area came out soft; 1024 costs ~3.6 GB peak of
/// graphics memory instead of ~2.4 GB, and about twice the time).
pub const SIDE: u32 = 1024;
/// Euler steps (RORem's card: 15-30 work as well as 50).
pub const STEPS: usize = 24;
const STRENGTH: f64 = 0.99;
const VAE_SCALE: f32 = 0.13025;
const FOLDER: &str = "rorem";

struct Sessions {
    unet: ort::session::Session,
    enc: ort::session::Session,
    dec: ort::session::Session,
    hidden: Vec<f32>,
    pooled: Vec<f32>,
    gpu: bool,
    /// The UNet's input channels: 9 as trained, 12 in the shipped model (see `fill`).
    unet_ch: usize,
}

static LOADED: Mutex<Option<Sessions>> = Mutex::new(None);

/// The folder holding RORem's ONNX files, if they're all there.
pub fn folder() -> Option<PathBuf> {
    crate::mask::model_dirs().into_iter().map(|d| d.join(FOLDER)).find(|d| {
        ["unet.onnx", "unet.onnx.data", "vae_encoder.onnx", "vae_decoder.onnx", "empty_prompt.f32"].iter().all(|f| d.join(f).is_file())
    })
}

pub fn available() -> bool {
    folder().is_some()
}

/// A session on the graphics card (CPU when `cpu`, or when no GPU provider works).
pub(crate) fn build(env: &ort::environment::Environment, path: &std::path::Path, cpu: bool) -> Result<(ort::session::Session, bool), String> {
    build_cache(env, path, cpu, None)
}

/// `build`, with WebGPU's buffer cache modes (storage, default) chosen by the caller on Linux (GPU
/// (Smarter) uses bucket/bucket: its lowest peak, 21 Nov); None: RORem's (simple/disabled).
pub(crate) fn build_cache(env: &ort::environment::Environment, path: &std::path::Path, cpu: bool, cache: Option<(&str, &str)>) -> Result<(ort::session::Session, bool), String> {
    let _ = cache;
    let plain = || -> Result<ort::session::Session, String> {
        let b = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
        // ort's own default on a Mac is CoreML (see ortgpu.rs): the CPU fallback asks for the CPU.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let b = b.with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU).map_err(|e| e.to_string())?;
        #[allow(unused_mut)]
        let mut b = b;
        b.commit_from_file(path).map_err(|e| e.to_string())
    };
    if cpu {
        return Ok((plain()?, false));
    }
    let gpu = (|| -> Option<ort::session::Session> {
        let b = ort::session::Session::builder(env).ok()?;
        // Testing: RAPIDONE_ROREM_PREPACK=0 stops ONNX Runtime keeping re-arranged copies of weights.
        let b = if std::env::var("RAPIDONE_ROREM_PREPACK").as_deref() == Ok("0") { b.with_prepacking(false).ok()? } else { b };
        // Testing: RAPIDONE_ROREM_OPT=0..3, the graph optimisation level.
        let b = match std::env::var("RAPIDONE_ROREM_OPT").as_deref() {
            Ok("0") => b.with_optimization_level(ort::session::builder::GraphOptimizationLevel::Disable).ok()?,
            Ok("1") => b.with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level1).ok()?,
            Ok("2") => b.with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level2).ok()?,
            _ => b,
        };
        #[cfg(windows)]
        let b = b
            .with_parallel_execution(false)
            .ok()?
            .with_memory_pattern(false)
            .ok()?
            .with_execution_providers([ort::ep::DirectML::default().with_device_id(0).build().error_on_failure()])
            .ok()?;
        // Apple Silicon too: WebGPU over Metal. CoreML couldn't take most of Klein (its MatMulNBits layers,
        // unbounded shapes), so it ran in dozens of pieces compiled one by one (Mac Studio, 24 Nov).
        #[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
        let b = {
            use ort::ep::webgpu::BufferCacheMode as M;
            // Storage buffers: "simple" (measured 20 Nov, 512x256 fill, RTX 5070 Ti: peak 8.8 GB
            // vs 9.8 GB with ONNX Runtime's default "bucket", same speed; "lazyRelease" and
            // "disabled" ran out of memory). Testing: RAPIDONE_ROREM_CACHE / RAPIDONE_ROREM_DEFAULT_CACHE
            // = bucket | simple | lazy | disabled.
            let pick = |var: &str, dflt: M| match std::env::var(var).as_deref() {
                Ok("bucket") => M::Bucket,
                Ok("simple") => M::Simple,
                Ok("lazy") => M::LazyRelease,
                Ok("disabled") => M::Disabled,
                _ => dflt,
            };
            let named = |v: &str| match v {
                "bucket" => Some(M::Bucket),
                "simple" => Some(M::Simple),
                "lazy" => Some(M::LazyRelease),
                "disabled" => Some(M::Disabled),
                _ => None,
            };
            let (mode, dmode) = match cache {
                Some((a, b)) if std::env::var("RAPIDONE_ROREM_CACHE").is_err() => (named(a).unwrap_or(M::Bucket), named(b).unwrap_or(M::Bucket)),
                _ => (pick("RAPIDONE_ROREM_CACHE", M::Simple), pick("RAPIDONE_ROREM_DEFAULT_CACHE", M::Disabled)),
            };
            b.with_execution_providers([ort::ep::WebGPU::default().with_storage_buffer_cache_mode(mode).with_default_buffer_cache_mode(dmode).build().error_on_failure()]).ok()?
        };
        #[cfg(not(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))))]
        return None;
        #[allow(unreachable_code)]
        let mut b = b;
        // Testing: RAPIDONE_ORT_PROFILE=<prefix> writes ONNX Runtime's per-node timings (JSON).
        if let Ok(p) = std::env::var("RAPIDONE_ORT_PROFILE") {
            b = b.with_profiling(p).ok()?;
        }
        b.commit_from_file(path).map_err(|e| eprintln!("rorem: GPU session for {}: {e}", path.display())).ok()
    })();
    match gpu {
        Some(s) => Ok((s, true)),
        None => Ok((plain()?, false)),
    }
}

/// Loads RORem (once; ~3.6 GB on the card at its peak at 1024 px, the VAE on the CPU). `RAPIDONE_REMOVE_CPU=1` keeps it on the CPU (testing).
fn load(slot: &mut Option<Sessions>) -> Result<&mut Sessions, String> {
    if slot.is_none() {
        let dir = folder().ok_or("RORem (Remove GPU) isn't installed")?;
        let env = crate::mask::ort_env().ok_or("ONNX Runtime didn't start")?;
        let cpu = std::env::var("RAPIDONE_REMOVE_CPU").is_ok();
        // Testing: RAPIDONE_REMOVE_SPLIT=unet-cpu puts the UNet on the CPU; vae-gpu the VAE on the card.
        let split = std::env::var("RAPIDONE_REMOVE_SPLIT").unwrap_or_default();
        let t0 = std::time::Instant::now();
        let (unet, gpu) = build(env, &dir.join("unet.onnx"), cpu || split == "unet-cpu")?;
        let t_unet = t0.elapsed().as_secs_f32();
        // The VAE runs on the CPU unless asked otherwise: in fp32 (SDXL's VAE gives NaNs in
        // fp16) its working memory on the graphics card was ~3.2 GB at 512 px, more than the
        // 4-bit UNet's weights. On the CPU RORem peaks at ~2.4 GB of graphics memory (fits 8 GB
        // cards next to the app), for ~1 s more per fill (measured 20 Nov, RTX 5070 Ti: 3.9 s
        // vs 2.8 s). RAPIDONE_REMOVE_SPLIT=vae-gpu puts it on the card.
        let vae_cpu = cpu || split != "vae-gpu" || !gpu;
        let (enc, _) = build(env, &dir.join("vae_encoder.onnx"), vae_cpu)?;
        let (dec, _) = build(env, &dir.join("vae_decoder.onnx"), vae_cpu)?;
        let bytes = std::fs::read(dir.join("empty_prompt.f32")).map_err(|e| e.to_string())?;
        let all: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        if all.len() != 77 * 2048 + 1280 {
            return Err("RORem's empty_prompt.f32 is the wrong size".into());
        }
        let unet_ch = match unet.inputs().first().map(|i| i.dtype()) {
            Some(ort::value::ValueType::Tensor { shape, .. }) if shape.len() == 4 && shape[1] > 0 => shape[1] as usize,
            _ => 9,
        };
        eprintln!("rorem: loaded in {:.1} s (UNet {t_unet:.1} s) on the {}", t0.elapsed().as_secs_f32(), if gpu { "graphics card" } else { "CPU" });
        *slot = Some(Sessions { hidden: all[..77 * 2048].to_vec(), pooled: all[77 * 2048..].to_vec(), unet, enc, dec, gpu, unet_ch });
    }
    Ok(slot.as_mut().unwrap())
}

/// Frees RORem's memory (graphics card and RAM) until it's next used. Waits for
/// a fill in progress (it holds the same lock).
pub fn unload() {
    let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        let t = std::time::Instant::now();
        *g = None;
        eprintln!("rorem: unloaded in {:.2} s", t.elapsed().as_secs_f32());
    }
}

/// Once loaded, RORem stays until 15 minutes pass without a removal, whether Remove is open
/// or not (Chris, 20 Nov: working through an album it's used photo after photo, with Remove
/// opened and closed in between). Switching Remove to CPU or quitting the app frees it at
/// once. It holds ~2 GB of graphics memory while it waits.
const IDLE_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);

static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Counts holds and uses: a timer only unloads if nothing happened since it started.
static EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump() -> u64 {
    EVENTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Unloads after `after` unless something happens to RORem meanwhile.
fn unload_later(after: std::time::Duration) {
    let n = bump();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        if EVENTS.load(std::sync::atomic::Ordering::SeqCst) == n {
            unload();
        }
    });
}

/// The Remove tool opened or closed (`gpu`: it's in GPU mode). Opened in GPU mode, RORem
/// starts loading in the background if it isn't loaded (~5 s), so it's ready by the time a
/// box is drawn. Opening and closing Remove never unload it or restart its clock (Chris,
/// 20 Nov: working through an album Remove is opened and closed photo after photo): only
/// 15 minutes without a removal, switching to CPU, or quitting free it.
pub fn hold(open: bool, gpu: bool) {
    HELD.store(open, std::sync::atomic::Ordering::SeqCst);
    if open && gpu {
        preload();
    }
}

/// Loads RORem on a background thread if it's installed and not loaded (after freeing the
/// renderer's picture buffers); its 15-minute clock starts then. Already loaded: nothing
/// changes. A removal asked for meanwhile waits for the load.
pub fn preload() {
    if !available() {
        return;
    }
    std::thread::spawn(|| {
        let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_some() {
            return;
        }
        crate::gpu::release(true);
        if let Err(e) = load(&mut g) {
            eprintln!("rorem: preload failed: {e}");
            *g = None;
            return;
        }
        drop(g);
        unload_later(IDLE_AFTER);
    });
}

/// Whether the Remove tool is open.
pub fn held() -> bool {
    HELD.load(std::sync::atomic::Ordering::SeqCst)
}

/// Whether the loaded RORem runs on the graphics card (None: not loaded).
pub fn on_gpu() -> Option<bool> {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.gpu)
}

/// The size RORem works at for a w x h picture: long edge SIDE, both sides
/// multiples of 64 (the UNet halves the latent three times).
pub fn work_size(w: u32, h: u32) -> (u32, u32) {
    // RAPIDONE_ROREM_SIDE: another long edge (testing).
    let side = std::env::var("RAPIDONE_ROREM_SIDE").ok().and_then(|v| v.parse().ok()).unwrap_or(SIDE);
    let s = side as f32 / w.max(h) as f32;
    let r = |v: u32| (((v as f32 * s) / 64.0).round() as u32).max(1) * 64;
    (r(w), r(h))
}

/// The sampler's timesteps and sigmas (one more sigma than timesteps, ending in 0).
fn schedule(steps: usize) -> (Vec<f32>, Vec<f32>) {
    let (b0, b1) = (0.00085f64.sqrt(), 0.012f64.sqrt());
    let mut acp = Vec::with_capacity(1000);
    let mut p = 1.0f64;
    for i in 0..1000 {
        let b = b0 + (b1 - b0) * i as f64 / 999.0;
        p *= 1.0 - b * b;
        acp.push(p);
    }
    let all: Vec<f64> = acp.iter().map(|a| ((1.0 - a) / a).sqrt()).collect();
    let ratio = 1000 / steps;
    let ts: Vec<f64> = (0..steps).rev().map(|i| (i * ratio) as f64 + 1.0).collect();
    // np.interp on whole numbers: the table itself.
    let mut sig: Vec<f64> = ts.iter().map(|&t| all[(t as usize).min(999)]).collect();
    sig.push(0.0);
    let init = ((steps as f64 * STRENGTH) as usize).min(steps);
    let start = steps - init;
    (ts[start..].iter().map(|&t| t as f32).collect(), sig[start..].iter().map(|&s| s as f32).collect())
}

/// Standard normal noise from a seed (splitmix64 + Box-Muller).
pub(crate) fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ seed as u64;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = Vec::with_capacity(n + 1);
    while out.len() < n {
        let (u1, u2) = (next().max(1e-12), next());
        let r = (-2.0 * u1.ln()).sqrt();
        out.push((r * (std::f64::consts::TAU * u2).cos()) as f32);
        out.push((r * (std::f64::consts::TAU * u2).sin()) as f32);
    }
    out.truncate(n);
    out
}

fn run1(s: &mut ort::session::Session, name: &'static str, shape: [usize; 4], data: Vec<f32>) -> Result<Vec<f32>, String> {
    let out = s
        .run(ort::inputs![name => ort::value::Tensor::from_array((shape, data.into_boxed_slice())).map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?;
    let (_, o) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    Ok(o.to_vec())
}

/// The sampling itself, on loaded sessions: the decoded picture (channels first, -1..1).
#[allow(clippy::too_many_arguments)]
fn denoise(
    s: &mut Sessions,
    x: &[f32],
    masked: &[f32],
    lm: &[f32],
    (ru, hu, lw, lh, rw, rh): (usize, usize, usize, usize, u32, u32),
    seed: u32,
    progress: &mut impl FnMut(usize, usize),
) -> Result<Vec<f32>, String> {
    let lplane = lw * lh;
    let shape = [1usize, 3, hu, ru];
    let image_lat: Vec<f32> = run1(&mut s.enc, "image", shape, x.to_vec())?.into_iter().map(|v| v * VAE_SCALE).collect();
    let masked_lat: Vec<f32> = run1(&mut s.enc, "image", shape, masked.to_vec())?.into_iter().map(|v| v * VAE_SCALE).collect();
    let (ts, sig) = schedule(STEPS);
    let n = noise(4 * lplane, seed);
    let mut lat: Vec<f32> = image_lat.iter().zip(&n).map(|(a, b)| a + b * sig[0]).collect();
    let time_ids = vec![rh as f32, rw as f32, 0.0, 0.0, rh as f32, rw as f32];
    for (i, &t) in ts.iter().enumerate() {
        let k = 1.0 / (sig[i] * sig[i] + 1.0).sqrt();
        // Latent (4), mask (1), masked picture's latent (4). The shipped model's first
        // convolution is padded to 12 input channels with zero weights (same result):
        // ONNX Runtime's WebGPU backend computes the 9-channel one wrongly (noise; found
        // 20 Nov, tools/rorem-onnx/pad_conv_in.py), so 3 zero channels follow.
        let mut inp = Vec::with_capacity(s.unet_ch * lplane);
        inp.extend(lat.iter().map(|v| v * k));
        inp.extend_from_slice(lm);
        inp.extend_from_slice(&masked_lat);
        inp.resize(s.unet_ch * lplane, 0.0);
        let tensor = |shape: Vec<usize>, v: Vec<f32>| ort::value::Tensor::from_array((shape, v.into_boxed_slice())).map_err(|e| e.to_string());
        let eps = {
            let out = s
                .unet
                .run(ort::inputs![
                    "sample" => tensor(vec![1, s.unet_ch, lh, lw], inp)?,
                    "timestep" => tensor(vec![1], vec![t])?,
                    "encoder_hidden_states" => tensor(vec![1, 77, 2048], s.hidden.clone())?,
                    "text_embeds" => tensor(vec![1, 1280], s.pooled.clone())?,
                    "time_ids" => tensor(vec![1, 6], time_ids.clone())?,
                ])
                .map_err(|e| e.to_string())?;
            let (_, o) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            o.to_vec()
        };
        let d = sig[i + 1] - sig[i];
        lat.iter_mut().zip(&eps).for_each(|(l, e)| *l += e * d);
        progress(i + 1, ts.len());
    }
    run1(&mut s.dec, "latent", [1, 4, lh, lw], lat.into_iter().map(|v| v / VAE_SCALE).collect())
}

/// RORem's fill of `hole` (true = remove; one per pixel of `picture`), worked at
/// `work_size` and scaled back: the whole picture as RORem paints it (the caller
/// keeps the original outside the hole). `progress(done, total)` after each step.
pub fn fill(picture: &RgbImage, hole: &[bool], seed: u32, mut progress: impl FnMut(usize, usize)) -> Result<RgbImage, String> {
    let (w, h) = picture.dimensions();
    if hole.len() != (w * h) as usize {
        return Err("RORem: the hole isn't the picture's size".into());
    }
    let (rw, rh) = work_size(w, h);
    let small = image::imageops::resize(picture, rw, rh, image::imageops::FilterType::Lanczos3);
    // The hole at the work size: a pixel is in it if the picture's pixel under its centre is.
    let (ru, hu) = (rw as usize, rh as usize);
    let m: Vec<f32> = (0..ru * hu)
        .map(|i| {
            let (x, y) = (i % ru, i / ru);
            let sx = (((x as f32 + 0.5) * w as f32 / rw as f32) as usize).min(w as usize - 1);
            let sy = (((y as f32 + 0.5) * h as f32 / rh as f32) as usize).min(h as usize - 1);
            if hole[sy * w as usize + sx] { 1.0 } else { 0.0 }
        })
        .collect();
    let plane = ru * hu;
    // Dark pictures are shown to RORem brightened (a gamma lift that brings the median to ~45%)
    // and its fill darkened back by the same curve. SDXL pulls what it paints towards mid-grey,
    // so in a night scene the fill's darks came out as a light haze (21 Nov, P1368513: darkest
    // 2% at 20 of 255 against the street's 12.5, the fill's edge a visibly lighter band). Lifted
    // to gamma 0.5 they came out at 17.7 and the band went; darkening back also squeezes what
    // haze is left. Normal and bright pictures (median 45% or more) go in unchanged.
    // RAPIDONE_ROREM_GAMMA: a fixed gamma (testing).
    let gamma: f32 = std::env::var("RAPIDONE_ROREM_GAMMA").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        let mut l: Vec<u8> = small.pixels().map(|p| ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8).collect();
        let mid = l.len() / 2;
        let median = (*l.select_nth_unstable(mid).1 as f32 / 255.0).max(0.02);
        (0.45f32.ln() / median.ln()).clamp(0.5, 1.0)
    });
    let mut x = vec![0f32; 3 * plane];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            x[c * plane + i] = lift(p[c] as f32 / 255.0, gamma) * 2.0 - 1.0;
        }
    }
    let masked: Vec<f32> = x.iter().enumerate().map(|(i, &v)| v * (1.0 - m[i % plane])).collect();
    let (lw, lh) = (ru / 8, hu / 8);
    let lplane = lw * lh;
    // torch's nearest downscale, as the pipeline does: every 8th pixel.
    let lm: Vec<f32> = (0..lplane).map(|i| m[(i / lw) * 8 * ru + (i % lw) * 8]).collect();

    // The app's own GPU renderer keeps full-size picture buffers on the card until it's been idle
    // a while (gpu::IDLE_BUFFERS), so they go first (20 Nov: the dev app had
    // 10.8 GB of a 16 GB card and RORem's load ran out of memory). The renderer starts again by
    // itself on the next render. A failure on the graphics card leaves the session unusable
    // ("invalid due to a previous error"), so it's dropped, the memory freed, and tried once more.
    let mut guard = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let mut tries = 0;
    let out = loop {
        crate::gpu::release(guard.is_none());
        let s = load(&mut guard)?;
        match denoise(s, &x, &masked, &lm, (ru, hu, lw, lh, rw, rh), seed, &mut progress) {
            Ok(o) => break o,
            Err(e) => {
                let gpu = s.gpu;
                if gpu {
                    *guard = None;
                }
                if gpu && tries == 0 {
                    tries += 1;
                    eprintln!("rorem: {e}; freeing GPU memory and trying again");
                    continue;
                }
                return Err(if e.contains("OUT_OF_DEVICE_MEMORY") || e.contains("out of memory") || e.contains("WebGPU") {
                    format!("The graphics card hasn't enough free memory for GPU Remove (it needs about 4 GB). Close other programs using it, or switch Remove to CPU. ({e})")
                } else {
                    e
                });
            }
        }
    };
    drop(guard);
    unload_later(IDLE_AFTER);
    if out.iter().any(|v| !v.is_finite()) {
        return Err("RORem gave an empty result (NaN)".into());
    }
    let mut out: Vec<f32> = out.iter().map(|v| lift((v.clamp(-1.0, 1.0) + 1.0) * 0.5, 1.0 / gamma) * 255.0).collect();
    // RAPIDONE_ROREM_NOMATCH=1: without it (testing).
    if std::env::var("RAPIDONE_ROREM_NOMATCH").is_err() {
        match_surroundings(&small, &mut out, &m, ru, hu);
    }
    let filled = RgbImage::from_fn(rw, rh, |x, y| {
        let i = y as usize * ru + x as usize;
        Rgb(std::array::from_fn(|c| out[c * plane + i].clamp(0.0, 255.0).round() as u8))
    });
    Ok(if (rw, rh) == (w, h) { filled } else { image::imageops::resize(&filled, w, h, image::imageops::FilterType::CatmullRom) })
}

fn lift(v: f32, gamma: f32) -> f32 {
    v.max(0.0).powf(gamma)
}

/// Pulls RORem's fill to the photo's brightness and colour where they meet. SDXL inpainting
/// drifts in tone inside the hole (21 Nov, P1368513: the wall it painted behind a couple was
/// ~10% lighter and paler than the lit wall beside it, a visible edge at the join). Outside the
/// hole RORem copies the photo almost exactly, so comparing the two there finds nothing (the first
/// version did that); what's compared is the fill just inside the hole's edge against the photo
/// just outside it. Each is averaged over a band along the edge (normalised blur, so the averages
/// are of the same neighbourhood on both sides) and the ratio spread over the hole, fading to the
/// whole edge's ratio where the blur doesn't reach (the middle of a big hole). Only low-frequency
/// tone changes; RORem's texture stays as painted. `out` is channels first, 0..255, at the work size.
fn match_surroundings(photo: &RgbImage, out: &mut [f32], hole: &[f32], w: usize, h: usize) {
    let plane = w * h;
    let side = w.max(h);
    let band = (side / 40).max(3);
    let inside: Vec<bool> = hole.iter().map(|&v| v > 0.5).collect();
    let outside: Vec<bool> = inside.iter().map(|&v| !v).collect();
    let near = grow(&inside, w, h, band);
    let reach_in = grow(&outside, w, h, band);
    // Just outside the hole (the photo) and just inside it (RORem's fill).
    let w_out: Vec<f32> = (0..plane).map(|i| if near[i] && !inside[i] { 1.0 } else { 0.0 }).collect();
    let w_in: Vec<f32> = (0..plane).map(|i| if inside[i] && reach_in[i] { 1.0 } else { 0.0 }).collect();
    let (n_out, n_in) = (w_out.iter().sum::<f32>(), w_in.iter().sum::<f32>());
    if n_out < 16.0 || n_in < 16.0 {
        return;
    }
    // RAPIDONE_ROREM_MATCH_DIV: the blur radius as a fraction of the long edge (testing).
    let div: usize = std::env::var("RAPIDONE_ROREM_MATCH_DIV").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let radius = (side / div).max(4);
    let wb_out = box_blur3(&w_out, w, h, radius);
    let wb_in = box_blur3(&w_in, w, h, radius);
    // The local figures count fully once there's a few percent of edge in reach, fading to the
    // whole edge's figures where there's none.
    let average = |v: &dyn Fn(usize) -> f32, wt: &[f32], wb: &[f32], n: f32| -> Vec<f32> {
        let x: Vec<f32> = (0..plane).map(|i| wt[i] * v(i)).collect();
        let all = x.iter().sum::<f32>() / n;
        let xb = box_blur3(&x, w, h, radius);
        (0..plane)
            .map(|i| {
                let k = wb[i] / (wb[i] + 0.02);
                if wb[i] > 1e-6 { k * xb[i] / wb[i] + (1.0 - k) * all } else { all }
            })
            .collect()
    };
    // Spread round the hole's soft edge too (where the fill is still partly used).
    let used = grow(&inside, w, h, band * 2);
    for c in 0..3 {
        let o: Vec<f32> = out[c * plane..(c + 1) * plane].to_vec();
        let p = average(&|i| photo.as_raw()[i * 3 + c] as f32, &w_out, &wb_out, n_out);
        let f = average(&|i| o[i], &w_in, &wb_in, n_in);
        for i in 0..plane {
            if used[i] {
                // As a ratio (a lit wall and its shadow drift alike); +4 keeps blacks steady.
                let gain = ((p[i] + 4.0) / (f[i] + 4.0)).clamp(0.6, 1.6);
                out[c * plane + i] = (o[i] + 4.0) * gain - 4.0;
            }
        }
    }
}

/// `mask` grown by `r` pixels (square neighbourhood, separable).
fn grow(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let mut last: Option<usize> = None;
        let mut next = vec![usize::MAX; w];
        for x in (0..w).rev() {
            if row[x] {
                last = Some(x);
            }
            next[x] = last.unwrap_or(usize::MAX);
        }
        let mut prev: Option<usize> = None;
        for x in 0..w {
            if row[x] {
                prev = Some(x);
            }
            let a = prev.is_some_and(|p| x - p <= r);
            let b = next[x] != usize::MAX && next[x] - x <= r;
            tmp[y * w + x] = a || b;
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut prev: Option<usize> = None;
        let mut next = vec![usize::MAX; h];
        let mut last: Option<usize> = None;
        for y in (0..h).rev() {
            if tmp[y * w + x] {
                last = Some(y);
            }
            next[y] = last.unwrap_or(usize::MAX);
        }
        for y in 0..h {
            if tmp[y * w + x] {
                prev = Some(y);
            }
            out[y * w + x] = prev.is_some_and(|p| y - p <= r) || (next[y] != usize::MAX && next[y] - y <= r);
        }
    }
    out
}

/// Three box blurs of radius `r` (close to a Gaussian), edges clamped.
fn box_blur3(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut a = v.to_vec();
    let mut b = vec![0f32; w * h];
    for _ in 0..3 {
        blur_pass(&a, &mut b, w, h, r, true);
        blur_pass(&b, &mut a, w, h, r, false);
    }
    a
}

fn blur_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, rows: bool) {
    let (n, lines) = if rows { (w, h) } else { (h, w) };
    let at = |line: usize, k: usize| if rows { line * w + k } else { k * w + line };
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let get = |k: isize| src[at(line, k.clamp(0, n as isize - 1) as usize)];
        let mut sum: f32 = (-(r as isize)..=r as isize).map(get).sum();
        for k in 0..n {
            dst[at(line, k)] = sum * norm;
            sum += get(k as isize + r as isize + 1) - get(k as isize - r as isize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surroundings_pull_a_light_patch_back() {
        // A flat grey photo; RORem's "fill" 20 levels lighter everywhere: afterwards the hole
        // matches the photo.
        let (w, h) = (128usize, 64usize);
        let photo = RgbImage::from_pixel(w as u32, h as u32, Rgb([100, 110, 120]));
        let hole: Vec<f32> = (0..w * h).map(|i| if (40..88).contains(&(i % w)) && (16..48).contains(&(i / w)) { 1.0 } else { 0.0 }).collect();
        let mut out: Vec<f32> = (0..3 * w * h).map(|i| [120.0, 130.0, 140.0][i / (w * h)]).collect();
        match_surroundings(&photo, &mut out, &hole, w, h);
        let centre = 32 * w + 64;
        for (c, want) in [100.0, 110.0, 120.0].iter().enumerate() {
            assert!((out[c * w * h + centre] - want).abs() < 1.0, "channel {c}: {}", out[c * w * h + centre]);
        }
    }

    #[test]
    fn a_fill_lighter_than_its_edge_comes_back() {
        // As RORem does it: outside the hole a copy of the photo, inside a fill 15% lighter.
        // The ring round the hole matches; only the edge shows the drift.
        let (w, h) = (128usize, 64usize);
        let photo = RgbImage::from_pixel(w as u32, h as u32, Rgb([150, 100, 50]));
        let hole: Vec<f32> = (0..w * h).map(|i| if (40..88).contains(&(i % w)) && (16..48).contains(&(i / w)) { 1.0 } else { 0.0 }).collect();
        let mut out: Vec<f32> = (0..3 * w * h)
            .map(|i| {
                let v = [150.0, 100.0, 50.0][i / (w * h)];
                if hole[i % (w * h)] > 0.5 { v * 1.15 } else { v }
            })
            .collect();
        match_surroundings(&photo, &mut out, &hole, w, h);
        let centre = 32 * w + 64;
        for (c, want) in [150.0, 100.0, 50.0].iter().enumerate() {
            assert!((out[c * w * h + centre] - want).abs() < 2.0, "channel {c}: {}", out[c * w * h + centre]);
        }
    }

    #[test]
    fn a_big_hole_is_matched_in_the_middle_too() {
        // A hole covering most of the picture: its middle is beyond the blur's reach.
        let (w, h) = (256usize, 256usize);
        let photo = RgbImage::from_pixel(w as u32, h as u32, Rgb([60, 60, 60]));
        let hole: Vec<f32> = (0..w * h).map(|i| if (12..244).contains(&(i % w)) && (12..244).contains(&(i / w)) { 1.0 } else { 0.0 }).collect();
        let mut out = vec![80f32; 3 * w * h];
        match_surroundings(&photo, &mut out, &hole, w, h);
        let centre = 128 * w + 128;
        assert!((out[centre] - 60.0).abs() < 1.0, "centre: {}", out[centre]);
    }

    #[test]
    fn schedule_matches_diffusers() {
        // diffusers EulerDiscreteScheduler(RORem's config).set_timesteps(24) gives
        // 944, 903, 862 ... 1 with sigmas[1] = 8.4798; strength 0.99 drops the first.
        let (ts, sig) = schedule(24);
        assert_eq!(ts.len(), 23);
        assert_eq!(sig.len(), 24);
        assert_eq!(ts[0], 903.0);
        assert!((sig[0] - 8.4798).abs() < 1e-3, "{}", sig[0]);
        assert_eq!(*ts.last().unwrap(), 1.0);
        assert_eq!(*sig.last().unwrap(), 0.0);
        assert!(sig.windows(2).all(|w| w[0] > w[1]));
    }

    #[test]
    fn work_size_is_multiples_of_64() {
        assert_eq!(work_size(1900, 1000), (1024, 512));
        assert_eq!(work_size(4000, 3000), (1024, 768));
        assert_eq!(work_size(100, 100), (1024, 1024));
    }
}
