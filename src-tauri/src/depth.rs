//! The depth map: Depth Anything V2 Small (Yang et al., Apache-2.0), run with ONNX Runtime.
//! It estimates how far away every part of the photo is, from the developed picture alone, so a
//! mask can act on the near band, the back, or everything before a distance: a depth mask
//! (`gradient::Kind::Depth`). The model file is downloaded, never shipped in git (fetch-models);
//! on Windows it runs with DirectML on the graphics card, on Linux with WebGPU, on Apple Silicon
//! with WebGPU, else on the CPU (`ortgpu`).
//!
//! This is the fallback: when the scene geometry's own models are installed (`scene.rs`, MoGe-2)
//! the depth mask reads its metric control depth instead, and this model only runs without them.
//!
//! The model outputs relative inverse depth (no metres, no calibration): each photo is
//! normalised to its own range, robustly, so 1 is its nearest part and 0 its farthest. Values
//! are therefore comparable within a photo, not between photos.
//!
//! On the GPU that band is what most of the cost is; the output is refined to 2048 on the photo
//! and snapped to its real edges (`refine`), so a band holds up zoomed in, then cached as 16-bit
//! greyscale in the folder's cache (`diskcache::save_depth`).

use crate::pipeline::SceneImage;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Everything a different depth model would change: its file, the picture size it wants (long
/// side and the multiple its patches need), how its input is normalised, and whether its output
/// is inverse depth (larger = nearer, as Depth Anything's) or depth in metres (smaller = nearer).
pub struct DepthModel {
    pub file: &'static str,
    /// The input's long side (the other side keeps the photo's shape).
    pub long_side: usize,
    /// Each side is rounded to a multiple of this (the model's patch size).
    pub patch: usize,
    /// ImageNet's mean and spread, as the model was trained.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Its output is inverse depth: larger = nearer.
    pub inverse: bool,
    /// Its ONNX input and output names.
    pub input: &'static str,
    pub output: &'static str,
}

/// Depth Anything V2 Small: Apache-2.0, from `onnx-community/depth-anything-v2-small`
/// (`onnx/model.onnx`, 99 MB). 518 px and patches of 14 are the model's own preprocessing.
/// Never the Base or Large weights: those are CC-BY-NC.
pub const MODEL: DepthModel = DepthModel {
    file: "depth-anything-v2-small.onnx",
    long_side: 518,
    patch: 14,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    inverse: true,
    input: "pixel_values",
    output: "predicted_depth",
};

/// A depth map over the whole frame, `w` x `h`, 0..1: 1 = nearest, 0 = farthest.
#[derive(Clone, Debug)]
pub struct DepthMap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl DepthMap {
    /// The depth at (x, y), each 0..1 across the frame, bilinear.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let fx = (x * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (y * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * self.w + x];
        let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
        let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// The picture size the model is given for a photo `w` x `h` (its frame: e.g. the developed
/// scene, already upright): the long side at `long_side`, both sides rounded to `patch`
/// multiples, the photo's shape kept (scaled, never padded).
pub fn model_size(w: usize, h: usize, model: &DepthModel) -> (usize, usize) {
    let (w, h) = (w.max(1), h.max(1));
    let scale = model.long_side as f32 / w.max(h) as f32;
    let side = |v: usize| (((v as f32 * scale / model.patch as f32).round() as usize).max(1)) * model.patch;
    (side(w), side(h))
}

/// What the model ran on, for the report and the note (set when its session is built).
static EP: OnceLock<String> = OnceLock::new();

fn session() -> Option<&'static Mutex<ort::session::Session>> {
    static SESSION: OnceLock<Option<Mutex<ort::session::Session>>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let env = crate::mask::ort_env()?;
            let path = crate::mask::find_model(MODEL.file)?;
            // RAPIDONE_DEPTH_CPU=1: as a machine without a graphics card would (testing).
            let cpu_only = std::env::var("RAPIDONE_DEPTH_CPU").is_ok();
            #[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
            if !cpu_only {
                let (w, h) = model_size(MODEL.long_side, MODEL.long_side, &MODEL);
                let probe = vec![0.5f32; 3 * w * h];
                if let Some((s, device)) = crate::ortgpu::fastest_with(env, &path, 0.25, |s| run(s, probe.clone(), w, h).is_some()) {
                    let _ = EP.set(crate::ortgpu::ep_name(device));
                    return Some(Mutex::new(s));
                }
            }
            let _ = EP.set("CPU".to_string());
            Some(Mutex::new(crate::ortgpu::build(env, &path, None)?))
        })
        .as_ref()
}

