//! AI noise reduction: SCUNet (Kai Zhang et al., "Practical Blind Image
//! Denoising via Swin-Conv-UNet and Data Synthesis", Apache-2.0), run with
//! ONNX Runtime. Blind: it judges each photo's noise itself, so a clean photo
//! is left almost as it is and a high-ISO one is cleaned hard. The model file
//! is converted once from the official `scunet_color_real_psnr` weights and
//! never shipped in git: `models/scunet-color-real-psnr.onnx` (a fixed
//! 512 x 512 tile; photos go through in overlapping tiles).
//!
//! It works on the scene image, before exposure and the sliders, so every
//! tool works on the cleaned picture, as with Lightroom's Denoise.
//!
//! Preferred when installed: `denoise-student.onnx`, a small network distilled
//! from SCUNet (`tools/ml/train_student.py --width 16`, same 512 px tiles and
//! encoding; 2.9 M parameters, 12 MB, 49.3 dB vs SCUNet on held-out crops).
//! On Windows it runs with DirectML on whichever graphics card is fastest
//! (each is timed once, on first use; RTX 5070 Ti 0.005 s a tile), on Linux
//! with WebGPU (Vulkan) on the graphics card (0.21 s vs 3.3 s on the CPU for
//! 2048 x 1536, the same result to 0.0001 of an 8-bit level), on Apple Silicon
//! with CoreML on the Mac's graphics (not yet measured), else on the CPU (0.14 s
//! a tile); SCUNet on the CPU if it isn't installed. (A
//! width-32 student scored 50.4 dB but was twice as slow everywhere, and an
//! 8-bit one was slower and worse: neither is used, 16 Oct.)

use crate::pipeline::SceneImage;
use rayon::prelude::*;
use std::sync::{Mutex, OnceLock};

const MODEL_FILE: &str = "scunet-color-real-psnr.onnx";
const STUDENT_FILE: &str = "denoise-student.onnx";
const TILE: usize = 512;
/// Tiles overlap by this much and are blended across it, so no seams show.
const OVERLAP: usize = 64;

/// A session for `path`: on the CPU, or on graphics card `gpu` (Windows:
/// a DirectML device; Linux: WebGPU, which picks the card itself; Apple
/// Silicon: CoreML on the graphics, at full precision: not the Neural Engine,
/// whose 16-bit maths would change the result).
fn build(env: &ort::environment::Environment, path: &std::path::Path, gpu: Option<i32>) -> Option<ort::session::Session> {
    let builder = ort::session::Session::builder(env).ok()?;
    let mut builder = match gpu {
        #[cfg(windows)]
        Some(d) => builder
            .with_parallel_execution(false)
            .ok()?
            .with_memory_pattern(false)
            .ok()?
            .with_execution_providers([ort::ep::DirectML::default().with_device_id(d).build().error_on_failure()])
            .ok()?,
        #[cfg(target_os = "linux")]
        Some(_) => builder.with_execution_providers([ort::ep::WebGPU::default().build().error_on_failure()]).ok()?,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Some(_) => {
            let cache = std::env::temp_dir().join("rapidone-coreml");
            builder
                .with_execution_providers([ort::ep::CoreML::default()
                    .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
                    .with_compute_units(ort::ep::coreml::ComputeUnits::CPUAndGPU)
                    .with_model_cache_dir(cache.display())
                    .build()
                    .error_on_failure()])
                .ok()?
        }
        #[cfg(not(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))))]
        Some(_) => return None,
        None => builder,
    };
    builder.commit_from_file(path).ok()
}

/// The student on the fastest graphics card: each device (DirectML's on
/// Windows; WebGPU's choice on Linux; the Mac's own graphics) is timed on a
/// tile (after a warm-up run) and the quickest kept. None: no GPU works.
#[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
fn fastest_gpu(env: &ort::environment::Environment, path: &std::path::Path) -> Option<ort::session::Session> {
    let mut best: Option<(f32, ort::session::Session)> = None;
    let devices = if cfg!(windows) { 0..4 } else { 0..1 };
    for device in devices {
        let Some(s) = build(env, path, Some(device)) else { continue };
        let s = Mutex::new(s);
        let tile = vec![0.5f32; 3 * TILE * TILE];
        // A first run that isn't timed: it can include compiling the model for the
        // device (CoreML on a Mac: 20 s or more the first time, which put Denoise on
        // the CPU for the whole session, 25 Oct). Then a warm-up run; a device that
        // is clearly slow (an integrated chip can be slower than the CPU) is not
        // timed further.
        if run_tile(&s, tile.clone()).is_none() {
            continue;
        }
        let warm = std::time::Instant::now();
        if run_tile(&s, tile.clone()).is_none() || warm.elapsed().as_secs_f32() > 0.25 {
            continue;
        }
        let t = std::time::Instant::now();
        let ok = (0..3).all(|_| run_tile(&s, tile.clone()).is_some());
        let secs = t.elapsed().as_secs_f32() / 3.0;
        if ok && best.as_ref().is_none_or(|(b, _)| secs < *b) {
            best = Some((secs, s.into_inner().ok()?));
        }
    }
    best.map(|(_, s)| s)
}

