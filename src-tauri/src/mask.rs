//! The automatic subject mask: BiRefNet lite (MIT, Zheng Peng et al.), run
//! with ONNX Runtime. Finds the photo's main subject (people, a group, an
//! object) with no clicks. The model file is downloaded, never shipped in git:
//! `models/birefnet-lite.onnx` (the official `BiRefNet-general-bb_swin_v1_tiny`
//! export from the BiRefNet v1 release).

use crate::pipeline::SceneImage;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// The model's input size (square; the photo is stretched to it).
const SIZE: usize = 1024;
const MODEL_FILE: &str = "birefnet-lite.onnx";

/// A soft mask, 0..1 (1 = subject), at `SIZE` x `SIZE` over the whole frame.
#[derive(Clone, Debug)]
pub struct Mask {
    pub size: usize,
    pub data: Vec<f32>,
}

impl Mask {
    /// The mask at (x, y), each 0..1 across the frame, bilinear.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let n = self.size;
        let fx = (x * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let fy = (y * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * n + x];
        let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
        let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// Where the model file is (see `find_model`).
fn model_path() -> Option<PathBuf> {
    find_model(MODEL_FILE)
}

/// ONNX Runtime's environment, one for the app (the subject mask and AI
/// noise reduction share it).
pub(crate) fn ort_env() -> Option<&'static ort::environment::Environment> {
    static ENV: OnceLock<Option<ort::environment::Environment>> = OnceLock::new();
    ENV.get_or_init(|| ort::init().build().ok()).as_ref()
}

/// The installed app's bundled models folder (`models/` in Tauri's resource
/// dir: `/usr/lib/RapidOne` on Linux, `Contents/Resources` on a Mac, next to
/// the app on Windows), set at start-up.
static RESOURCE_MODELS: OnceLock<PathBuf> = OnceLock::new();

pub fn set_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_MODELS.set(dir.join("models"));
}

/// A model file in the models folder: `RAPIDONE_MODELS`, next to the app, the
/// installer's bundled models, or (in development) the project's `models/` folder.
pub(crate) fn find_model(file: &str) -> Option<PathBuf> {
    model_dirs().into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

/// The models folders, in the order they're searched.
pub(crate) fn model_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("RAPIDONE_MODELS") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(exe_dir.join("models"));
    }
    if let Some(d) = RESOURCE_MODELS.get() {
        dirs.push(d.clone());
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models"));
    dirs
}

fn session() -> Option<&'static Mutex<ort::session::Session>> {
    static SESSION: OnceLock<Option<Mutex<ort::session::Session>>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let path = model_path()?;
            let env = ort_env()?;
            let session = ort::session::Session::builder(env).ok()?.commit_from_file(&path).ok()?;
            Some(Mutex::new(session))
        })
        .as_ref()
}

/// Whether the subject model is installed.
pub fn available() -> bool {
    session().is_some()
}

/// The subject mask of a scene image (linear light), or None without the model.
pub fn subject(scene: &SceneImage) -> Option<Mask> {
    let session = session()?;

    // A rough display rendering: brightness scaled so the 99th percentile is
    // near white, then gamma; enough for the model to see the picture.
    let mut lums: Vec<f32> = scene.px.iter().step_by(7).map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).collect();
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let white = lums.get(lums.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-4);

    // Stretch to SIZE x SIZE (nearest neighbour is plenty at 1024), then
    // normalise with ImageNet's mean and spread, channels first.
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let mut input = vec![0.0f32; 3 * SIZE * SIZE];
    for y in 0..SIZE {
        let sy = ((y as f32 + 0.5) / SIZE as f32 * scene.height as f32) as usize;
        for x in 0..SIZE {
            let sx = ((x as f32 + 0.5) / SIZE as f32 * scene.width as f32) as usize;
            let p = scene.px[sy.min(scene.height - 1) * scene.width + sx.min(scene.width - 1)];
            for c in 0..3 {
                let v = (p[c] / white).clamp(0.0, 1.0).powf(1.0 / 2.2);
                input[c * SIZE * SIZE + y * SIZE + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }

    let tensor = ort::value::Tensor::from_array(([1usize, 3, SIZE, SIZE], input.into_boxed_slice())).ok()?;
    let mut session = session.lock().ok()?;
    let outputs = session.run(ort::inputs![tensor]).ok()?;
    // The last output is the finest prediction (logits).
    let last = outputs.len().checked_sub(1)?;
    let (_, logits) = outputs[last].try_extract_tensor::<f32>().ok()?;
    if logits.len() != SIZE * SIZE {
        return None;
    }
    let data = logits.iter().map(|&l| 1.0 / (1.0 + (-l).exp())).collect();
    Some(Mask { size: SIZE, data })
}
