//! The photo's scene geometry: a real 3D scene from one MoGe-2 pass (Wang et al., Microsoft,
//! MIT; the ONNX exports are `Ruicheng/moge-2-vits-normal-onnx` and `...-vitb-normal-onnx`).
//! Where `depth.rs` gives a relative map (each photo's nearest to its farthest), MoGe gives
//! per-pixel **metric** points in metres, **surface normals**, a **sky / validity mask** and the
//! camera's **field of view** all at once. That is the base the Creative tools need: Bokeh's
//! thin-lens CoC works in metres, Mist's transmittance in metres, Relight on the normals, and the
//! Depth Scope draws the point cloud.
//!
//! One pass writes Four things, all at the model's own size (see `model_size`):
//! - `distance`: metres from the camera along the optical axis (`z`), `+inf` for sky. `z` rather
//!   than `|p|`: a lens's blur (and depth of field) depends on the axial distance, 1/s' = 1/f - 1/s,
//!   so `z` is what Bokeh and Mist need; MoGe's own depth output is `z` too, and at the frame's
//!   corners `|p|` would read up to 1.4x too far.
//! - `control`: the same distance on a **log scale between robust anchors** (the 0.5% and 99.5%
//!   quantiles of the valid distances), 0..1 with 1 = nearest and 0 = farthest (phase 1's
//!   convention), sky forced to 0. Every depth slider works on this, so a band's edges move evenly
//!   in perceived distance and a stray pixel can't set either end.
//! - `normals`: unit vectors in camera space (x right, y down, z forward, as MoGe's points).
//! - `sky`: 1 where MoGe says there is no surface (sky, infinity), 0 on solid geometry.
//!
//! The affine point map MoGe predicts is only defined up to a focal length and a shift along z;
//! its post-processing recovers both from the point map itself (`recover_focal_shift`, ported from
//! MoGe (MIT)), and that least-squares solve is what turns the map into camera space and its
//! focal into a field of view. It is written out here (LM on the 64 x 64 downsample), not run in
//! Python.
//!
//! ViT-S runs on the CPU (1800 tokens); ViT-B on a graphics card (2500 tokens), which is the
//! sharper of the two. Depth Anything V2 Small stays the fallback when neither MoGe file is
//! installed or they fail (`depth.rs`).
//!
//! Phase 1c adds three things on top, all optional:
//! - **High quality** (`hq.rs`: Depth Anything 3 Mono-Large, an on-demand download): its relative
//!   depth fitted to MoGe's metric distances (`fusion.rs`, robust Huber IRLS) replaces the
//!   distance map; normals and the sky stay MoGe's.
//! - **Tiled detail** (both qualities): overlapping windows at the relative model's own input size,
//!   each fitted to the global map and blended in, so edges keep the detail a whole-frame pass
//!   loses. The map is `detail`, at the refined (2048) size.
//! - **Halo-free edges** (`edges.rs`): the two surfaces at a depth jump and their coverage, made
//!   once per photo from the refined map and kept in the cache with it.
//!
//! Everything is cached in the folder (`diskcache::save_scene`, version 2), and the control map
//! also as the 16-bit `<photo>.depth.png` the depth masks already read.

use crate::edges::EdgeMaps;
use crate::gradient::Grid;
use crate::pipeline::SceneImage;
use std::sync::{Mutex, OnceLock};

/// MoGe-2 ViT-S: the CPU model, 1800 tokens (its suggested range is 1200..2500).
const VITS: &str = "moge-2-vits-normal.onnx";
/// MoGe-2 ViT-B: the graphics card's, 2500 tokens for the sharper edges it can afford.
const VITB: &str = "moge-2-vitb-normal.onnx";
const TOKENS_VITS: i64 = 1800;
const TOKENS_VITB: i64 = 2500;
/// The models' patch size: both sides of the picture are multiples of it.
const PATCH: usize = 14;

/// Which model ran, on what, for notes and `scene_info`.
static EP: OnceLock<String> = OnceLock::new();

/// The Depth quality setting (the app's, persisted in its own settings): Standard is MoGe-2 alone;
/// High adds Depth Anything 3 Mono-Large's detail, fitted to MoGe's metric distance, when that
/// model has been downloaded (`hq.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Standard,
    High,
}

static QUALITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The Depth quality setting now (Standard until the page says otherwise).
pub fn quality() -> Quality {
    match QUALITY.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Quality::High,
        _ => Quality::Standard,
    }
}

/// Sets the Depth quality setting (from the page; the engine reads it when it makes a scene).
pub fn set_quality(q: Quality) {
    QUALITY.store(if q == Quality::High { 1 } else { 0 }, std::sync::atomic::Ordering::Relaxed);
}

/// The control-depth value for a distance in metres, on the same log scale between the scene's
/// anchors (the shared formula: `Scene3D::control_of`, the High quality fusion and the tiled pass).
pub fn control_of(metres: f32, d_near: f32, d_far: f32) -> f32 {
    if !metres.is_finite() || metres <= 0.0 || !(d_near > 0.0 && d_far > 0.0) {
        return 0.0;
    }
    let denom = (d_far / d_near).ln();
    if denom.abs() < 1e-9 {
        return 0.5;
    }
    1.0 - ((metres / d_near).ln() / denom).clamp(0.0, 1.0)
}

/// The metres a control-depth value stands for (the shared formula: `Scene3D::metres`).
pub fn metres_of(control: f32, d_near: f32, d_far: f32) -> f32 {
    if !(d_near > 0.0 && d_far > 0.0) || (d_far - d_near).abs() < 1e-9 {
        return d_near.max(0.0);
    }
    d_near * (d_far / d_near).powf(1.0 - control.clamp(0.0, 1.0))
}

/// A MoGe-2 model's session: its file, how many tokens to ask it for, and its name. The app
/// builds one per run (`choice`); `scene_check` builds them itself to time both.
pub struct SceneModel {
    session: Mutex<ort::session::Session>,
    tokens: i64,
    pub model: &'static str,
    /// What it runs on, as its provider's name ("CPU", "WebGPU (Vulkan)", ...).
    pub ep: String,
}

impl SceneModel {
    /// A named model ("moge-2-vits-normal.onnx" or "moge-2-vitb-normal.onnx"), on a graphics card
    /// (`gpu`) or the CPU. ViT-B gets 2500 tokens, ViT-S 1800.
    pub fn build(file: &str, gpu: bool) -> Option<SceneModel> {
        let env = crate::mask::ort_env()?;
        let path = crate::mask::find_model(file)?;
        let (tokens, model) = if file == VITB { (TOKENS_VITB, "MoGe-2 ViT-B") } else { (TOKENS_VITS, "MoGe-2 ViT-S") };
        let (session, ep) = match gpu {
            true => (crate::ortgpu::build(env, &path, Some(crate::ortgpu::DEPTH_DEVICE as i32))?, crate::ortgpu::ep_name(crate::ortgpu::DEPTH_DEVICE)),
            false => (crate::ortgpu::build(env, &path, None)?, "CPU".to_string()),
        };
        Some(SceneModel { session: Mutex::new(session), tokens, model, ep })
    }

    /// One pass over the developed picture.
    pub fn run(&self, image: &SceneImage) -> Option<Scene3D> {
        let (w, h) = model_size(image.width, image.height, self.tokens as usize);
        let raw = run_with(&self.session, &(input_image(image, w, h), w, h, self.tokens))?;
        Some(Scene3D::from_raw(raw, self.model))
    }
}

/// The session, built once: ViT-B on a graphics card, else ViT-S on the CPU (or without MoGe at
/// all: `None`, and the depth map falls back to Depth Anything).
fn choice() -> Option<&'static SceneModel> {
    static CHOICE: OnceLock<Option<SceneModel>> = OnceLock::new();
    CHOICE
        .get_or_init(|| {
            let env = crate::mask::ort_env()?;
            // RAPIDONE_SCENE_CPU=1 (or the depth map's own switch): as a machine without a
            // graphics card would (testing).
            let cpu_only = std::env::var("RAPIDONE_SCENE_CPU").is_ok() || std::env::var("RAPIDONE_DEPTH_CPU").is_ok();
            #[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
            if !cpu_only {
                if let Some(path) = crate::mask::find_model(VITB) {
                    // Probed at the shape a 3:2 photo gets, which is what most runs are.
                    let (w, h) = model_size(3, 2, TOKENS_VITB as usize);
                    let probe = || run_args(w, h, TOKENS_VITB);
                    // A generous warm-up limit: ViT-B is a big model, and this is only here to
                    // drop a card that is hopeless rather than merely slow (the CPU fallback is
                    // the small model, which a slow card would beat anyway).
                    if let Some((session, device)) = crate::ortgpu::fastest_with(env, &path, 2.0, |s| run_with(s, &probe()).is_some()) {
                        let ep = crate::ortgpu::ep_name(device);
                        let _ = EP.set(ep.clone());
                        return Some(SceneModel { session: Mutex::new(session), tokens: TOKENS_VITB, model: "MoGe-2 ViT-B", ep });
                    }
                }
            }
            let path = crate::mask::find_model(VITS)?;
            let _ = EP.set("CPU".to_string());
            Some(SceneModel { session: Mutex::new(crate::ortgpu::build(env, &path, None)?), tokens: TOKENS_VITS, model: "MoGe-2 ViT-S", ep: "CPU".to_string() })
        })
        .as_ref()
}

/// Whether either MoGe model is installed (and runs: the session is built and kept).
pub fn available() -> bool {
    choice().is_some()
}