fn session() -> Option<&'static Mutex<ort::session::Session>> {
    static SESSION: OnceLock<Option<Mutex<ort::session::Session>>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let env = crate::mask::ort_env()?;
            let student = crate::mask::find_model(STUDENT_FILE);
            // RAPIDONE_DENOISE_CPU=1: as a machine without a graphics card would (testing).
            let cpu_only = std::env::var("RAPIDONE_DENOISE_CPU").is_ok();
            #[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
            if let Some(s) = student.as_ref().filter(|_| !cpu_only).and_then(|p| fastest_gpu(env, p)) {
                return Some(Mutex::new(s));
            }
            if let Some(s) = student.as_ref().and_then(|p| build(env, p, None)) {
                return Some(Mutex::new(s));
            }
            let path = crate::mask::find_model(MODEL_FILE)?;
            Some(Mutex::new(build(env, &path, None)?))
        })
        .as_ref()
}

/// Whether the denoise model is installed.
pub fn available() -> bool {
    session().is_some()
}

/// Tile starts along an edge of `len`: every `TILE - OVERLAP`, the last one
/// flush with the end.
fn starts(len: usize) -> Vec<usize> {
    if len <= TILE {
        return vec![0];
    }
    let step = TILE - OVERLAP;
    let mut v: Vec<usize> = (0..).map(|i| i * step).take_while(|&s| s + TILE < len).collect();
    v.push(len - TILE);
    v
}

/// One 512 x 512 tile through the model (channels first, 0..1, sRGB-like).
fn run_tile(session: &Mutex<ort::session::Session>, input: Vec<f32>) -> Option<Vec<f32>> {
    let tensor = ort::value::Tensor::from_array(([1usize, 3, TILE, TILE], input.into_boxed_slice())).ok()?;
    let mut session = session.lock().ok()?;
    let outputs = session.run(ort::inputs![tensor]).ok()?;
    let (_, out) = outputs[0].try_extract_tensor::<f32>().ok()?;
    (out.len() == 3 * TILE * TILE).then(|| out.to_vec())
}

/// Denoises an image already in the model's terms (0..1, display-encoded),
/// tile by tile; edges are repeated where a tile overhangs the picture.
pub fn denoise_encoded(img: &[[f32; 3]], w: usize, h: usize) -> Option<Vec<[f32; 3]>> {
    let session = session()?;
    let mut acc = vec![[0.0f32; 3]; w * h];
    let mut weight = vec![0.0f32; w * h];
    let (xs, ys) = (starts(w), starts(h));
    for &ty in &ys {
        for &tx in &xs {
            let mut input = vec![0.0f32; 3 * TILE * TILE];
            for y in 0..TILE {
                let sy = (ty + y).min(h - 1);
                for x in 0..TILE {
                    let p = img[sy * w + (tx + x).min(w - 1)];
                    for c in 0..3 {
                        input[c * TILE * TILE + y * TILE + x] = p[c];
                    }
                }
            }
            let out = run_tile(session, input)?;
            // Weight: 1 inside, ramping down across the overlap (but not at
            // the picture's own edges, which no other tile covers).
            let ramp = |i: usize, start: usize, len: usize| -> f32 {
                let lead = if start == 0 { f32::MAX } else { (i as f32 + 0.5) / OVERLAP as f32 };
                let trail = if start + TILE >= len { f32::MAX } else { (TILE as f32 - i as f32 - 0.5) / OVERLAP as f32 };
                lead.min(trail).min(1.0)
            };
            for y in 0..TILE.min(h - ty) {
                let wy = ramp(y, ty, h);
                for x in 0..TILE.min(w - tx) {
                    let wgt = wy * ramp(x, tx, w);
                    let i = (ty + y) * w + tx + x;
                    for c in 0..3 {
                        acc[i][c] += out[c * TILE * TILE + y * TILE + x] * wgt;
                    }
                    weight[i] += wgt;
                }
            }
        }
    }
    Some(acc.into_par_iter().zip(weight.into_par_iter()).map(|(a, wt)| a.map(|v| v / wt.max(1e-6))).collect())
}

/// The scene as the model sees it: brightness scaled so the 99.5th percentile
/// is white, gamma 2.2, clipped to 1 (and that white). Also what its training
/// data is made from (`examples/denoise_data.rs`).
pub fn encode(scene: &SceneImage) -> (Vec<[f32; 3]>, f32) {
    let mut lums: Vec<f32> = scene.px.iter().step_by(7).map(|p| p[0].max(p[1]).max(p[2])).collect();
    lums.sort_by(|a, b| a.total_cmp(b));
    let white = lums.get(lums.len() * 995 / 1000).copied().unwrap_or(1.0).max(1e-4);
    let encoded = scene.px.par_iter().map(|p| p.map(|v| (v / white).max(0.0).powf(1.0 / 2.2).min(1.0))).collect();
    (encoded, white)
}

/// The scene image with its noise taken out, or None if the model isn't
/// installed. The model sees a display-like picture (brightness scaled so the
/// 99.5th percentile is white, then gamma 2.2); only the change it makes is
/// carried back, so highlights above that white are kept as they were.
pub fn scene(scene: &SceneImage) -> Option<SceneImage> {
    let (encoded, white) = encode(scene);
    let enc = |v: f32| (v / white).max(0.0).powf(1.0 / 2.2);
    let cleaned = denoise_encoded(&encoded, scene.width, scene.height)?;
    let px = scene
        .px
        .par_iter()
        .zip(encoded.par_iter().zip(cleaned.par_iter()))
        .map(|(p, (e, d))| std::array::from_fn(|c| white * (enc(p[c]) + d[c] - e[c]).max(0.0).powf(2.2)))
        .collect();
    Some(SceneImage { width: scene.width, height: scene.height, px, source_long: scene.source_long })
}