/// Whether the depth model is installed.
pub fn available() -> bool {
    session().is_some()
}

/// What the model runs on ("CPU", or the graphics card's provider), once it has run or been set up.
pub fn execution_provider() -> &'static str {
    let _ = session();
    EP.get().map_or("CPU", |s| s.as_str())
}

/// Where the model file is (developer tools).
pub fn model_path() -> Option<PathBuf> {
    crate::mask::find_model(MODEL.file)
}

/// One run of the model: a `w` x `h` picture, channels first, normalised, returns the model's
/// output's width, height and values (not yet normalised to 0..1).
fn run(session: &Mutex<ort::session::Session>, input: Vec<f32>, w: usize, h: usize) -> Option<(usize, usize, Vec<f32>)> {
    let tensor = ort::value::Tensor::from_array(([1usize, 3, h, w], input.into_boxed_slice())).ok()?;
    let mut session = session.lock().ok()?;
    let outputs = session.run(ort::inputs![MODEL.input => tensor]).ok()?;
    let (shape, out) = outputs[0].try_extract_tensor::<f32>().ok()?;
    // [1, h, w], or [1, 1, h, w]: the last two dims are the picture.
    let dims: Vec<usize> = shape.iter().map(|&d| d.max(0) as usize).collect();
    let (ow, oh) = (dims.get(dims.len() - 1).copied()?, dims.get(dims.len() - 2).copied()?);
    (out.len() == ow * oh).then(|| (ow, oh, out.to_vec()))
}

/// The picture's white point as the models see it: brightness scaled so its 99th percentile is
/// near white (then gamma, in each model's own input).
pub fn white_point(image: &SceneImage) -> f32 {
    let mut lums: Vec<f32> = image.px.iter().step_by(7).map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).collect();
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    lums.get(lums.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-4)
}

/// The photo's depth map: the model on the developed scene, then each photo normalised to its own
/// range (0.5th to 99.5th percentile, so one stray pixel can't set it) with 1 the nearest part.
pub fn depth(scene: &SceneImage) -> Option<DepthMap> {
    let session = session()?;
    let (w, h) = model_size(scene.width, scene.height, &MODEL);
    let white = white_point(scene);

    // Stretched to the model's size (nearest is plenty at 518), ImageNet-normalised, channels first.
    let mut input = vec![0.0f32; 3 * w * h];
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / h as f32 * scene.height as f32) as usize;
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / w as f32 * scene.width as f32) as usize;
            let p = scene.px[sy.min(scene.height - 1) * scene.width + sx.min(scene.width - 1)];
            for c in 0..3 {
                let v = (p[c] / white).clamp(0.0, 1.0).powf(1.0 / 2.2);
                input[c * w * h + y * w + x] = (v - MODEL.mean[c]) / MODEL.std[c];
            }
        }
    }

    let (ow, oh, raw) = run(session, input, w, h)?;
    Some(DepthMap { w: ow, h: oh, data: normalised(&raw, MODEL.inverse) })
}