/// What the scene model runs on ("CPU", or the graphics card's provider), once set up.
pub fn execution_provider() -> &'static str {
    let _ = choice();
    EP.get().map_or("CPU", |s| s.as_str())
}

/// The model that ran ("MoGe-2 ViT-S", or "MoGe-2 ViT-B").
pub fn model_name() -> &'static str {
    choice().map_or("none", |c| c.model)
}

/// The picture size the model is given for a photo `w` x `h` with `tokens` base tokens: the
/// photo's shape kept (scaled, never padded), both sides a multiple of PATCH, long side so
/// (H / 14) x (W / 14) is about `tokens`.
pub fn model_size(w: usize, h: usize, tokens: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let aspect = w / h;
    let side = |v: f32| (((v / PATCH as f32).round() as usize).max(1)) * PATCH;
    // tokens = W * H / 14^2, with W = aspect * H: H = 14 sqrt(tokens / aspect).
    let (mw, mh) = (side(WIDTH_FACTOR * (tokens as f32 * aspect).sqrt()), side(WIDTH_FACTOR * (tokens as f32 / aspect).sqrt()));
    (mw.max(PATCH), mh.max(PATCH))
}
/// 14: the long side is 14 sqrt(tokens x aspect) (see `model_size`).
const WIDTH_FACTOR: f32 = PATCH as f32;

/// The photo as the model sees it: RGB 0..1, no mean or spread (MoGe takes sRGB-like 0..1
/// directly), channels first, at `w` x `h`.
fn input_image(scene: &SceneImage, w: usize, h: usize) -> Vec<f32> {
    // A rough display rendering, as the subject and depth models see it: brightness scaled so
    // the 99th percentile is near white, then gamma.
    let mut lums: Vec<f32> = scene.px.iter().step_by(7).map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).collect();
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let white = lums.get(lums.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-4);
    let mut input = vec![0.0f32; 3 * w * h];
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / h as f32 * scene.height as f32) as usize;
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / w as f32 * scene.width as f32) as usize;
            let p = scene.px[sy.min(scene.height - 1) * scene.width + sx.min(scene.width - 1)];
            for c in 0..3 {
                input[c * w * h + y * w + x] = (p[c] / white).clamp(0.0, 1.0).powf(1.0 / 2.2);
            }
        }
    }
    input
}

/// A zero picture at the model's size, for the graphics card probe (its result is thrown away).
fn run_args(w: usize, h: usize, tokens: i64) -> (Vec<f32>, usize, usize, i64) {
    (vec![0.5f32; 3 * w * h], w, h, tokens)
}

/// MoGe's four outputs, at the size it ran on.
struct Raw {
    w: usize,
    h: usize,
    points: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    /// Sky / validity probability: above 0.5 is real geometry.
    mask: Vec<f32>,
    /// The scene's overall scale, to turn MoGe's units into metres.
    metric_scale: f32,
}

/// One run: the picture, the token count, and the model's outputs.
fn run_with(session: &Mutex<ort::session::Session>, args: &(Vec<f32>, usize, usize, i64)) -> Option<Raw> {
    let (input, w, h, tokens) = args;
    let image = ort::value::Tensor::from_array(([1usize, 3, *h, *w], input.clone().into_boxed_slice())).ok()?;
    // A scalar int64, as the model declares `num_tokens`.
    let tokens = ort::value::Tensor::from_array((Vec::<usize>::new(), vec![*tokens].into_boxed_slice())).ok()?;
    let mut session = session.lock().ok()?;
    let outputs = session.run(ort::inputs!["image" => image, "num_tokens" => tokens]).ok()?;
    let (_ps, points) = outputs.get("points")?.try_extract_tensor::<f32>().ok()?;
    let (_ns, normal) = outputs.get("normal")?.try_extract_tensor::<f32>().ok()?;
    let (_ms, mask) = outputs.get("mask")?.try_extract_tensor::<f32>().ok()?;
    // ViT-S names its scale output `scale`, ViT-B `metric_scale`.
    let (_, scale) = outputs.get("metric_scale").or_else(|| outputs.get("scale"))?.try_extract_tensor::<f32>().ok()?;
    let n = w * h;
    (points.len() == n * 3 && normal.len() == n * 3 && mask.len() == n && scale.len() == 1).then(|| Raw {
        w: *w,
        h: *h,
        points: points.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect(),
        normal: normal.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect(),
        mask: mask.to_vec(),
        metric_scale: scale[0],
    })
}

/// Runs MoGe on the developed scene: the scene's geometry, or None without the models. With the
/// High quality setting and its model installed, Depth Anything 3's relative depth is then fitted
/// to MoGe's metric distances and replaces them (`hq.rs`, `fusion.rs`), so the scene is as sharp as
/// the big model is; normals, sky and the field of view stay MoGe's.
pub fn scene(image: &SceneImage) -> Option<Scene3D> {
    let mut s = choice()?.run(image)?;
    if quality() == Quality::High && crate::hq::available() {
        let t = std::time::Instant::now();
        match fuse_high_quality(&mut s, image) {
            Some(fit) => eprintln!(
                "[hq] {} on {}: fitted to MoGe in {} (median error {:.1}%): {} × {:.6} + {:.4}",
                crate::hq::NAME,
                crate::hq::execution_provider(),
                format_args!("{:.2} s", t.elapsed().as_secs_f32()),
                fit.error * 100.0,
                fit.space.name(),
                fit.a,
                fit.b
            ),
            None => eprintln!("[hq] {} did not fit MoGe's distances: the scene keeps MoGe's own depth", crate::hq::NAME),
        }
    }
    Some(s)
}

/// The High quality pass: Depth Anything 3's inverse disparity over the whole picture, fitted to
/// MoGe's metric distances, replaces the distance map (and the control map rebuilt from it). The
/// sky is MoGe's, plus anything the big model calls sky (its own sky head is folded into its
/// output as a disparity of 0). Normals stay MoGe's: the big model gives none.
pub fn fuse_high_quality(scene: &mut Scene3D, image: &SceneImage) -> Option<crate::fusion::Fit> {
    let white = crate::depth::white_point(image);
    // At a size that follows MoGe's grid, so every pixel lines up with its normals and sky.
    let disparity = crate::hq::inverse_depth_white(image, (0, 0, image.width, image.height), scene.w, scene.h, white)?;
    // The model's own relative *distance* (it gives inverse depth, `1/(depth + 0.2)`; the fit's
    // shift takes up the 0.2). Pairs only where both see a surface: the sky's disparity is 0, and
    // MoGe's sky is excluded too.
    let mut model = Vec::with_capacity(disparity.data.len());
    let mut sky = scene.sky.data.clone();
    for i in 0..sky.len() {
        let d = disparity.data[i];
        if d.is_finite() && d > 1e-3 {
            model.push(1.0 / d);
        } else {
            model.push(0.0);
            sky[i] = 1.0;
        }
    }
    let pairs = crate::fusion::pairs_from(&scene.distance, &model, &sky);
    let fit = crate::fusion::fit_best(&pairs)?;
    // A fit that misses the median pixel by more than a third (a factor of 1.4) is not a fit to
    // MoGe's distances: the High quality pass is dropped and the scene keeps MoGe's own depth.
    if !fit.log_error.is_finite() || fit.log_error > 0.33 {
        return None;
    }
    // The fitted distance, with a sane range so one odd pixel can't blow up the cloud.
    let (lo, hi) = (scene.d_near * 0.25, scene.d_far * 4.0);
    for i in 0..scene.distance.len() {
        if sky[i] >= 0.5 {
            scene.distance[i] = f32::INFINITY;
            continue;
        }
        let z = fit.metres(model[i]);
        scene.distance[i] = if z.is_finite() && z > 0.0 { z.clamp(lo, hi) } else { f32::INFINITY };
    }
    scene.sky = Grid { w: scene.w, h: scene.h, data: sky };
    // The control map, from the same anchors, so every slider reads the same scale as before.
    let (d_near, d_far) = (scene.d_near, scene.d_far);
    scene.control = Grid {
        w: scene.w,
        h: scene.h,
        data: scene.distance.iter().map(|&d| control_of(d, d_near, d_far)).collect(),
    };
    scene.model = match scene.model {
        "MoGe-2 ViT-S" => "MoGe-2 ViT-S + DA3 Mono-Large",
        _ => "MoGe-2 ViT-B + DA3 Mono-Large",
    };
    Some(fit)
}

/// A scene: every pixel's distance, its control-depth value, its surface normal and whether it's
/// sky, plus the camera's focal length and field of view.
#[derive(Debug)]
pub struct Scene3D {
    pub w: usize,
    pub h: usize,
    /// Metres from the camera along the optical axis, row-major; `+inf` where there is no surface.
    pub distance: Vec<f32>,
    /// 0..1, 1 = nearest, 0 = farthest: log distance between `d_near` and `d_far`, sky at 0.
    pub control: Grid,
    /// Unit normal in camera space (x right, y down, z forward); [0, 0, 1] on sky. Shared with
    /// every `Refined` made from this scene, so Relight never copies the map.
    pub normals: std::sync::Arc<Vec<[f32; 3]>>,
    /// 0..1: 1 where MoGe says there is no surface (sky, infinity).
    pub sky: Grid,
    /// MoGe's horizontal field of view, degrees.
    pub fov_x_deg: f32,
    /// The recovered focal length in pixels of this grid (x and y are the same).
    pub focal_px: f32,
    /// The robust anchors the control scale runs between, metres.
    pub d_near: f32,
    pub d_far: f32,
    pub model: &'static str,
    /// The tiled pass's control map (phase 1c §3) at the refined size (2048 on the long side): the
    /// global layout with real detail from overlapping windows at the model's own input size.
    /// None: no tiled pass ran (a small picture, no budget, or a scene from before phase 1c).
    pub detail: Option<std::sync::Arc<Grid>>,
    /// The two surfaces at the depth map's edges (`EdgeMaps`, at the refined size), made once by
    /// `edges` and kept in the cache with the scene.
    pub edges: std::sync::OnceLock<Option<std::sync::Arc<EdgeMaps>>>,
}

