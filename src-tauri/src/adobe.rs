//! Adobe Color, read from Adobe's own profile files when they're installed
//! on the user's computer (by Lightroom, Camera Raw or the DNG Converter).
//! Nothing of Adobe's is shipped with RapidOne: without those files the
//! Profile menu simply doesn't offer Adobe Color.

use crate::dcp::{Look, Profile};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// A camera's Adobe Standard profile with Adobe Color's look on top, and the
/// default tone curve for profiles that leave theirs out. `profile` is the one
/// Adobe Color builds on: "Adobe Standard v2" where the camera has one (as
/// Lightroom does: its Nikon Z 6 exports report v2 under Adobe Color and plain
/// Adobe Standard when that is asked for), else Adobe Standard; `standard` is
/// plain Adobe Standard.
pub struct AdobeColor {
    pub profile: Profile,
    pub standard: Profile,
    pub look: Look,
    pub default_curve: Vec<(f32, f32)>,
}

/// Camera Raw's folders: the shared one, then the user's own.
pub(crate) fn camera_raw_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Ok(p) = std::env::var("ProgramData") {
            dirs.push(PathBuf::from(p).join("Adobe/CameraRaw"));
        }
        if let Ok(p) = std::env::var("APPDATA") {
            dirs.push(PathBuf::from(p).join("Adobe/CameraRaw"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/Application Support/Adobe/CameraRaw"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Application Support/Adobe/CameraRaw"));
        }
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

/// The Adobe Color look file (an .xmp holding a look table and curve).
fn look_path() -> Option<PathBuf> {
    camera_raw_dirs().into_iter().map(|d| d.join("Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp")).find(|p| p.is_file())
}

/// Folders holding the per-camera "Adobe Standard" profiles.
fn standard_dirs() -> Vec<PathBuf> {
    camera_raw_dirs().into_iter().map(|d| d.join("CameraProfiles/Adobe Standard")).filter(|d| d.is_dir()).collect()
}

/// Whether Adobe Color can be offered on this computer at all.
pub fn available() -> bool {
    look_path().is_some() && !standard_dirs().is_empty()
}

fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The camera's "Adobe Standard v2" profile, by file name, if installed.
fn find_v2(camera: &str) -> Option<Profile> {
    standard_dirs().iter().map(|d| d.join(format!("{camera} Adobe Standard v2.dcp"))).find(|p| p.is_file()).and_then(|p| Profile::load(&p).ok())
}