/// The model's **own** values over a window of the picture — `(x0, y0, w, h)` in its own pixels —
/// resampled to a `w` x `h` grid and not normalised to the photo's range: for the tiled pass
/// (`scene::detail`), where each window is fitted to the global map over it. Larger = nearer.
pub fn relative_rect(image: &SceneImage, rect: (usize, usize, usize, usize), w: usize, h: usize, white: f32) -> Option<crate::gradient::Grid> {
    let session = session()?;
    let (x0, y0, rw, rh) = rect;
    let (x1, y1) = ((x0 + rw).min(image.width), (y0 + rh).min(image.height));
    let (rw, rh) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    let (mw, mh) = model_size(rw, rh, &MODEL);
    let mut input = vec![0.0f32; 3 * mw * mh];
    for y in 0..mh {
        let sy = (y0 + ((y as f32 + 0.5) / mh as f32 * rh as f32) as usize).min(image.height - 1);
        for x in 0..mw {
            let sx = (x0 + ((x as f32 + 0.5) / mw as f32 * rw as f32) as usize).min(image.width - 1);
            let p = image.px[sy * image.width + sx];
            for c in 0..3 {
                let v = (p[c] / white.max(1e-4)).clamp(0.0, 1.0).powf(1.0 / 2.2);
                input[c * mw * mh + y * mw + x] = (v - MODEL.mean[c]) / MODEL.std[c];
            }
        }
    }
    let (ow, oh, raw) = run(session, input, mw, mh)?;
    let grid = crate::gradient::Grid {
        w: ow,
        h: oh,
        data: raw.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect(),
    };
    Some(crate::hq::resample(&grid, w, h))
}

/// The model's output as 0..1 with 1 = nearest: its 0.5th and 99.5th percentiles as the ends
/// (robust against a clipped or noisy pixel), anything outside clamped.
fn normalised(raw: &[f32], inverse: bool) -> Vec<f32> {
    let mut sorted: Vec<f32> = raw.iter().step_by(7).copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    if sorted.is_empty() {
        return vec![0.5; raw.len()];
    }
    let pick = |q: usize| sorted[(sorted.len() - 1) * q / 1000];
    let (lo, hi) = (pick(5), pick(995));
    let span = (hi - lo).max(1e-6);
    raw.iter()
        .map(|&v| {
            let t = ((v - lo) / span).clamp(0.0, 1.0);
            if inverse { t } else { 1.0 - t }
        })
        .collect()
}

/// The long side of a refined mask (as the subject mask's).
const REFINED_LONG: usize = 2048;

/// The depth map refined against the photo: scaled up to REFINED_LONG on its long side, snapped to
/// the photo's own edges with a guided filter on its lightness (`tools::guided_filter`, as the
/// subject mask is), then clamped to the unrefined map's local range so the filter can't invent a
/// bright or dark halo at an edge (a near-pole with a far sky behind it).
pub fn refine(map: &DepthMap, image: &SceneImage) -> crate::gradient::Grid {
    let grid = crate::gradient::Grid { w: map.w, h: map.h, data: map.data.clone() };
    refine_map(&grid, image)
}

/// A control-depth grid refined as `refine` does (the scene's own map is a grid already).
pub fn refine_map(map: &crate::gradient::Grid, image: &SceneImage) -> crate::gradient::Grid {
    let small = image.downscaled(REFINED_LONG);
    let (w, h) = (small.width, small.height);
    let n = w * h;
    // The guide: perceptual lightness (cube root of luminance), 0..1-ish.
    let peak = small.px.iter().map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).fold(1e-4f32, f32::max);
    let guide: Vec<f32> = small.px.iter().map(|p| ((0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0) / peak).cbrt()).collect();
    let rough: Vec<f32> = (0..n)
        .map(|i| map.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32))
        .collect();
    // Radius: about one model pixel's worth either side, a little more; eps: how strong an edge
    // in the photo has to be to be followed.
    let r = (w.max(h) / 256).max(2);
    let filtered = crate::tools::guided_filter(&guide, &rough, w, h, r, 1e-3);
    // The unrefined map's local range: the filter may only move a depth within what its
    // neighbourhood already held, so no halo appears either side of an edge.
    let negated: Vec<f32> = rough.iter().map(|v| -v).collect();
    let lo = crate::tools::min_filter(&rough, w, h, r);
    let hi: Vec<f32> = crate::tools::min_filter(&negated, w, h, r).into_iter().map(|v| -v).collect();
    let data = filtered.iter().zip(lo.iter().zip(hi.iter())).map(|(&v, (&lo, &hi))| v.clamp(lo, hi).clamp(0.0, 1.0)).collect();
    crate::gradient::Grid { w, h, data }
}