impl Scene3D {
    /// The geometry from one MoGe run (MoGe's own post-processing, ported).
    fn from_raw(raw: Raw, model: &'static str) -> Scene3D {
        let Raw { w, h, points, normal, mask, metric_scale } = raw;
        let n = w * h;
        let valid: Vec<bool> = mask.iter().map(|&m| m > 0.5).collect();
        let (focal, shift) = recover_focal_shift(&points, w, h, &valid);
        let diagonal = ((w * w + h * h) as f32).sqrt();
        // MoGe's focal is measured against half the frame's diagonal: in pixels that is
        // 0.5 x focal x diagonal, the same on both sides (square pixels).
        let focal_px = (0.5 * focal * diagonal).max(1e-3);
        let fov_x_deg = 2.0 * ((w as f32 * 0.5) / focal_px).atan().to_degrees();
        let scale = if metric_scale.is_finite() && metric_scale > 0.0 { metric_scale } else { 1.0 };

        let mut distance = vec![f32::INFINITY; n];
        let mut normals = vec![[0.0f32, 0.0, 1.0]; n];
        let mut sky = vec![0.0f32; n];
        for i in 0..n {
            // MoGe: the point map is camera space once z is shifted, and only where z is in front.
            let z = points[i][2] + shift;
            sky[i] = 1.0 - mask[i].clamp(0.0, 1.0);
            if !valid[i] || !(z > 1e-6) || !z.is_finite() {
                continue;
            }
            distance[i] = z * scale;
            let len = (normal[i][0] * normal[i][0] + normal[i][1] * normal[i][1] + normal[i][2] * normal[i][2]).sqrt();
            if len > 1e-6 {
                normals[i] = [normal[i][0] / len, normal[i][1] / len, normal[i][2] / len];
            }
        }

        // Robust anchors: the 0.5% and 99.5% quantiles of what the model says is real geometry,
        // so neither a clipped pixel nor a distant speck sets the ends.
        let (d_near, d_far) = anchors(&distance);
        let denom = (d_far / d_near).ln();
        let control = Grid {
            w,
            h,
            data: distance
                .iter()
                .map(|&d| if d.is_finite() && denom.abs() > 1e-6 { 1.0 - ((d / d_near).ln() / denom).clamp(0.0, 1.0) } else { 0.0 })
                .collect(),
        };
        Scene3D { w, h, distance, control, normals: std::sync::Arc::new(normals), sky: Grid { w, h, data: sky }, fov_x_deg, focal_px, d_near, d_far, model, detail: None, edges: OnceLock::new() }
    }

    /// Metres for a control-depth value (0..1), read back through the log scale the control map
    /// was made with, so a refined control value still means the distance it stands for.
    pub fn metres(&self, control: f32, sky: f32) -> f32 {
        if sky >= 0.5 {
            return f32::INFINITY;
        }
        metres_of(control, self.d_near, self.d_far)
    }

    /// The control-depth value for a distance in metres (the inverse of `metres`).
    pub fn control_of(&self, metres: f32) -> f32 {
        control_of(metres, self.d_near, self.d_far)
    }

    /// The map a depth mask refines: the tiled detail when the tiled pass ran (more real detail
    /// than the model's own grid), else the model's control map.
    pub fn mask_source(&self) -> &Grid {
        self.detail.as_deref().unwrap_or(&self.control)
    }

    /// The two surfaces at the depth map's edges, made once from the refined map (`edges.rs`):
    /// what a depth mask reads so a band on the subject leaves the background's rim alone, and what
    /// Bokeh and Mist will read later. None where a scene has no edges made (a fallback map).
    pub fn edges(&self, refined: &Grid, guide: Option<&Grid>, subject: Option<&Grid>) -> Option<std::sync::Arc<EdgeMaps>> {
        self.edges
            .get_or_init(|| {
                if refined.w < 4 || refined.h < 4 {
                    return None;
                }
                let r = (refined.w.max(refined.h) / 512).max(2);
                Some(std::sync::Arc::new(crate::edges::edges(refined, guide, subject, r)))
            })
            .clone()
    }

    /// The control map as a plain map (`depth.rs`'s type), for the mask refinement and the cache.
    pub fn control_map(&self) -> crate::depth::DepthMap {
        crate::depth::DepthMap { w: self.w, h: self.h, data: self.control.data.clone() }
    }

    /// The sky at (x, y), fractions of the frame, bilinear.
    pub fn sky_at(&self, x: f32, y: f32) -> f32 {
        self.sky.at(x, y)
    }
}

/// Which depth a mask refines: the scene's own control map when MoGe-2 ran, else the map already
/// known (the folder's cache), else Depth Anything V2 Small's relative map, the fallback when the
/// scene models aren't installed or failed. The log says which ran.
pub fn control_depth(
    scene: Option<&Scene3D>,
    cached: Option<std::sync::Arc<crate::depth::DepthMap>>,
    relative: impl FnOnce() -> Option<crate::depth::DepthMap>,
) -> Option<std::sync::Arc<crate::depth::DepthMap>> {
    if let Some(s) = scene {
        eprintln!("[depth] masks: {}'s control depth", s.model);
        return Some(std::sync::Arc::new(s.control_map()));
    }
    if let Some(m) = cached {
        eprintln!("[depth] masks: the cached depth map (no scene geometry)");
        return Some(m);
    }
    let map = relative();
    eprintln!("[depth] masks: Depth Anything V2 Small (MoGe-2 not installed or failed)");
    map.map(std::sync::Arc::new)
}

/// A control-depth map and, when the scene knows one, a refined sky mask and the two surfaces at
/// the map's edges (all at 2048 on the long side): what a depth mask reads, and what phase 2's
/// Bokeh and phase 3's Mist will read for their own two-surface edges.
#[derive(Debug)]
pub struct Refined {
    pub control: std::sync::Arc<Grid>,
    pub sky: Option<std::sync::Arc<Grid>>,
    pub edges: Option<std::sync::Arc<EdgeMaps>>,
    /// The metric anchors the control map's log scale runs between, metres (near, far), when the
    /// scene geometry was there to say. Bokeh turns a control value into the distance a lens maths
    /// needs; Mist turns it into the distance its extinction needs and the focal length its rays.
    pub anchors: Option<(f32, f32)>,
    /// The focal length in pixels of the control map's own frame (x and y alike), when the scene
    /// has it: Mist's rays and height fog need it.
    pub focal_px: Option<f32>,
    /// The ground plane fitted from the scene's points and normals (`scene::ground`), for Mist's
    /// height fog: camera-space metres, normal up, the camera's height above it.
    pub ground: Option<crate::scene::Ground>,
    /// The scene's surface normals (MoGe's, at its own size), which Relight shades with. None when
    /// the map came from Depth Anything alone: Relight then derives them from the depth.
    pub normals: Option<std::sync::Arc<Normals>>,
}

/// The scene's normals at the model's own size: unit vectors in camera space (x right, y down,
/// z forward), shared with the scene rather than copied.
#[derive(Clone, Debug)]
pub struct Normals {
    pub w: usize,
    pub h: usize,
    pub data: std::sync::Arc<Vec<[f32; 3]>>,
}