/// The camera's Adobe Standard profile: by file name ("<camera> Adobe
/// Standard.dcp"), else by the camera model recorded inside each profile.
fn find_profile(camera: &str) -> Option<Profile> {
    let want = squash(camera);
    let dirs = standard_dirs();
    for dir in &dirs {
        let direct = dir.join(format!("{camera} Adobe Standard.dcp"));
        if direct.is_file() {
            return Profile::load(&direct).ok();
        }
    }
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if squash(name) == format!("{want}adobestandarddcp") {
                return Profile::load(&path).ok();
            }
        }
    }
    // Last resort: read each profile's own camera model (slower; cached).
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if let Ok(p) = Profile::load(&path) {
                if squash(&p.camera_model) == want {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Adobe's default tone curve, taken from an installed profile that carries
/// it in full (newer profiles leave it out and rely on the converter's).
pub fn default_curve() -> Vec<(f32, f32)> {
    static CURVE: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    CURVE
        .get_or_init(|| {
            for camera in ["Nikon D810", "Canon EOS 5D Mark III", "Nikon D800"] {
                if let Some(p) = find_profile(camera) {
                    if p.tone_curve.len() >= 2 {
                        return p.tone_curve;
                    }
                }
            }
            Vec::new()
        })
        .clone()
}

/// Adobe Color for a camera (as the decoder names it, e.g. "Nikon D810"),
/// loaded once and cached. None if Adobe's files aren't installed or don't
/// cover the camera.
pub fn for_camera(camera: &str) -> Option<Arc<AdobeColor>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<AdobeColor>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(camera) {
        return hit.clone();
    }
    let found = (|| {
        let look = Look::load(&look_path()?).ok()?;
        let standard = find_profile(camera)?;
        let profile = find_v2(camera).unwrap_or_else(|| standard.clone());
        Some(Arc::new(AdobeColor { profile, standard, look, default_curve: default_curve() }))
    })();
    cache.lock().unwrap().insert(camera.to_string(), found.clone());
    found
}

/// Lightroom's starting brightness under Adobe Color (stops), which its
/// profile files don't record: per camera model where measured, else per
/// make. Measured 2026-10-08 against Lightroom's exports of 33 kit photos
/// (`adobecheck` FIT_EV); photos of one model agree closely (D810 within
/// 0.01, S5II 0.05, 5D Mark II 0.01), so it's a per-model setting in
/// Lightroom. Since 2026-10-09 also 52 models from the CC0 samples, most with
/// two photos; the A7R IV rests on one (-0.72), which its scores bear out.
pub fn baseline_exposure(make: &str, camera: &str) -> f32 {
    let by_model = match camera {
        "Canon EOS 5D Mark II" => Some(0.32),
        "Canon EOS 5D Mark IV" => Some(0.07),
        "Canon EOS 5DS R" => Some(0.15),
        "Canon EOS 6D" => Some(0.16),
        "Canon EOS 6D Mark II" => Some(0.17),
        "Canon EOS 80D" => Some(-0.07),
        "Canon EOS R5" => Some(0.28),
        "Canon EOS R6 Mark II" => Some(0.13),
        "Canon EOS R7" => Some(0.01),
        "Fujifilm GFX 100S" => Some(0.19),
        "Nikon D610" => Some(0.30),
        "Nikon D810" => Some(0.29),
        "Nikon Z 6" => Some(0.25),
        "Panasonic DC-S5M2" => Some(-0.08),
        "Sony ILCE-7M4" => Some(0.15),
        // Measured 2026-10-09 on the shadow tint kits (CC0 raw.pixls.us samples,
        // usually two per model; Fuji with each photo's own exposure bias taken out).
        "Canon EOS 5D Mark III" => Some(0.23),
        "Canon EOS 750D" => Some(-0.15),
        "Canon EOS 7D Mark II" => Some(-0.32),
        "Canon EOS M50" => Some(0.05),
        "Canon EOS R" => Some(0.27),
        "Canon EOS R10" => Some(-0.11),
        "Canon EOS R3" => Some(0.99),
        "Canon EOS R50" => Some(-0.16),
        "Canon EOS R8" => Some(0.87),
        "Canon EOS RP" => Some(0.22),
        "Canon EOS-1D X Mark II" => Some(0.36),
        "Fujifilm GFX 50S" => Some(0.19),
        "Fujifilm X-E4" => Some(-0.01),
        "Fujifilm X-H2" => Some(-0.01),
        "Fujifilm X-Pro3" => Some(0.01),
        "Fujifilm X-S10" => Some(0.02),
        "Fujifilm X-T3" => Some(0.00),
        "Fujifilm X-T30" => Some(0.03),
        "Fujifilm X-T5" => Some(-0.05),
        "Nikon D500" => Some(0.30),
        "Nikon D5600" => Some(0.11),
        "Nikon D750" => Some(0.13),
        "Nikon D7500" => Some(0.28),
        "Nikon D780" => Some(0.13),
        "Nikon D850" => Some(0.32),
        "Nikon Df" => Some(0.13),
        "Nikon Z 5" => Some(0.16),
        "Nikon Z 50" => Some(0.38),
        "Nikon Z 6 2" => Some(0.14),
        "Nikon Z 7 2" => Some(0.05),
        "Nikon Z 8" => Some(0.14),
        "Nikon Z f" => Some(0.12),
        "OM Digital Solutions OM-1" => Some(0.28),
        "Olympus E-M1MarkII" => Some(0.80),
        "Olympus E-M1MarkIII" => Some(0.21),
        "Olympus E-M5 Mark III" => Some(0.27),
        "Panasonic DC-G9" => Some(0.60),
        "Panasonic DC-GH5" => Some(0.24),
        "Panasonic DC-GH6" => Some(-0.07),
        "Panasonic DC-GX9" => Some(0.19),
        "Panasonic DC-S1" => Some(-0.21),
        "Panasonic DC-S1R" => Some(0.15),
        "Panasonic DC-S5" => Some(-0.08),
        "Panasonic DC-S9" => Some(-0.02),
        "Pentax K-1 Mark II" => Some(-0.58),
        "Pentax K-3 Mark III" => Some(0.68),
        "Pentax K-70" => Some(0.45),
        "Pentax KP" => Some(0.40),
        "Sony ILCE-1" => Some(-0.08),
        "Sony ILCE-7RM3" => Some(0.00),
        "Sony ILCE-7SM3" => Some(-0.04),
        // One sample, but the same per-model measurement as the rest (it was
        // left out at first as unusual; its Mixer and base scores show it is right).
        "Sony ILCE-7RM4" => Some(-0.72),
        _ => None,
    };
    by_model.unwrap_or_else(|| baseline_for_make(make))
}

/// How much brighter Lightroom starts this model than its make's average (0 for
/// models not measured): the per-model part of the baseline, which Shinobi's
/// per-make values take on too.
pub fn model_offset(make: &str, camera: &str) -> f32 {
    baseline_exposure(make, camera) - baseline_for_make(make)
}

/// The per-make fallback for models not yet measured.
fn baseline_for_make(make: &str) -> f32 {
    match make.to_ascii_lowercase().as_str() {
        m if m.starts_with("panasonic") => -0.15,
        m if m.starts_with("fuji") => 0.09,
        m if m.starts_with("nikon") => 0.23,
        m if m.starts_with("canon") => 0.11,
        m if m.starts_with("sony") => 0.2,
        _ => 0.1,
    }
}

/// How much stronger Lightroom's Fine tuning Shadow tint is on this camera
/// model than our model predicts (white-balance rule included), measured on
/// the shadow tint kits (`toolmatch DIAG=bins`, Adobe path: Lightroom's
/// colour change over ours in the darker half): models whose photos agree
/// within 30%, plus the S5 II, GFX 100S and Z 6 at the mean of 4-7 photos.
/// Leave-one-photo-out over 133 photos: the strength mismatch (mean |log|)
/// falls from 0.243 to 0.070. None: no correction.
pub fn shadow_tint_scale(camera: &str) -> Option<f32> {
    match camera {
        "Canon EOS 5D Mark II" => Some(2.26),
        "Canon EOS 90D" => Some(0.91),
        "Canon EOS M50" => Some(1.24),
        "Canon EOS R" => Some(1.29),
        "Canon EOS R10" => Some(1.25),
        "Canon EOS R50" => Some(1.13),
        "Canon EOS R6" => Some(1.16),
        "Canon EOS R8" => Some(2.04),
        "Canon EOS RP" => Some(1.19),
        "Fujifilm GFX 100 II" => Some(1.83),
        "Fujifilm GFX 100S" => Some(1.04),
        "Fujifilm GFX 50S" => Some(0.82),
        "Fujifilm X-E4" => Some(1.74),
        "Fujifilm X-H2" => Some(0.95),
        "Fujifilm X-Pro3" => Some(0.80),
        "Fujifilm X-S10" => Some(1.00),
        "Fujifilm X-T2" => Some(0.88),
        "Fujifilm X-T3" => Some(0.86),
        "Fujifilm X-T30" => Some(0.43),
        "Fujifilm X-T4" => Some(0.97),
        "Fujifilm X-T5" => Some(0.81),
        "Hasselblad X2D 100C" => Some(1.04),
        "Nikon D500" => Some(1.56),
        "Nikon D5600" => Some(0.94),
        "Nikon D610" => Some(2.15),
        "Nikon D750" => Some(1.06),
        "Nikon D7500" => Some(1.05),
        "Nikon D780" => Some(0.99),
        "Nikon D810" => Some(1.49),
        "Nikon D850" => Some(1.17),
        "Nikon Df" => Some(1.70),
        "Nikon Z 5" => Some(1.06),
        "Nikon Z 50" => Some(1.33),
        "Nikon Z 6 2" => Some(1.36),
        "Nikon Z 6" => Some(1.01),
        "Nikon Z 7 2" => Some(1.03),
        "Nikon Z 7" => Some(0.86),
        "Nikon Z fc" => Some(1.11),
        "OM Digital Solutions OM-1" => Some(2.06),
        "Olympus E-M1MarkII" => Some(2.19),
        "Olympus E-M1MarkIII" => Some(1.69),
        "Olympus E-M5 Mark III" => Some(1.03),
        "Panasonic DC-G9 II" => Some(0.96),
        "Panasonic DC-G9" => Some(1.50),
        "Panasonic DC-GH5" => Some(1.09),
        "Panasonic DC-GH6" => Some(0.78),
        "Panasonic DC-GX9" => Some(1.09),
        "Panasonic DC-S5M2" => Some(0.88),
        "Panasonic DC-S9" => Some(0.79),
        "Pentax K-1 Mark II" => Some(0.60),
        "Pentax K-3 Mark III" => Some(1.32),
        "Sony ILCE-6400" => Some(1.05),
        "Sony ILCE-7CR" => Some(1.08),
        "Sony ILCE-7M3" => Some(1.43),
        "Sony ILCE-7RM3" => Some(0.96),
        "Sony ILCE-7SM3" => Some(1.01),
        "Sony ILCE-9" => Some(1.24),
        _ => None,
    }
}

/// Adobe's CameraCalibration for a camera model: per-channel factors (red,
/// green, blue) on its colour matrices, which Adobe DNG Converter writes into
/// DNGs and Lightroom applies (DNG spec: ColorMatrix' = CameraCalibration x
/// ColorMatrix). Without it, Kelvin/Tint numbers drift from Lightroom's (X-T5
/// as shot 5783 K vs Lightroom's 5300 K; with it 5280 K). Read 2026-10-09 from
/// DNGs of one sample per model (Nikon and Panasonic have none). It can vary
/// between bodies of a model (two GFX 100S shots disagreed), so this is the
/// measured body's value. None = no calibration.
pub fn camera_calibration(camera: &str) -> Option<[f32; 3]> {
    match camera {
        "Canon EOS 5D Mark III" => Some([1.0020, 1.0000, 1.0062]),
        "Canon EOS 750D" => Some([1.0041, 1.0000, 1.0134]),
        "Canon EOS 7D Mark II" => Some([1.0307, 1.0000, 0.9627]),
        "Canon EOS 90D" => Some([0.9983, 1.0000, 1.0341]),
        "Canon EOS M50" => Some([1.0253, 1.0000, 0.9712]),
        "Canon EOS R" => Some([0.9941, 1.0000, 1.0101]),
        "Canon EOS R10" => Some([0.9836, 1.0000, 1.0166]),
        "Canon EOS R3" => Some([1.0040, 1.0000, 0.9984]),
        "Canon EOS R50" => Some([0.9836, 1.0000, 1.0219]),
        "Canon EOS R6" => Some([1.0165, 1.0000, 1.0075]),
        "Canon EOS R7" => Some([1.0139, 1.0000, 0.9822]),
        "Canon EOS R8" => Some([0.9868, 1.0000, 1.0132]),
        "Canon EOS RP" => Some([1.0097, 1.0000, 1.0030]),
        "Canon EOS-1D X Mark II" => Some([1.0199, 1.0000, 0.9969]),
        "Fujifilm GFX 100 II" => Some([1.0099, 1.0000, 1.0147]),
        "Fujifilm GFX 100S" => Some([1.0633, 1.0000, 0.9715]),
        "Fujifilm GFX 50S" => Some([1.0089, 1.0000, 0.9503]),
        "Fujifilm X-E4" => Some([0.9812, 1.0000, 0.9538]),
        "Fujifilm X-H2" => Some([0.9639, 1.0000, 0.9671]),
        "Fujifilm X-Pro3" => Some([0.9921, 1.0000, 0.9834]),
        "Fujifilm X-S10" => Some([0.9812, 1.0000, 0.9425]),
        "Fujifilm X-T2" => Some([1.0362, 1.0000, 1.0430]),
        "Fujifilm X-T3" => Some([1.0129, 1.0000, 0.9855]),
        "Fujifilm X-T30" => Some([1.0032, 1.0000, 0.9814]),
        "Fujifilm X-T4" => Some([0.9921, 1.0000, 0.9387]),
        "Fujifilm X-T5" => Some([0.9347, 1.0000, 1.0550]),
        "OM Digital Solutions OM-1" => Some([0.9726, 1.0000, 0.8787]),
        "Olympus E-M1MarkII" => Some([1.0841, 1.0000, 0.8447]),
        "Olympus E-M1MarkIII" => Some([1.0255, 1.0000, 0.8939]),
        "Olympus E-M5 Mark III" => Some([0.9414, 1.0000, 1.0216]),
        "Sony ILCE-1" => Some([0.9894, 1.0000, 0.9710]),
        "Sony ILCE-6000" => Some([0.9504, 1.0000, 0.9107]),
        "Sony ILCE-6400" => Some([0.9939, 1.0000, 0.9558]),
        "Sony ILCE-6600" => Some([0.9420, 1.0000, 0.9518]),
        "Sony ILCE-7C" => Some([1.0476, 1.0000, 0.8829]),
        "Sony ILCE-7CR" => Some([0.9323, 1.0000, 1.0471]),
        "Sony ILCE-7M2" => Some([0.9984, 1.0000, 0.9430]),
        "Sony ILCE-7M3" => Some([1.0476, 1.0000, 0.9106]),
        "Sony ILCE-7RM3" => Some([0.9888, 1.0000, 0.9904]),
        "Sony ILCE-7RM4" => Some([1.0255, 1.0000, 0.9789]),
        "Sony ILCE-7RM5" => Some([0.9634, 1.0000, 0.9994]),
        "Sony ILCE-7SM3" => Some([1.0781, 1.0000, 0.9633]),
        "Sony ILCE-9" => Some([0.9872, 1.0000, 0.9901]),
        // Added the same day from the tools kit photos.
        "Canon EOS 5D Mark II" => Some([1.0064, 1.0000, 1.0065]),
        "Canon EOS 5D Mark IV" => Some([1.0022, 1.0000, 0.9836]),
        "Canon EOS 5DS R" => Some([0.9903, 1.0000, 0.9854]),
        "Canon EOS 6D" => Some([1.0305, 1.0000, 0.9635]),
        "Canon EOS 6D Mark II" => Some([0.9812, 1.0000, 0.9986]),
        "Canon EOS 80D" => Some([0.9742, 1.0000, 0.9804]),
        "Canon EOS R5" => Some([0.9905, 1.0000, 1.0197]),
        "Canon EOS R6 Mark II" => Some([0.9853, 1.0000, 1.0247]),
        "Sony ILCE-7M4" => Some([0.9817, 1.0000, 0.9687]),
        _ => None,
    }
}