/// A depth map as a 16-bit greyscale PNG (the cache, `diskcache::save_depth`).
pub fn to_png16(map: &DepthMap) -> Result<Vec<u8>, String> {
    let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_fn(map.w as u32, map.h as u32, |x, y| {
        image::Luma([(map.data[(y * map.w as u32 + x) as usize].clamp(0.0, 1.0) * 65535.0).round() as u16])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

/// A depth map from `to_png16`.
pub fn from_png16(png: &[u8]) -> Option<DepthMap> {
    let img = image::load_from_memory(png).ok()?.to_luma16();
    Some(DepthMap { w: img.width() as usize, h: img.height() as usize, data: img.pixels().map(|p| p[0] as f32 / 65535.0).collect() })
}

/// The depth at (x, y), fractions of the photo, as the mean over a 5 x 5 neighbourhood: one
/// pixel's noise shouldn't set the band when the photo is clicked.
pub fn at_average(grid: &crate::gradient::Grid, x: f32, y: f32) -> f32 {
    let (dx, dy) = (1.0 / grid.w.max(1) as f32, 1.0 / grid.h.max(1) as f32);
    let mut sum = 0.0;
    for j in -2..=2 {
        for i in -2..=2 {
            sum += grid.at((x + i as f32 * dx).clamp(0.0, 1.0), (y + j as f32 * dy).clamp(0.0, 1.0));
        }
    }
    sum / 25.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_sees_the_photos_shape_rounded_to_its_patches() {
        assert_eq!(model_size(6000, 4000, &MODEL), (518, 350));
        // A tall photo: the height takes the long side.
        assert_eq!(model_size(4000, 6000, &MODEL), (350, 518));
        // Both sides a multiple of 14, and a panorama still gets a side.
        for (w, h) in [(1, 1), (10000, 500), (3, 4000)] {
            let (mw, mh) = model_size(w, h, &MODEL);
            assert_eq!(mw % MODEL.patch, 0);
            assert_eq!(mh % MODEL.patch, 0);
        }
    }

    #[test]
    fn depth_is_normalised_to_its_own_range_with_1_nearest() {
        let raw = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0];
        let near = normalised(&raw, true);
        assert!((near[5] - 1.0).abs() < 1e-6 && near[0].abs() < 1e-6, "inverse: the largest is nearest");
        let far = normalised(&raw, false);
        assert!((far[0] - 1.0).abs() < 1e-6 && far[5].abs() < 1e-6, "depth: the smallest is nearest");
        // One absurd pixel doesn't set the ends (the 0.5th and 99.5th percentiles do).
        let mut with_outlier = vec![0.5f32; 1000];
        with_outlier[7] = 1e9;
        let n = normalised(&with_outlier, true);
        assert!(n.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn a_16_bit_png_round_trips() {
        let map = DepthMap { w: 3, h: 2, data: vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.333] };
        let back = from_png16(&to_png16(&map).unwrap()).unwrap();
        assert_eq!((back.w, back.h), (3, 2));
        for (a, b) in map.data.iter().zip(&back.data) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn depth_at_averages_its_neighbourhood() {
        let grid = crate::gradient::Grid { w: 100, h: 100, data: vec![0.2; 10000] };
        assert!((at_average(&grid, 0.5, 0.5) - 0.2).abs() < 1e-4);
        // A single bright pixel doesn't move it much (the 5 x 5 mean): one of 25 is 1.0, so +0.032.
        let mut spotty = grid;
        spotty.data[50 * 100 + 50] = 1.0;
        let d = at_average(&spotty, 0.505, 0.505);
        assert!(d > 0.2 && d < 0.24, "{d}");
    }
}
