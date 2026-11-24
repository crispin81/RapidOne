//! ONNX Runtime sessions on the graphics card where there is one, so the AI models (the subject
//! mask, noise reduction and the depth map) run there instead of on the CPU: DirectML on Windows
//! (whichever DirectX 12 card), WebGPU (Vulkan) on Linux, CoreML (noise reduction) or
//! WebGPU over Metal (the depth models) on Apple Silicon. Any failure
//! falls back to the CPU, so a machine without one still works.

use std::sync::Mutex;

/// Apple Silicon's "cards": the same graphics through CoreML (0) or through WebGPU (Metal). The depth
/// models only ever use WebGPU: CoreML aborts the whole app loading MoGe-2 ViT-B (an assertion in
/// Apple's MPSGraph, which no fallback here can catch) and can't run ViT-S.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const WEBGPU: i32 = 1;

/// The first device number to try for a depth model.
pub const DEPTH_DEVICE: usize = if cfg!(all(target_os = "macos", target_arch = "aarch64")) { 1 } else { 0 };

/// A session for `path`: on the CPU, or on graphics card `gpu` (Windows: a
/// DirectML device; Linux: WebGPU, which picks the card itself; Apple
/// Silicon: CoreML on the graphics, at full precision: not the Neural Engine,
/// whose 16-bit maths would change the result).
pub fn build(env: &ort::environment::Environment, path: &std::path::Path, gpu: Option<i32>) -> Option<ort::session::Session> {
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
        Some(WEBGPU) => builder.with_execution_providers([ort::ep::WebGPU::default().build().error_on_failure()]).ok()?,
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
        // ort's own default is the most efficient device, which on a Mac is CoreML: ask for the CPU.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        None => builder.with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU).ok()?,
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        None => builder,
    };
    builder.commit_from_file(path).ok()
}

/// A model on the fastest graphics card: each device (DirectML's on Windows; WebGPU's choice on
/// Linux; the Mac's own graphics) is timed with `run` (a pass through the model, false where it
/// fails) after two warm-up runs, and the quickest kept with its device number. None: no GPU works.
#[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
pub fn fastest(
    env: &ort::environment::Environment,
    path: &std::path::Path,
    run: impl Fn(&Mutex<ort::session::Session>) -> bool,
) -> Option<(ort::session::Session, usize)> {
    fastest_of(env, path, if cfg!(windows) { 0..4 } else { 0..1 }, 0.25, run)
}

/// As `fastest` for the depth models, with the warm-up run's limit in seconds: a card whose second
/// pass takes longer is dropped (a big model may need more than the noise reducer's quarter second).
#[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
pub fn fastest_with(
    env: &ort::environment::Environment,
    path: &std::path::Path,
    warm_limit: f32,
    run: impl Fn(&Mutex<ort::session::Session>) -> bool,
) -> Option<(ort::session::Session, usize)> {
    fastest_of(env, path, if cfg!(windows) { 0..4 } else { DEPTH_DEVICE..DEPTH_DEVICE + 1 }, warm_limit, run)
}

#[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
fn fastest_of(
    env: &ort::environment::Environment,
    path: &std::path::Path,
    devices: std::ops::Range<usize>,
    warm_limit: f32,
    run: impl Fn(&Mutex<ort::session::Session>) -> bool,
) -> Option<(ort::session::Session, usize)> {
    let mut best: Option<(f32, ort::session::Session, usize)> = None;
    for device in devices {
        let Some(s) = build(env, path, Some(device as i32)) else { continue };
        let s = Mutex::new(s);
        // A first run that isn't timed: it can include compiling the model for the
        // device (CoreML on a Mac: 20 s or more the first time, which put Denoise on
        // the CPU for the whole session, 25 Oct). Then a warm-up run; a device that
        // is clearly slow (an integrated chip can be slower than the CPU) is not
        // timed further.
        if !run(&s) {
            continue;
        }
        let warm = std::time::Instant::now();
        if !run(&s) || warm.elapsed().as_secs_f32() > warm_limit {
            continue;
        }
        let t = std::time::Instant::now();
        let ok = (0..3).all(|_| run(&s));
        let secs = t.elapsed().as_secs_f32() / 3.0;
        if ok && best.as_ref().is_none_or(|(b, _, _)| secs < *b) {
            best = Some((secs, s.into_inner().ok()?, device));
        }
    }
    best.map(|(_, s, device)| (s, device))
}

/// The graphics card an ONNX Runtime session runs on, as its name (for a test or a note): the
/// platform's, `device` as `fastest` found it.
pub fn ep_name(device: usize) -> String {
    if cfg!(windows) {
        format!("DirectML device {device}")
    } else if cfg!(target_os = "linux") {
        "WebGPU (Vulkan)".to_string()
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        if device == 0 { "CoreML (GPU)" } else { "WebGPU (Metal)" }.to_string()
    } else {
        "CPU".to_string()
    }
}