impl Normals {
    /// The normal at fractions `u`, `v` of the frame, bilinearly.
    pub fn at(&self, u: f32, v: f32) -> [f32; 3] {
        let (w, h) = (self.w.max(1), self.h.max(1));
        let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * w + x];
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let top = mix(p(x0, y0), p(x1, y0), tx);
        let bottom = mix(p(x0, y1), p(x1, y1), tx);
        let n = mix(top, bottom, ty);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

/// The ground plane fitted from a scene's points and normals: unit normal (pointing up, so its y
/// is negative in camera space), and the offset with the plane at `normal · P + offset = 0`; the
/// camera sits at the origin, `offset` metres above the plane.
#[derive(Clone, Copy, Debug)]
pub struct Ground {
    pub normal: [f32; 3],
    pub offset: f32,
}

impl Ground {
    /// Height above the plane of a camera-space point, metres.
    pub fn height(&self, p: [f32; 3]) -> f32 {
        self.normal[0] * p[0] + self.normal[1] * p[1] + self.normal[2] * p[2] + self.offset
    }
}

/// The ground plane from MoGe's points and normals: a deterministic RANSAC over points whose
/// normals point up (camera y is down), the plane with the most inliers; a horizontal plane through
/// the lowest 10% of points when the geometry is too thin or a flat fit fails. The camera origin is
/// above it, so `offset` is the camera's height (metres).
pub fn ground_of(s: &Scene3D) -> Option<Ground> {
    let (w, h) = (s.w, s.h);
    if w < 8 || h < 8 || !(s.focal_px > 1.0) {
        return None;
    }
    let step = ((w * h) / 6000).max(1);
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for i in (0..w * h).step_by(step) {
        let z = s.distance[i];
        if !z.is_finite() || z <= 0.0 || z > s.d_far * 1.5 || s.sky.data[i] > 0.5 {
            continue;
        }
        let x = (((i % w) as f32 + 0.5) - w as f32 / 2.0) / s.focal_px * z;
        let y = (((i / w) as f32 + 0.5) - h as f32 / 2.0) / s.focal_px * z;
        ys.push(y);
        if s.normals[i][1] < -0.5 {
            points.push([x, y, z]);
        }
    }
    let horizontal = |ys: &mut Vec<f32>| -> Option<Ground> {
        if ys.len() < 20 {
            return None;
        }
        ys.sort_by(f32::total_cmp);
        let y = ys[((ys.len() - 1) as f32 * 0.9) as usize];
        (y > 0.03).then(|| Ground { normal: [0.0, -1.0, 0.0], offset: y })
    };
    if points.len() < 30 {
        return horizontal(&mut ys);
    }
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut best: Option<(Ground, usize)> = None;
    let n = points.len();
    for k in 0..60 {
        let (i1, i2, i3) = (k * n / 60, (k * 37 + n / 3) % n, (k * 91 + 2 * n / 3) % n);
        if i1 == i2 || i2 == i3 || i1 == i3 {
            continue;
        }
        let (p1, p2, p3) = (points[i1], points[i2], points[i3]);
        let mut normal = cross(sub(p2, p1), sub(p3, p1));
        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if !(len > 1e-6) {
            continue;
        }
        normal = normal.map(|v| v / len);
        if normal[1] > 0.0 {
            normal = normal.map(|v| -v);
        }
        if normal[1] > -0.5 {
            continue;
        }
        let offset = -dot(normal, p1);
        if !(offset > 0.05) {
            continue;
        }
        let inliers = points.iter().filter(|p| (dot(normal, **p) + offset).abs() < 0.03 * p[2] + 0.02).count();
        if best.as_ref().is_none_or(|(_, b)| inliers > *b) {
            best = Some((Ground { normal, offset }, inliers));
        }
    }
    match best {
        Some((g, count)) if count >= 30 => Some(g),
        _ => horizontal(&mut ys),
    }
}

/// The 0.5% and 99.5% quantiles of the valid distances.
fn anchors(distance: &[f32]) -> (f32, f32) {
    let mut valid: Vec<f32> = distance.iter().copied().filter(|d| d.is_finite() && *d > 0.0).collect();
    if valid.is_empty() {
        return (1.0, 2.0);
    }
    valid.sort_by(f32::total_cmp);
    let at = |q: f32| valid[((valid.len() - 1) as f32 * q).round() as usize];
    let (mut near, mut far) = (at(0.005), at(0.995));
    if !(far > near * 1.001) {
        // A flat scene (a wall, a document): keep a sane span rather than a zero one.
        near = valid[0];
        far = valid[valid.len() - 1].max(near * 1.01);
    }
    (near.max(1e-3), far.max(near * 1.01))
}

/// MoGe's `normalized_view_plane_uv`: corner (-w/diag, -h/diag) to (w/diag, h/diag), the same
/// units the focal length is relative to. Pixel `i` of `n`.
fn uv_at(i: usize, n: usize, span: f32) -> f32 {
    if n <= 1 {
        return 0.0;
    }
    span * (2.0 * i as f32 - (n - 1) as f32) / n as f32
}

/// MoGe's `recover_focal_shift` (MIT): the focal length and the z shift that best fit the point
/// map as `uv = focal * (x, y) / (z + shift)` over a 64 x 64 nearest downsample of the real
/// geometry. Ported from MoGe's `geometry_torch.py` / `geometry_numpy.py` (their least-squares
/// solve, done here with Levenberg-Marquardt and MoGe's closed form for the focal each step:
/// given the shift, the best focal is `sum(xy_proj . uv) / sum(|xy_proj|^2)`).
///
/// Returns (focal relative to half the diagonal, z shift).
pub fn recover_focal_shift(points: &[[f32; 3]], w: usize, h: usize, valid: &[bool]) -> (f32, f32) {
    if w == 0 || h == 0 || points.len() < w * h {
        return (1.0, 0.0);
    }
    let diagonal = ((w * w + h * h) as f32).sqrt();
    let (span_x, span_y) = ((w as f32 / diagonal), (h as f32 / diagonal));
    // MoGe's downsample: 64 x 64, nearest, then only the real geometry.
    const DOWN: usize = 64;
    let mut samples: Vec<(f32, f32, f32, f32, f32)> = Vec::with_capacity(DOWN * DOWN);
    for j in 0..DOWN.min(h) {
        let py = j * h / DOWN;
        for i in 0..DOWN.min(w) {
            let px = i * w / DOWN;
            let k = py * w + px;
            if !valid[k] {
                continue;
            }
            let p = points[k];
            if !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            samples.push((uv_at(px, w, span_x), uv_at(py, h, span_y), p[0], p[1], p[2]));
        }
    }
    if samples.len() < 2 {
        return (1.0, 0.0);
    }
    // Levenberg-Marquardt on (shift, focal), residual r = focal * xy / (z + shift) - uv.
    let (mut focal, mut shift, mut lambda) = (1.0f64, 0.0f64, 1e-3f64);
    let cost = |focal: f64, shift: f64| -> f64 {
        samples
            .iter()
            .map(|&(u, v, x, y, z)| {
                let dz = z as f64 + shift;
                if dz.abs() < 1e-9 {
                    return 1e9;
                }
                let (ex, ey) = (focal * x as f64 / dz - u as f64, focal * y as f64 / dz - v as f64);
                ex * ex + ey * ey
            })
            .sum()
    };
    let mut best = cost(focal, shift);
    for _ in 0..100 {
        // Normal equations: A = J^T J (2 x 2), b = J^T r.
        let (mut ass, mut asf, mut aff, mut bs, mut bf) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for &(u, v, x, y, z) in &samples {
            let dz = z as f64 + shift;
            if dz.abs() < 1e-9 {
                continue;
            }
            let inv = 1.0 / dz;
            // d(r)/d(shift) = -focal * xy / dz^2; d(r)/d(focal) = xy / dz.
            let (jsx, jsy) = (-focal * x as f64 * inv * inv, -focal * y as f64 * inv * inv);
            let (jfx, jfy) = (x as f64 * inv, y as f64 * inv);
            let (rx, ry) = (focal * x as f64 * inv - u as f64, focal * y as f64 * inv - v as f64);
            ass += jsx * jsx + jsy * jsy;
            asf += jsx * jfx + jsy * jfy;
            aff += jfx * jfx + jfy * jfy;
            bs += jsx * rx + jsy * ry;
            bf += jfx * rx + jfy * ry;
        }
        let (a00, a01, a11) = (ass + lambda * ass.max(1e-12), asf, aff + lambda * aff.max(1e-12));
        let det = a00 * a11 - a01 * a01;
        if det.abs() < 1e-18 {
            break;
        }
        let (ds, df) = ((-bs * a11 + bf * a01) / det, (-bf * a00 + bs * a01) / det);
        let (nf, ns) = (focal + df, shift + ds);
        let (c, limit) = (cost(nf, ns), ds.abs().max(df.abs()));
        if c.is_finite() && c < best && nf > 0.0 {
            focal = nf;
            shift = ns;
            best = c;
            lambda = (lambda * 0.5).max(1e-9);
        } else {
            lambda *= 4.0;
        }
        if limit < 1e-12 {
            break;
        }
    }
    (focal as f32, shift as f32)
}

/// The long side of a refined control map and sky mask (as the subject mask's).
const REFINED_LONG: usize = 2048;

/// The sky mask refined against the photo: upsampled to REFINED_LONG on its long side, its edge
/// snapped to the photo's own edge (a horizon against a tree line, a roofline) with a guided
/// filter on its lightness, then clamped to the coarse mask's local range so it can't spill.
/// Away from an edge the coarse mask is flat, so the filter changes nothing there: only the edge
/// moves, and the normals stay smooth (they are left at the model's size, sampled bilinearly).
pub fn refine_sky(sky: &Grid, image: &SceneImage) -> Grid {
    let small = image.downscaled(REFINED_LONG);
    let (w, h) = (small.width, small.height);
    let n = w * h;
    let peak = small.px.iter().map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).fold(1e-4f32, f32::max);
    let guide: Vec<f32> = small.px.iter().map(|p| ((0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0) / peak).cbrt()).collect();
    let rough: Vec<f32> = (0..n).map(|i| sky.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)).collect();
    let r = (w.max(h) / 256).max(2);
    let filtered = crate::tools::guided_filter(&guide, &rough, w, h, r, 1e-3);
    let negated: Vec<f32> = rough.iter().map(|v| -v).collect();
    let lo = crate::tools::min_filter(&rough, w, h, r);
    let hi: Vec<f32> = crate::tools::min_filter(&negated, w, h, r).into_iter().map(|v| -v).collect();
    let data = filtered.iter().zip(lo.iter().zip(hi.iter())).map(|(&v, (&lo, &hi))| v.clamp(lo, hi).clamp(0.0, 1.0)).collect();
    Grid { w, h, data }
}

