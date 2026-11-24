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
use std::sync::{Arc, Mutex};

const MODEL_FILE: &str = "scunet-color-real-psnr.onnx";
const STUDENT_FILE: &str = "denoise-student.onnx";
const TILE: usize = 512;
/// Tiles overlap by this much and are blended across it, so no seams show.
const OVERLAP: usize = 64;

/// The noise model's session, built on first use and kept until `release` (then built again
/// when next needed). Released when GPU (Smarter) unloads: while any WebGPU session lives, ONNX
/// Runtime keeps its context, and with it the buffer cache GPU (Smarter) filled (~9 GB stayed on
/// the card after GPU OFF, 21 Nov).
static SESSION: Mutex<Option<Arc<Mutex<ort::session::Session>>>> = Mutex::new(None);
/// The model isn't installed (or wouldn't load): not tried again.
static MISSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn session() -> Option<Arc<Mutex<ort::session::Session>>> {
    if MISSING.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = slot.as_ref() {
        return Some(s.clone());
    }
    let built = (|| {
        let env = crate::mask::ort_env()?;
        let student = crate::mask::find_model(STUDENT_FILE);
        // RAPIDONE_DENOISE_CPU=1: as a machine without a graphics card would (testing).
        let cpu_only = std::env::var("RAPIDONE_DENOISE_CPU").is_ok();
        #[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
        if let Some(s) = student.as_ref().filter(|_| !cpu_only).and_then(|p| {
            let tile = vec![0.5f32; 3 * TILE * TILE];
            crate::ortgpu::fastest(env, p, |s| run_tile(s, tile.clone()).is_some()).map(|(s, _)| s)
        }) {
            return Some(s);
        }
        if let Some(s) = student.as_ref().and_then(|p| crate::ortgpu::build(env, p, None)) {
            return Some(s);
        }
        let path = crate::mask::find_model(MODEL_FILE)?;
        crate::ortgpu::build(env, &path, None)
    })();
    match built {
        Some(s) => {
            let s = Arc::new(Mutex::new(s));
            *slot = Some(s.clone());
            Some(s)
        }
        None => {
            MISSING.store(true, std::sync::atomic::Ordering::Relaxed);
            None
        }
    }
}

/// Drops the noise model's session (a denoise in progress keeps its own until done); the next
/// denoise builds it again (~0.3 s).
pub fn release() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
            let out = run_tile(&session, input)?;
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
/// `scene`, with the last results kept by content: a zoomed-in part is a new Scene each render but
/// usually the same pixels while a slider moves, and the model takes a second or more on it (1 Nov).
/// No lock is held while the model runs (as `colournoise::for_scene`).
pub fn kept(img: &SceneImage) -> Option<std::sync::Arc<SceneImage>> {
    type Recent = Vec<(u64, std::sync::Arc<SceneImage>)>;
    static RECENT: Mutex<Recent> = Mutex::new(Vec::new());
    const KEPT: usize = 2;
    // Bigger than this (a whole photo for export) isn't held here as well as on its Scene.
    const MAX_PIXELS: usize = 16_000_000;
    if img.px.len() > MAX_PIXELS {
        return scene(img).map(std::sync::Arc::new);
    }
    let key = crate::defringe::fingerprint(img);
    if let Some((_, out)) = RECENT.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Some(out.clone());
    }
    let out = std::sync::Arc::new(scene(img)?);
    let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    if !recent.iter().any(|(k, _)| *k == key) {
        recent.push((key, out.clone()));
        if recent.len() > KEPT {
            recent.remove(0);
        }
    }
    Some(out)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Released (GPU (Smarter) unloading) and used again: rebuilt by itself, same result, and a
    /// denoise in progress keeps its own session (the batch export case). Skipped without the model.
    #[test]
    fn release_then_denoise_again() {
        if !available() {
            eprintln!("denoise model not installed: skipped");
            return;
        }
        let (w, h) = (300usize, 200usize);
        let img: Vec<[f32; 3]> = (0..w * h).map(|i| {
            let v = ((i * 7919) % 97) as f32 / 97.0 * 0.2 + 0.4;
            [v, v * 0.9, v * 1.1]
        }).collect();
        let a = denoise_encoded(&img, w, h).expect("first");
        // A session held across a release (a denoise in progress) still runs.
        let held = session().expect("held");
        release();
        let mut input = vec![0.0f32; 3 * TILE * TILE];
        input.iter_mut().enumerate().for_each(|(i, v)| *v = (i % 13) as f32 / 13.0);
        assert!(run_tile(&held, input).is_some());
        drop(held);
        let b = denoise_encoded(&img, w, h).expect("after release: rebuilt by itself");
        let diff = a.iter().zip(&b).map(|(p, q)| (0..3).map(|c| (p[c] - q[c]).abs()).fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
        assert!(diff < 1e-3, "same result after rebuilding: max diff {diff}");
    }
}