/// The photo's perceptual lightness at a `w` x `h` grid, 0..1-ish: the guide both the refinement
/// and the edge maps follow (cube root of luminance, as `depth::refine_map` uses).
pub fn lightness_guide(image: &SceneImage, w: usize, h: usize) -> Grid {
    let small = image.downscaled(w.max(h).max(64));
    let peak = small.px.iter().map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).fold(1e-4f32, f32::max);
    let data = (0..w * h)
        .map(|i| {
            let x = ((i % w) as f32 + 0.5) / w as f32;
            let y = ((i / w) as f32 + 0.5) / h as f32;
            let sx = (x * small.width as f32) as usize;
            let sy = (y * small.height as f32) as usize;
            let p = small.px[sy.min(small.height - 1) * small.width + sx.min(small.width - 1)];
            ((0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0) / peak).cbrt()
        })
        .collect();
    Grid { w, h, data }
}

/// The scene's refined control map, sky mask and edge surfaces, at REFINED_LONG on the photo's
/// long side. The map refined is the tiled detail when the tiled pass ran (`Scene3D::mask_source`),
/// so the extra detail reaches the mask; `subject` is the photo's refined subject matte, which is
/// what makes a hair's edge exact (`edges::edges`).
pub fn refine(scene: &Scene3D, image: &SceneImage, subject: Option<&Grid>) -> Refined {
    refine_map_with(Some(scene), scene.mask_source(), image, subject)
}

/// As `refine`, from a given control map (the folder's cached one, or the fallback model's) rather
/// than the scene's own; `scene` supplies the edge cache when there is one.
pub fn refine_map_with(scene: Option<&Scene3D>, source: &Grid, image: &SceneImage, subject: Option<&Grid>) -> Refined {
    let control = crate::depth::refine_map(source, image);
    let refined_w = control.w.max(1) as f32;
    let focal_px = scene.map(|s| s.focal_px * refined_w / s.w.max(1) as f32);
    let guide = lightness_guide(image, control.w, control.h);
    let matte = subject.map(|m| crate::hq::resample(m, control.w, control.h));
    let edges = match scene {
        Some(s) => s.edges(&control, Some(&guide), matte.as_ref()),
        None => Some(std::sync::Arc::new(crate::edges::edges(&control, Some(&guide), matte.as_ref(), (control.w.max(control.h) / 512).max(2)))),
    };
    Refined {
        control: std::sync::Arc::new(control),
        sky: scene.map(|s| std::sync::Arc::new(refine_sky(&s.sky, image))),
        edges,
        anchors: scene.map(|s| (s.d_near, s.d_far)),
        focal_px,
        ground: scene.and_then(ground_of),
        normals: scene.map(|s| std::sync::Arc::new(Normals { w: s.w, h: s.h, data: s.normals.clone() })),
    }
}

// --- The tiled pass (phase 1c §3) ---------------------------------------------------------------

/// The window size the tiled pass uses, in the picture's own pixels: the relative model's own
/// input size, so each window is seen at full resolution rather than squeezed into the global
/// pass (Depth Anything V2 Small's 518 in Standard, the High quality model's 1036 there).
pub fn tile_side(quality: Quality) -> usize {
    match quality {
        Quality::High => crate::hq::SIZE,
        Quality::Standard => crate::depth::MODEL.long_side,
    }
}

/// How many windows at most, before the time budget has a say: a wide spread over a big picture
/// (the budget below usually stops it well before this).
const TILE_MAX: usize = 24;

/// The tiled pass: overlapping windows over the developed picture at the relative model's own
/// input size, each its own output least-squares fitted to the global map over that window (in
/// inverse depth, the space a disparity map is linear in), then blended together with raised-cosine
/// weights — detail from the windows, layout from the global pass.
///
/// `budget`: the most time to spend (a third of the global pass is the brief's cap: three times its
/// cost in total), and windows are skipped once it's gone. Returns the control map at the refined
/// size (2048 on the long side), or None when the picture is too small to gain anything or the
/// model isn't there.
pub fn detail(image: &SceneImage, scene: &Scene3D, quality: Quality, budget: std::time::Duration) -> Option<Grid> {
    let started = std::time::Instant::now();
    let side_px = tile_side(quality);
    let small = image.downscaled(REFINED_LONG);
    let (dw, dh) = (small.width, small.height);
    if dw < 8 || dh < 8 {
        return None;
    }
    // The windows are square in the picture, so they are square in the grid too (same scale both
    // ways); the plan is made in grid pixels.
    let side = ((side_px as f32 * dw as f32 / image.width.max(1) as f32).round() as usize).max(8);
    let tiles = crate::fusion::plan(dw, dh, side, TILE_MAX);
    if tiles.is_empty() {
        return None;
    }
    let global = crate::hq::resample(&scene.control, dw, dh);
    let white = crate::depth::white_point(image);
    let quality_name = if quality == Quality::High { "Depth Anything 3 Mono-Large" } else { "Depth Anything V2 Small" };
    let mut parts: Vec<(crate::fusion::Tile, Grid)> = Vec::with_capacity(tiles.len());
    for tile in &tiles {
        if !parts.is_empty() && started.elapsed() > budget {
            break;
        }
        // The window in the picture's own pixels, and its relative depth at the grid's size.
        let x0 = tile.x0 * image.width / dw;
        let y0 = tile.y0 * image.height / dh;
        let x1 = ((tile.x0 + tile.w) * image.width / dw).min(image.width);
        let y1 = ((tile.y0 + tile.h) * image.height / dh).min(image.height);
        let rect = (x0, y0, x1.saturating_sub(x0).max(8), y1.saturating_sub(y0).max(8));
        let relative = match quality {
            Quality::High => crate::hq::inverse_depth_white(image, rect, tile.w, tile.h, white),
            Quality::Standard => crate::depth::relative_rect(image, rect, tile.w, tile.h, white),
        };
        let Some(relative) = relative else { continue };
        // The window's values as its own relative distance (both models give inverse depth).
        let model: Vec<f32> = relative.data.iter().map(|v| if v.is_finite() && *v > 1e-4 { 1.0 / v } else { 0.0 }).collect();
        // Least squares of the window against the global map over it, in inverse depth.
        let mut pairs = Vec::with_capacity(model.len());
        for y in 0..tile.h {
            for x in 0..tile.w {
                let v = model[y * tile.w + x];
                if v <= 0.0 {
                    continue;
                }
                let g = global.data[(tile.y0 + y) * dw + tile.x0 + x];
                let z = metres_of(g, scene.d_near, scene.d_far);
                if z.is_finite() && z > 0.0 {
                    pairs.push((z, v));
                }
            }
        }
        // A window with too little real geometry, or a fit that doesn't rise, is left to the global.
        let Some((a, b)) = crate::fusion::fit_plain(&pairs, crate::fusion::Space::Inverse) else { continue };
        let mut data = Vec::with_capacity(model.len());
        for &v in &model {
            let z = if v > 0.0 { crate::fusion::Space::Inverse.metres_of(a, b, v) as f32 } else { f32::NAN };
            data.push(if z.is_finite() && z > 0.0 { control_of(z, scene.d_near, scene.d_far) } else { f32::NAN });
        }
        parts.push((*tile, Grid { w: tile.w, h: tile.h, data }));
    }
    if parts.is_empty() {
        return None;
    }
    let out = crate::fusion::blend(&global, &parts.iter().map(|(t, g)| (*t, g)).collect::<Vec<_>>());
    eprintln!(
        "[scene] tiled detail: {quality_name}, {} of {} windows in {:.2} s (budget {:.2} s), {}x{}",
        parts.len(),
        tiles.len(),
        started.elapsed().as_secs_f32(),
        budget.as_secs_f32(),
        out.w,
        out.h
    );
    Some(out)
}

// --- The cache: `<photo>.scene.bin` -------------------------------------------------------------

/// The file's magic and version (a newer layout gets a new number and is ignored until saved).
/// 2 (phase 1c): after the model-size geometry, the tiled detail map and the edge surfaces, each
/// 8-bit and deflated (mostly flat, so a fraction of their raw size).
const MAGIC: &[u8; 4] = b"R1SC";
const VERSION: u16 = 2;

/// f32 to IEEE half (round to nearest even), and back. Distances are stored at half precision:
/// 6 mm at 6 m, far finer than the model's own accuracy, and half the file.
fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x7F_FFFF;
    if exp == 0xFF {
        // Infinity or NaN: half has both (a quiet NaN keeps its sign).
        return sign | 0x7C00 | if mant != 0 { 0x200 } else { 0 };
    }
    let unbiased = exp - 127;
    if unbiased > 15 {
        return sign | 0x7C00;
    }
    if unbiased >= -14 {
        let e = ((unbiased + 15) as u16) << 10;
        let m = (mant >> 13) as u16;
        let rest = mant & 0x1FFF;
        // Round to nearest even.
        let mut out = sign | e | m;
        if rest > 0x1000 || (rest == 0x1000 && (m & 1) == 1) {
            out = out.wrapping_add(1);
        }
        return out;
    }
    if unbiased < -24 {
        return sign;
    }
    // Subnormal: shift the implicit 1 into the mantissa.
    let m = (mant | 0x80_0000) >> (-unbiased - 14 + 13);
    sign | m as u16
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // Subnormal: normalise it.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        sign | 0x7F80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// The signed normal component as one byte: -1..1 to 0..255.
fn normal_to_u8(v: f32) -> u8 {
    (((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0) + 0.5) as u8
}
fn u8_to_normal(b: u8) -> f32 {
    b as f32 / 127.5 - 1.0
}

/// The scene as `<photo>.scene.bin`: a small header, then half-precision distances, 8-bit
/// normals (x, y, z) and 8-bit sky, all at the model's size.
pub fn to_bytes(scene: &Scene3D) -> Vec<u8> {
    let name = scene.model.as_bytes();
    let mut out = Vec::with_capacity(32 + scene.w * scene.h * 6);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(scene.w as u32).to_le_bytes());
    out.extend_from_slice(&(scene.h as u32).to_le_bytes());
    for v in [scene.fov_x_deg, scene.focal_px, scene.d_near, scene.d_far] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(name.len().min(255) as u8);
    out.extend_from_slice(&name[..name.len().min(255)]);
    for &d in &scene.distance {
        out.extend_from_slice(&f32_to_f16(d).to_le_bytes());
    }
    for n in scene.normals.iter() {
        out.extend_from_slice(&[normal_to_u8(n[0]), normal_to_u8(n[1]), normal_to_u8(n[2])]);
    }
    for &s in &scene.sky.data {
        out.push((s.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    // The tiled detail (control 0..1, 8-bit: a 0.4% step of the log range, finer than any mask's
    // fade) and the edge surfaces (alpha, near, far), deflated: they are mostly flat.
    let mut flags = 0u8;
    if scene.detail.is_some() {
        flags |= 1;
    }
    if scene.edges.get().is_some_and(|e| e.is_some()) {
        flags |= 2;
    }
    out.push(flags);
    if let Some(detail) = &scene.detail {
        out.extend_from_slice(&(detail.w as u32).to_le_bytes());
        out.extend_from_slice(&(detail.h as u32).to_le_bytes());
        let raw: Vec<u8> = detail.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        push_zlib(&mut out, &raw);
    }
    if let Some(Some(edges)) = scene.edges.get() {
        out.extend_from_slice(&(edges.w as u32).to_le_bytes());
        out.extend_from_slice(&(edges.h as u32).to_le_bytes());
        let mut raw = Vec::with_capacity(edges.w * edges.h * 3);
        for data in [&edges.alpha, &edges.near, &edges.far] {
            raw.extend(data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
        push_zlib(&mut out, &raw);
    }
    out
}

/// `bytes` deflated (zlib) with its length, into `out`.
fn push_zlib(out: &mut Vec<u8>, bytes: &[u8]) {
    use flate2::write::ZlibEncoder;
    use std::io::Write;
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = encoder.write_all(bytes);
    let packed = encoder.finish().unwrap_or_default();
    out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    out.extend_from_slice(&packed);
}

/// The next `n` bytes of a deflated block, inflated.
fn take_zlib(bytes: &[u8], at: &mut usize, n: usize) -> Option<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let end = (*at).checked_add(4)?;
    let len = u32::from_le_bytes(bytes.get(*at..end)?.try_into().ok()?) as usize;
    *at = end;
    let packed = bytes.get(*at..(*at).checked_add(len)?)?;
    *at += len;
    let mut out = Vec::with_capacity(n);
    ZlibDecoder::new(packed).read_to_end(&mut out).ok()?;
    (out.len() == n).then_some(out)
}

/// A `u8` grid's values as 0..1 floats.
fn u8_grid(raw: &[u8]) -> Vec<f32> {
    raw.iter().map(|b| *b as f32 / 255.0).collect()
}

/// A scene from `to_bytes`: None if it isn't one, or is another version.
pub fn from_bytes(bytes: &[u8]) -> Option<Scene3D> {
    let mut at = 0usize;
    fn take<'a>(bytes: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
        let end = at.checked_add(n)?;
        let s = bytes.get(*at..end)?;
        *at = end;
        Some(s)
    }
    if take(bytes, &mut at, 4)? != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes(take(bytes, &mut at, 2)?.try_into().ok()?);
    if version != VERSION {
        return None;
    }
    let w = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
    let mut f = [0.0f32; 4];
    for v in &mut f {
        *v = f32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?);
    }
    let [fov_x_deg, focal_px, d_near, d_far] = f;
    let name_len = *take(bytes, &mut at, 1)?.first()? as usize;
    let model = std::str::from_utf8(take(bytes, &mut at, name_len)?).ok()?;
    let model: &'static str = match model {
        "MoGe-2 ViT-S" => "MoGe-2 ViT-S",
        "MoGe-2 ViT-B" => "MoGe-2 ViT-B",
        "MoGe-2 ViT-S + DA3 Mono-Large" => "MoGe-2 ViT-S + DA3 Mono-Large",
        "MoGe-2 ViT-B + DA3 Mono-Large" => "MoGe-2 ViT-B + DA3 Mono-Large",
        _ => return None,
    };
    let n = w.checked_mul(h)?;
    if n == 0 || n > 40_000_000 {
        return None;
    }
    let mut distance = Vec::with_capacity(n);
    for _ in 0..n {
        distance.push(f16_to_f32(u16::from_le_bytes(take(bytes, &mut at, 2)?.try_into().ok()?)));
    }
    let mut normals = Vec::with_capacity(n);
    for _ in 0..n {
        let b = take(bytes, &mut at, 3)?;
        let v = [u8_to_normal(b[0]), u8_to_normal(b[1]), u8_to_normal(b[2])];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        normals.push(if len > 1e-6 { [v[0] / len, v[1] / len, v[2] / len] } else { [0.0, 0.0, 1.0] });
    }
    let sky = take(bytes, &mut at, n)?.iter().map(|&b| b as f32 / 255.0).collect();
    // The control map, from the distances exactly as `from_raw` made it.
    let denom = (d_far / d_near).ln();
    let control = Grid {
        w,
        h,
        data: distance
            .iter()
            .map(|&d| if d.is_finite() && denom.abs() > 1e-6 { 1.0 - ((d / d_near).ln() / denom).clamp(0.0, 1.0) } else { 0.0 })
            .collect(),
    };
    // The tiled detail and the edge surfaces (phase 1c): absent in a file saved before them.
    let flags = *take(bytes, &mut at, 1)?.first()?;
    let detail = if flags & 1 != 0 {
        let dw = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
        let dh = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
        let n = dw.checked_mul(dh)?;
        if n == 0 || n > 40_000_000 {
            return None;
        }
        let raw = take_zlib(bytes, &mut at, n)?;
        Some(std::sync::Arc::new(Grid { w: dw, h: dh, data: u8_grid(&raw) }))
    } else {
        None
    };
    let edges = if flags & 2 != 0 {
        let ew = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
        let eh = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().ok()?) as usize;
        let n = ew.checked_mul(eh)?;
        if n == 0 || n > 40_000_000 {
            return None;
        }
        let raw = take_zlib(bytes, &mut at, n * 3)?;
        Some(std::sync::Arc::new(EdgeMaps {
            w: ew,
            h: eh,
            alpha: u8_grid(&raw[..n]),
            near: u8_grid(&raw[n..2 * n]),
            far: u8_grid(&raw[2 * n..]),
        }))
    } else {
        None
    };
    let scene = Scene3D { w, h, distance, control, normals: std::sync::Arc::new(normals), sky: Grid { w, h, data: sky }, fov_x_deg, focal_px, d_near, d_far, model, detail, edges: OnceLock::new() };
    let _ = scene.edges.set(edges);
    Some(scene)
}

// --- The point cloud: `scene_points` ---------------------------------------------------------------

/// The point cloud's magic and version. Version 2 adds each point's place in the photo (`u`, `v`)
/// so the Depth Scope can link a point in 3D back to its pixel (hover, pick, the photo marker).
const POINTS_MAGIC: &[u8; 4] = b"R1PT";
const POINTS_VERSION: u16 = 2;

/// Points for the Depth Scope: a camera-space point and its colour, as `(x, y, z)` in metres
/// (half precision), 8-bit r, g, b, and its place in the photo as `u`, `v` (u16, fractions of
/// the upright, uncropped frame x 65535). The sky is left out.
pub struct Cloud {
    /// The header and the points, ready for base64: magic, version (u16), point count (u32),
    /// then 13 bytes a point.
    pub bytes: Vec<u8>,
    pub count: usize,
}

/// A decimated cloud of at most `max_points` points: every `step`th real-geometry pixel (so it
/// covers the frame evenly), its colour from the developed picture. Up to 300k points.
pub fn cloud(scene: &Scene3D, image: &SceneImage, max_points: usize) -> Cloud {
    let max_points = max_points.clamp(1_000, 300_000);
    // The sky cutout at the photo's own resolution, not the model's grid (which is ~10x smaller
    // than a full-size photo: that staircase is what made a mountain ridge jagged). The refined
    // mask snaps to the photo's edge; a little dithering softens the last of the point grid.
    let sky = refine_sky(&scene.sky, image);
    let mut valid: Vec<usize> = (0..scene.w * scene.h)
        .filter(|&i| scene.distance[i].is_finite())
        .filter(|&i| {
            let u = ((i % scene.w) as f32 + 0.5) / scene.w as f32;
            let v = ((i / scene.w) as f32 + 0.5) / scene.h as f32;
            let s = sky.at(u, v).clamp(0.0, 1.0);
            let h = (i as u32).wrapping_mul(0x9E37_79B1);
            let noise = ((h >> 8) & 0xffff) as f32 / 65535.0;
            s < 0.5 + (noise - 0.5) * 0.4
        })
        .collect();
    let step = valid.len().div_ceil(max_points).max(1);
    valid = valid.into_iter().step_by(step).collect();

    // The picture's own colours, rendered as the models see it (brightness so the 99th
    // percentile is near white, then gamma).
    let mut lums: Vec<f32> = image.px.iter().step_by(7).map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).collect();
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let white = lums.get(lums.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-4);
    let (w, h) = (scene.w, scene.h);
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let mut bytes = Vec::with_capacity(10 + valid.len() * 13);
    bytes.extend_from_slice(POINTS_MAGIC);
    bytes.extend_from_slice(&POINTS_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(valid.len() as u32).to_le_bytes());
    for &i in &valid {
        let (px, py) = ((i % w) as f32, (i / w) as f32);
        let z = scene.distance[i];
        // Camera space: x right, y down, z forward, as MoGe's points.
        let x = (px - cx) / scene.focal_px * z;
        let y = (py - cy) / scene.focal_px * z;
        for v in [x, y, z] {
            bytes.extend_from_slice(&f32_to_f16(v).to_le_bytes());
        }
        let sx = (((px + 0.5) / w as f32 * image.width as f32) as usize).min(image.width - 1);
        let sy = (((py + 0.5) / h as f32 * image.height as f32) as usize).min(image.height - 1);
        let p = image.px[sy * image.width + sx];
        for c in 0..3 {
            let v = (p[c] / white).clamp(0.0, 1.0).powf(1.0 / 2.2);
            bytes.push((v * 255.0 + 0.5) as u8);
        }
        // Where the point is in the photo (pixel centres, fractions of the frame), so the Depth
        // Scope can find it again on the picture and vice versa.
        let u = ((px + 0.5) / w as f32 * 65535.0).round() as u16;
        let v = ((py + 0.5) / h as f32 * 65535.0).round() as u16;
        bytes.extend_from_slice(&u.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Cloud { bytes, count: valid.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_cutout_is_refined_to_the_photo_not_the_model_grid() {
        // A 400 x 300 photo with a 40 x 30 model mask (sky across the top half): the refined mask
        // must come back at the photo's size, so the cloud's edge is not the model's staircase.
        let mut data = vec![0.0f32; 40 * 30];
        for y in 0..15 {
            for x in 0..40 {
                data[y * 40 + x] = 1.0;
            }
        }
        let sky = Grid { w: 40, h: 30, data };
        let image = SceneImage { width: 400, height: 300, px: vec![[0.5, 0.5, 0.5]; 400 * 300], source_long: 400 };
        let refined = refine_sky(&sky, &image);
        assert_eq!((refined.w, refined.h), (400, 300), "the sky mask follows the photo's resolution");
    }

    /// A synthetic point map: a plane at z = 1 seen with focal `f` (relative to half the
    /// diagonal) and shifted by `shift` in the affine space the model predicts.
    fn synthetic(w: usize, h: usize, focal: f32, shift: f32) -> Vec<[f32; 3]> {
        let diagonal = ((w * w + h * h) as f32).sqrt();
        let (sx, sy) = (w as f32 / diagonal, h as f32 / diagonal);
        let mut points = Vec::with_capacity(w * h);
        for j in 0..h {
            for i in 0..w {
                let u = uv_at(i, w, sx);
                let v = uv_at(j, h, sy);
                let z = 1.3 + 0.4 * (i as f32 / w as f32); // a little depth variation
                points.push([u / focal * (z + shift), v / focal * (z + shift), z]);
            }
        }
        points
    }

    #[test]
    fn a_synthetic_point_map_gives_its_focal_and_shift_back() {
        let (w, h) = (140, 98);
        for (focal, shift) in [(0.8f32, 0.35f32), (1.15, -0.2), (0.6, 0.0)] {
            let points = synthetic(w, h, focal, shift);
            let valid = vec![true; w * h];
            let (f, s) = recover_focal_shift(&points, w, h, &valid);
            assert!((f - focal).abs() < 1e-3, "focal {f} vs {focal}");
            assert!((s - shift).abs() < 1e-3, "shift {s} vs {shift}");
        }
    }

    #[test]
    fn the_focal_becomes_the_horizontal_field_of_view() {
        // 0.7097 of the half diagonal at 3:2: about 98.6 degrees across (MoGe's reading of a
        // 14 mm full-frame frame is near 104: a little under is normal).
        let fov = 2.0 * ((518.0f32 * 0.5) / (0.5 * 0.7097 * (518.0f32 * 518.0 + 350.0 * 350.0).sqrt())).atan().to_degrees();
        assert!((fov - 98.6).abs() < 1.0, "{fov}");
    }

    #[test]
    fn the_model_sees_the_photos_shape_with_about_the_tokens_asked_for() {
        for (w, h, tokens) in [(6000, 4000, 2500), (4000, 6000, 1800), (3000, 3000, 1800), (10000, 2000, 1800)] {
            let (mw, mh) = model_size(w, h, tokens);
            assert_eq!(mw % PATCH, 0, "{mw}");
            assert_eq!(mh % PATCH, 0, "{mh}");
            let got = (mw / PATCH) * (mh / PATCH);
            let ratio = got as f32 / tokens as f32;
            assert!((0.8..1.2).contains(&ratio), "{w}x{h}: {mw}x{mh} = {got} tokens for {tokens}");
        }
        // The shape is kept, not padded: 3:2 stays 3:2 within a patch.
        let (mw, mh) = model_size(6000, 4000, 2500);
        assert!(((mw as f32 / mh as f32) - 1.5).abs() < 0.06, "{mw}x{mh}");
    }

    /// A scene from control values and distances, for the mapping tests.
    fn scene_of(distances: &[f32]) -> Scene3D {
        scene_grid(distances.len(), 1, distances)
    }

    /// A `w` x `h` scene from its distances (row-major), for the cloud's tests.
    fn scene_grid(w: usize, h: usize, distances: &[f32]) -> Scene3D {
        let (d_near, d_far) = anchors(distances);
        let denom = (d_far / d_near).ln();
        Scene3D {
            w,
            h,
            distance: distances.to_vec(),
            control: Grid {
                w,
                h,
                data: distances
                    .iter()
                    .map(|&d| if d.is_finite() { 1.0 - ((d / d_near).ln() / denom).clamp(0.0, 1.0) } else { 0.0 })
                    .collect(),
            },
            normals: std::sync::Arc::new(vec![[0.0, 0.0, 1.0]; w * h]),
            sky: Grid { w, h, data: distances.iter().map(|d| if d.is_finite() { 0.0 } else { 1.0 }).collect() },
            fov_x_deg: 60.0,
            focal_px: 500.0,
            d_near,
            d_far,
            model: "MoGe-2 ViT-S",
            detail: None,
            edges: OnceLock::new(),
        }
    }

    #[test]
    fn the_control_scale_is_log_between_robust_anchors_with_sky_farthest() {
        // Mostly 2..4 m, with one absurd near pixel and one absurd far one: the anchors ignore
        // them and 1 is nearest, 0 farthest, the sky at 0.
        let mut distances: Vec<f32> = (0..1000).map(|i| 2.0 + 2.0 * i as f32 / 999.0).collect();
        distances[10] = 0.01;
        distances[500] = 1e6;
        distances[700] = f32::INFINITY;
        let scene = scene_of(&distances);
        assert!(scene.d_near > 1.99 && scene.d_near < 2.1, "near anchor {}", scene.d_near);
        assert!(scene.d_far > 3.9 && scene.d_far < 4.01, "far anchor {}", scene.d_far);
        assert!((scene.control.data[1] - 1.0).abs() < 1e-4, "nearest is 1");
        assert!((scene.control.data[999] - 0.0).abs() < 1e-4, "farthest is 0");
        assert!(scene.control.data[700] < 1e-6, "sky is 0");
        // Halfway in log distance is halfway in control.
        let mid = (scene.d_near * scene.d_far).sqrt();
        assert!((scene.control_of(mid) - 0.5).abs() < 1e-4);
        assert!((scene.metres(0.5, 0.0) - mid).abs() < 1e-2, "{}", scene.metres(0.5, 0.0));
        // And back: a control value's metres, and the sky is infinite.
        for c in [0.0, 0.25, 0.5, 0.9, 1.0] {
            let d = scene.metres(c, 0.0);
            assert!((scene.control_of(d) - c).abs() < 1e-4, "{c}");
        }
        assert!(scene.metres(0.7, 0.9).is_infinite(), "sky is infinite");
    }

    #[test]
    fn every_side_is_a_patch_multiple_and_the_anchors_accept_a_flat_scene() {
        for (w, h) in [(1, 1), (14, 14), (1400, 7)] {
            let (mw, mh) = model_size(w, h, 1800);
            assert_eq!(mw % PATCH, 0);
            assert_eq!(mh % PATCH, 0);
        }
        let flat = vec![3.0f32; 100];
        let (near, far) = anchors(&flat);
        assert!(far > near && near > 0.0);
    }

    #[test]
    fn half_precision_round_trips_distances() {
        for v in [0.0f32, 0.5, 1.0, 2.5, 6.25, 12.5, 100.0, 1000.0, 1e-3, f32::INFINITY] {
            let back = f16_to_f32(f32_to_f16(v));
            if v.is_finite() {
                assert!((back - v).abs() <= v.abs().max(1e-3) * 6e-4, "{v} vs {back}");
            } else {
                assert!(back.is_infinite());
            }
        }
    }

    #[test]
    fn a_scene_file_round_trips() {
        let scene = scene_of(&[1.2, 2.0, 3.5, 8.0, f32::INFINITY, 1.5, 2.2, 4.4]);
        let bytes = to_bytes(&scene);
        let back = from_bytes(&bytes).expect("its own bytes");
        assert_eq!((back.w, back.h), (8, 1));
        assert_eq!(back.model, scene.model);
        assert!((back.fov_x_deg - scene.fov_x_deg).abs() < 1e-4);
        assert!((back.d_near - scene.d_near).abs() < 1e-3 && (back.d_far - scene.d_far).abs() < 1e-3);
        for (a, b) in scene.control.data.iter().zip(&back.control.data) {
            assert!((a - b).abs() < 2e-3, "{a} vs {b}");
        }
        for (a, b) in scene.distance.iter().zip(&back.distance) {
            if a.is_finite() {
                assert!(((a - b) / a).abs() < 2e-3, "{a} vs {b}");
            } else {
                assert!(b.is_infinite());
            }
        }
        // Another version (or anything else) is not a scene.
        let mut other = bytes.clone();
        other[4] = 9;
        assert!(from_bytes(&other).is_none());
        assert!(from_bytes(b"not a scene").is_none());
    }

    #[test]
    fn a_scene_file_keeps_its_tiled_detail_and_edge_surfaces() {
        let mut scene = scene_grid(2, 2, &[1.0, 2.0, 4.0, 8.0]);
        scene.detail = Some(std::sync::Arc::new(Grid { w: 3, h: 2, data: vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6] }));
        let _ = scene.edges.set(Some(std::sync::Arc::new(EdgeMaps {
            w: 2,
            h: 2,
            alpha: vec![1.0, 0.5, 0.0, 0.75],
            near: vec![0.8, 0.7, 0.6, 0.5],
            far: vec![0.2, 0.2, 0.2, 0.2],
        })));
        let bytes = to_bytes(&scene);
        let back = from_bytes(&bytes).expect("its own bytes");
        let detail = back.detail.expect("the tiled detail is kept");
        assert_eq!((detail.w, detail.h), (3, 2));
        for (a, b) in scene.detail.as_ref().unwrap().data.iter().zip(&detail.data) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
        let edges = back.edges.get().cloned().flatten().expect("the edge surfaces are kept");
        assert_eq!((edges.w, edges.h), (2, 2));
        for (a, b) in [1.0f32, 0.5, 0.0, 0.75].iter().zip(&edges.alpha) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
        assert!((edges.near[0] - 0.8).abs() < 0.01 && (edges.far[3] - 0.2).abs() < 0.01);
        // A file with neither is smaller, and reads back without them.
        let plain = scene_grid(2, 2, &[1.0, 2.0, 4.0, 8.0]);
        let back = from_bytes(&to_bytes(&plain)).expect("its own bytes");
        assert!(back.detail.is_none() && back.edges.get().cloned().flatten().is_none());
    }

    #[test]
    fn the_depth_map_falls_back_to_depth_anything_without_the_scene() {
        // With the scene: its control map, whatever else was cached.
        let scene = scene_of(&[1.0, 2.0, 4.0, f32::INFINITY]);
        let cached = std::sync::Arc::new(crate::depth::DepthMap { w: 1, h: 1, data: vec![0.9] });
        let from_scene = control_depth(Some(&scene), Some(cached.clone()), || unreachable!()).unwrap();
        assert_eq!(from_scene.data, scene.control.data);
        // Without it: the cached map (phase 1's, or Depth Anything's) if there is one, without
        // running any model.
        let from_cache = control_depth(None, Some(cached.clone()), || unreachable!()).unwrap();
        assert_eq!(from_cache.data, vec![0.9]);
        // Nothing cached: Depth Anything V2 Small, the fallback (or nothing, without it too).
        let relative = || Some(crate::depth::DepthMap { w: 2, h: 1, data: vec![0.3, 0.7] });
        let from_relative = control_depth(None, None, relative).unwrap();
        assert_eq!(from_relative.data, vec![0.3, 0.7]);
        assert!(control_depth(None, None, || None).is_none());
    }

    #[test]
    fn the_cloud_leaves_the_sky_out_and_is_capped() {
        let mut distances: Vec<f32> = (0..20000).map(|i| 1.0 + i as f32 / 1000.0).collect();
        for d in distances.iter_mut().take(500) {
            *d = f32::INFINITY;
        }
        let scene = scene_of(&distances);
        let image = SceneImage { width: 200, height: 100, px: vec![[0.5, 0.5, 0.5]; 200 * 100], source_long: 200 };
        let decimated = cloud(&scene, &image, 2000);
        assert!(decimated.count <= 2000 && decimated.count > 1000, "{}", decimated.count);
        assert_eq!(decimated.bytes.len(), 10 + decimated.count * 13);
        assert_eq!(&decimated.bytes[..4], POINTS_MAGIC);
        // Every point is real geometry: the 500 sky pixels can't be in it.
        assert!(decimated.count <= 19500);
        let all = cloud(&scene, &image, 300_000);
        // The cutout's edge is now soft (a refined mask, dithered), so a few pixels either side of
        // the sky's boundary may come or go; the body of the sky must still be gone.
        assert!(all.count <= 19500 && all.count >= 19000, "the sky is left out: {}", all.count);
    }

    /// A point of a v2 cloud: its camera-space metres, its 0..255 colour, and its place in the
    /// photo (u, v as 0..1).
    fn cloud_point(bytes: &[u8], i: usize) -> ([f32; 3], [u8; 3], [f32; 2]) {
        let at = 10 + i * 13;
        let half = |o: usize| f16_to_f32(u16::from_le_bytes([bytes[at + o], bytes[at + o + 1]]));
        let pos = [half(0), half(2), half(4)];
        let rgb = [bytes[at + 6], bytes[at + 7], bytes[at + 8]];
        let uv = [u16::from_le_bytes([bytes[at + 9], bytes[at + 10]]) as f32 / 65535.0, u16::from_le_bytes([bytes[at + 11], bytes[at + 12]]) as f32 / 65535.0];
        (pos, rgb, uv)
    }

    #[test]
    fn the_point_cloud_is_version_two_with_u_v_and_no_sky() {
        // A 20 x 10 frame (the scene's grid): every pixel real except a sky block top left.
        let (w, h) = (20usize, 10usize);
        let mut distances: Vec<f32> = (0..w * h).map(|i| 1.0 + 0.1 * (i % w) as f32).collect();
        for y in 0..3 {
            for x in 0..4 {
                distances[y * w + x] = f32::INFINITY;
            }
        }
        let scene = scene_grid(w, h, &distances);
        let image = SceneImage { width: 40, height: 20, px: vec![[0.25, 0.5, 0.75]; 40 * 20], source_long: 40 };
        let c = cloud(&scene, &image, 300_000);
        // Header: the magic, version 2, the count; then 13 bytes a point.
        assert_eq!(&c.bytes[..4], b"R1PT");
        assert_eq!(u16::from_le_bytes([c.bytes[4], c.bytes[5]]), 2);
        assert_eq!(u32::from_le_bytes([c.bytes[6], c.bytes[7], c.bytes[8], c.bytes[9]]) as usize, c.count);
        let expected_count = w * h - 12;
        assert!((c.count as isize - expected_count as isize).abs() <= 16, "the sky block is left out, with a soft edge: {}", c.count);
        assert_eq!(c.bytes.len(), 10 + c.count * 13);
        // The first point is pixel (4, 0): its u, v are that pixel's centre, and its metres
        // rebuild the camera-space point from the pixel's ray.
        let (pos, rgb, uv) = cloud_point(&c.bytes, 0);
        assert!(uv[0] >= 3.0 / w as f32 && uv[0] <= 5.0 / w as f32, "the first point sits at the sky block's right edge: {:?}", uv);
        assert!((uv[1] - 0.5 / h as f32).abs() < 2e-4, "{:?}", uv);
        // The colour is the picture's, as the model sees it (brightness so the 99th percentile is
        // near white, then gamma): one byte a channel, r, g, b.
        let white = 0.2627 * 0.25 + 0.678 * 0.5 + 0.0593 * 0.75;
        let expected: Vec<u8> = [0.25f32, 0.5, 0.75].iter().map(|c| ((c / white).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8).collect();
        assert_eq!(&rgb[..], &expected[..], "the picture's own colour");
        let px0 = (uv[0] * w as f32).floor() as usize;
        let z = scene.distance[px0];
        assert!(z.is_finite(), "the first point is real geometry");
        assert!((pos[2] - z).abs() < z * 1e-3, "{} vs {}", pos[2], z);
        assert!((pos[0] - (px0 as f32 - (w as f32 - 1.0) * 0.5) / 500.0 * z).abs() < 1e-3);
        // Round trip: every point's u, v lands back on the pixel it came from, which is real
        // geometry (the sky's pixels are nowhere in the cloud), and its metres agree with that
        // pixel's distance.
        let mut on_sky = 0usize;
        for i in 0..c.count {
            let (pos, _, uv) = cloud_point(&c.bytes, i);
            assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]), "{i}: {:?}", uv);
            let (px, py) = ((uv[0] * w as f32).floor() as usize, (uv[1] * h as f32).floor() as usize);
            assert!(px < w && py < h, "{i}: {px},{py}");
            let d = scene.distance[py * w + px];
            if !d.is_finite() {
                // Only the soft edge may keep a point that maps to the sky's own pixels.
                on_sky += 1;
                continue;
            }
            assert!((pos[2] - d).abs() < d * 2e-3, "{i}: {} vs {d}", pos[2]);
        }
        assert!(on_sky * 10 <= c.count, "the sky's body is out of the cloud, only its edge is soft: {on_sky} of {}", c.count);
    }
}
