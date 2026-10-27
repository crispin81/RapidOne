//! The scene-referred pipeline (CPU reference implementation).
//!
//!   developed RAW (camera RGB, white balanced)
//!     -> highlight blending (sensor-clipped pixels go neutral, not magenta)
//!     -> camera colour -> linear Rec.2020, unbounded ("scene-referred")
//!     -> edits in linear light (exposure today; more to come)
//!     -> view transform: the profile's tone curve, applied hue-preservingly
//!     -> sRGB for the screen
//!
//! Everything before the view transform is real light: values above 1 are
//! kept, so pulling exposure down recovers highlights the camera JPEG lost.

use crate::whitebalance::WhiteBalance;
use image::{DynamicImage, RgbImage};
use rayon::prelude::*;

/// Linear Rec.2020 (D65) -> XYZ.
pub const REC2020_TO_XYZ: [[f32; 3]; 3] = [
    [0.636_958, 0.144_617, 0.168_881],
    [0.262_700, 0.677_998, 0.059_302],
    [0.000_000, 0.028_073, 1.060_985],
];

/// XYZ (D65) -> linear sRGB.
pub const XYZ_TO_SRGB: [[f32; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266_0, 1.876_010_8, 0.041_556_0],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

pub fn mul3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn apply3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// The developed photo as linear Rec.2020 light, upright.
#[derive(Clone)]
pub struct SceneImage {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[f32; 3]>,
    /// Long edge of the photo at full sensor resolution, in pixels. Texture
    /// and Clarity work at a fixed size in those pixels, so a shrunken
    /// preview needs it to scale their radii.
    pub source_long: usize,
}

/// Turns white-balanced camera RGB into a scene image.
///
/// `clip` is where each channel saturates after white balance (the sensor's
/// ceiling, 1.0, times that channel's multiplier). Where any channel nears
/// its ceiling, the true colour is unknown - it was cut off - so the pixel
/// is blended towards neutral at its brightest channel's level. Without
/// this, blown skies and lamps turn magenta. Unclipped highlights keep all
/// their headroom above 1.
pub fn scene_from_camera(
    width: usize,
    height: usize,
    cam: &[[f32; 3]],
    clip: [f32; 3],
    camera_to_rec2020: &[[f32; 3]; 3],
) -> SceneImage {
    let px = cam
        .par_iter()
        .map(|&c| {
            let fill = (0..3).map(|i| c[i] / clip[i]).fold(0.0f32, f32::max);
            // 0 below 92% of the ceiling, easing to 1 at the ceiling.
            let t = ((fill - 0.92) / 0.08).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            let blended = if t > 0.0 {
                let level = c[0].max(c[1]).max(c[2]);
                std::array::from_fn(|i| c[i] * (1.0 - t) + level * t)
            } else {
                c
            };
            apply3(camera_to_rec2020, blended).map(|v| v.max(0.0))
        })
        .collect();
    SceneImage { width, height, px, source_long: width.max(height) }
}

impl SceneImage {
    /// Box-filter shrink so the long edge is at most `max_dim` - done in
    /// linear light, so it's a true average of the light.
    pub fn downscaled(&self, max_dim: usize) -> SceneImage {
        let long = self.width.max(self.height);
        if long <= max_dim {
            return self.clone();
        }
        let scale = max_dim as f32 / long as f32;
        let (w, h) = (
            ((self.width as f32 * scale).round() as usize).max(1),
            ((self.height as f32 * scale).round() as usize).max(1),
        );
        let px = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (x0, x1) = (x * self.width / w, ((x + 1) * self.width / w).max(x * self.width / w + 1));
                let (y0, y1) = (y * self.height / h, ((y + 1) * self.height / h).max(y * self.height / h + 1));
                let mut sum = [0.0f32; 3];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let p = self.px[yy * self.width + xx];
                        sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                sum.map(|v| v / n)
            })
            .collect();
        SceneImage { width: w, height: h, px, source_long: self.source_long }
    }

    /// Applies an EXIF orientation (1-8) so the image is upright.
    pub fn oriented(self, orientation: u16) -> SceneImage {
        let (w, h) = (self.width, self.height);
        let swap = matches!(orientation, 5..=8);
        let (nw, nh) = if swap { (h, w) } else { (w, h) };
        // Source pixel for each destination pixel.
        let src = |x: usize, y: usize| -> (usize, usize) {
            match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            }
        };
        if orientation <= 1 || orientation > 8 {
            return self;
        }
        let px = (0..nw * nh)
            .into_par_iter()
            .map(|i| {
                let (sx, sy) = src(i % nw, i / nw);
                self.px[sy * w + sx]
            })
            .collect();
        SceneImage { width: nw, height: nh, px, source_long: self.source_long }
    }
}

// ---- View transform: RapidOne Standard -------------------------------------

/// A filmic curve, x(a x + b) / (x(c x + d) + e): a toe in the shadows and a
/// long soft shoulder that brings highlights into white gradually.
/// Scene-linear in, display-linear out. The shape comes from Krzysztof
/// Narkowicz's widely used fit of the ACES film curve; RapidOne Standard's
/// coefficients are fitted by measurement (see `Tuning::curve`).
fn filmic(x: f32, k: &[f32; 5]) -> f32 {
    let x = x.max(0.0);
    (x * (k[0] * x + k[1])) / (x * (k[2] * x + k[3]) + k[4])
}

/// Display-linear value mid-grey should land on: L* 50, where Adobe's
/// default rendering puts an 18% grey card.
const MID_GREY_OUT: f32 = 0.184;

pub struct ToneCurve {
    /// Scales scene light into the curve so mid-grey lands on MID_GREY_OUT.
    pub(crate) pre_gain: f32,
    pub(crate) k: [f32; 5],
    /// All-zero parameters: the curve changes nothing (the "As is" profile).
    pub(crate) identity: bool,
}

impl ToneCurve {
    pub fn standard() -> Self {
        Self::with(&Tuning::default().curve)
    }

    pub fn with(k: &[f32; 5]) -> Self {
        if *k == [0.0; 5] {
            return Self { pre_gain: 1.0, k: *k, identity: true };
        }
        // Solve filmic(0.18 * g) = MID_GREY_OUT for g (filmic is monotonic).
        let (mut lo, mut hi) = (0.01f32, 100.0f32);
        for _ in 0..60 {
            let g = (lo * hi).sqrt();
            if filmic(0.18 * g, k) < MID_GREY_OUT {
                lo = g;
            } else {
                hi = g;
            }
        }
        Self { pre_gain: (lo * hi).sqrt(), k: *k, identity: false }
    }

    pub fn apply(&self, x: f32) -> f32 {
        if self.identity {
            return x.clamp(0.0, 1.0);
        }
        filmic(x * self.pre_gain, &self.k).min(1.0)
    }
}

/// Applies a tone curve to a colour without shifting its hue - the method
/// the DNG specification gives for profile tone curves: the curve maps the
/// largest and smallest channels, and the middle channel is placed at the
/// same proportion between them as before.
pub fn tone_rgb(curve: &ToneCurve, c: [f32; 3]) -> [f32; 3] {
    // The channels in order, as a stable sort would put them (ties keep their order): three
    // compares, not a general sort on every call (it showed in a render's time, 24 Oct).
    let mut idx = [0usize, 1, 2];
    let after = |a: usize, b: usize| c[a].total_cmp(&c[b]) == std::cmp::Ordering::Greater;
    if after(idx[0], idx[1]) {
        idx.swap(0, 1);
    }
    if after(idx[1], idx[2]) {
        idx.swap(1, 2);
        if after(idx[0], idx[1]) {
            idx.swap(0, 1);
        }
    }
    let (lo, mid, hi) = (c[idx[0]], c[idx[1]], c[idx[2]]);
    let (new_lo, new_hi) = (curve.apply(lo), curve.apply(hi));
    let new_mid = if hi > lo { new_lo + (new_hi - new_lo) * (mid - lo) / (hi - lo) } else { new_hi };
    let mut out = [0.0; 3];
    out[idx[0]] = new_lo;
    out[idx[1]] = new_mid;
    out[idx[2]] = new_hi;
    out
}

// ---- Rendering -----------------------------------------------------------

/// Where a rendered part of a photo sits in the whole photo: pixel (x, y) of
/// the part is pixel (x0 + x, y0 + y) of the whole photo, which is full_w x
/// full_h at the part's scale.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub x0: f32,
    pub y0: f32,
    pub full_w: f32,
    pub full_h: f32,
}

impl Frame {
    /// Pixel `i` of a part `w` wide, as fractions (0..1) of the whole photo.
    pub fn frac(&self, i: usize, w: usize) -> (f32, f32) {
        (((i % w) as f32 + 0.5 + self.x0) / self.full_w, ((i / w) as f32 + 0.5 + self.y0) / self.full_h)
    }
}

/// What a whole-photo render measured that depends on the whole photo (the
/// tone map, the brightness statistics, Clarity's and Dehaze's measures), so
/// a part rendered later at full resolution comes out the same.
#[derive(Clone, Debug, Default)]
pub struct Measured {
    pub tone_map: Option<ToneMap>,
    pub stats: Option<crate::tonetable::PhotoStats>,
    pub definition: Option<crate::tools::DefinitionMeasured>,
}

impl Measured {
    /// Whether it holds everything these settings need for a part of the
    /// photo to come out as in the whole (the tone map and statistics for the
    /// tone sliders, Clarity's and Dehaze's measures).
    pub fn covers(&self, s: &RenderSettings) -> bool {
        let tuning = s.profile.tuning();
        let tone = !ToneMap::wanted(s, &tuning) || self.tone_map.is_some();
        // As render_core_q decides it needs the photo's statistics.
        let any = s.highlights != 0.0 || s.shadows != 0.0 || s.whites != 0.0 || s.blacks != 0.0;
        let table = tuning.tone_table > 0.5 && any;
        let adapt = !table && tuning.tone_adapt != 0.0 && (s.highlights != 0.0 || s.shadows != 0.0 || s.whites > 0.0);
        let stats = !(table || adapt) || self.stats.is_some();
        let def = (s.tools.clarity == 0.0 && s.tools.dehaze == 0.0)
            || self.definition.as_ref().is_some_and(|d| d.covers(&s.tools, &tuning.tools));
        tone && stats && def
    }
}

/// A part of the photo to render (zoomed-in detail).
#[derive(Clone, Debug)]
pub struct Region {
    pub frame: Frame,
    pub measured: std::sync::Arc<Measured>,
}

/// Settings the engine applies. Grows as more sliders move into the engine.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettings {
    /// Stops, applied as a linear gain on scene light.
    pub exposure: f32,
    /// Objects removed (`removal`), blended into the scene before everything else.
    #[serde(default)]
    pub removals: Vec<crate::removal::Spot>,
    /// Linear and radial gradients (local adjustments, `gradient`).
    #[serde(default)]
    pub gradients: Vec<crate::gradient::Gradient>,
    /// The photo's subject mask, for its subject masks (`gradient::Kind::Subject`),
    /// filled in by the renderer.
    #[serde(skip)]
    pub subject_mask: Option<std::sync::Arc<crate::gradient::Grid>>,
    /// Rendering just a part of the photo (zoomed-in detail): where it sits
    /// and what was measured on the whole photo. None: the whole photo.
    #[serde(skip)]
    pub region: Option<std::sync::Arc<Region>>,
    /// While a slider moves: what the photo's last full render measured for Texture, Clarity and
    /// Dehaze (their network's gain map, haze), reused for the quick frames instead of measured
    /// again (Clarity's network alone took half a second a frame). The full render after measures.
    #[serde(skip)]
    pub reuse_measured: Option<std::sync::Arc<Measured>>,
    /// Where a whole-photo render leaves what it measured, for a later part.
    #[serde(skip)]
    pub measure_into: Option<std::sync::Arc<std::sync::Mutex<Option<Measured>>>>,
    /// White balance in Kelvin, as Lightroom shows it for RAWs. None = as shot.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Tint in Lightroom units (positive = magenta). None = as shot.
    #[serde(default)]
    pub tint: Option<f32>,
    /// -100..100. Contrast around mid-grey in stops (see `contrast`).
    #[serde(default)]
    pub contrast: f32,
    /// -100..100 each. Tone-band adjustments (see `tone_bands`).
    #[serde(default)]
    pub highlights: f32,
    #[serde(default)]
    pub shadows: f32,
    #[serde(default)]
    pub whites: f32,
    #[serde(default)]
    pub blacks: f32,
    /// -100..100 each, Lightroom-style (see `colour_adjust`).
    #[serde(default)]
    pub vibrance: f32,
    #[serde(default)]
    pub saturation: f32,
    /// The profile: tone curve, colour character and baseline exposure.
    #[serde(default)]
    pub profile: Profile,
    /// A profile the user added (`userprofiles`: its file in RapidOne's
    /// profiles folder): used in place of `profile` for the cameras it covers.
    #[serde(default)]
    pub user_profile: Option<String>,
    /// Shinobi Colour's colour correction for this camera model (`camera_fix`),
    /// applied first to scene light. Filled in by the engine's caller, not the UI.
    #[serde(skip)]
    pub camera_fix: Option<[[f32; 3]; 3]>,
    /// This camera model's Shadow tint strength (`adobe::shadow_tint_scale`).
    /// Filled in by the engine's caller, not the UI.
    #[serde(skip)]
    pub shadow_tint_scale: Option<f32>,
    /// Lens profile vignetting correction, 0..1 (Lightroom's Lens Corrections
    /// Vignetting / 100; 0 = off). Applied by `commands::render_image`.
    #[serde(default)]
    pub lens_vignette: f32,
    /// Lens corrections from the Lensfun database (`lensfun.rs`): on above 0 (the
    /// photo's Lens button). `lens_id`: the lens chosen, 0 = found from the photo's EXIF.
    #[serde(default)]
    pub lens_correct: f32,
    #[serde(default)]
    pub lens_id: f64,
    /// Crop, straighten and quarter turns, on the finished picture (`crop.rs`).
    #[serde(default)]
    pub crop: crate::crop::Crop,
    /// Keystone (Vertical, Horizontal, Rotate, Scale), before the crop (`keystone.rs`).
    #[serde(default)]
    pub keystone: crate::keystone::Keystone,
    /// Fine tuning, Mixer, Curves, Grade and Vignette (see `tools`).
    #[serde(flatten)]
    pub tools: crate::tools::ToolSettings,
}

/// RapidOne's own profiles. Each is our own tone curve and colour character,
/// tuned by measuring Lightroom's renderings (`examples/standardfit.rs`):
/// Shinobi Standard sits close to Adobe Standard, Shinobi Colour close to
/// Adobe Color (Lightroom's default, so ours too).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Profile {
    ShinobiStandard,
    #[default]
    ShinobiColour,
    /// Adobe's own Adobe Color, read from the user's installed Adobe profiles
    /// (`crate::adobe`) and rendered by `render_adobe`; where they're missing
    /// it renders as Shinobi Colour.
    AdobeColor,
    /// Adobe's own Adobe Standard (the camera profile without Adobe Color's
    /// look), from the same installed files; where they're missing it renders
    /// as Shinobi Standard. For measuring the engine against Lightroom.
    AdobeStandard,
    /// For photos that are already toned (a TIFF or JPEG, including the TIFF
    /// Retouch hands back): no baseline exposure, tone curve or colour look,
    /// so the picture is as it is until a slider moves. Chosen by the
    /// engine for those files, never by the user.
    AsIs,
}

/// A profile's numbers.
struct ProfileLook {
    /// Baseline exposure in stops for Panasonic, Fujifilm, Nikon, Canon, Sony,
    /// then any other make. Cameras record RAWs darker than their JPEGs to protect
    /// highlights, and every RAW developer brightens them back by a
    /// per-camera "baseline exposure".
    baseline: [f32; 6],
    curve: [f32; 5],
    look_chroma: [f32; 5],
    look_hue: [f32; 4],
    look_light: [f32; 4],
    look_comp: f32,
    look_lut: [[f32; 3]; 12],
}

impl Profile {
    fn look(self) -> ProfileLook {
        match self {
            // No baseline, an identity tone curve (all zeros: `ToneCurve::with`)
            // and no colour character (the values `has_look` treats as none).
            Profile::AsIs => ProfileLook {
                baseline: [0.0; 6],
                curve: [0.0; 5],
                look_chroma: [1.0, 0.0, 0.0, 0.0, 0.0],
                look_hue: [0.0; 4],
                look_light: [0.0; 4],
                look_comp: 0.0,
                look_lut: [[0.0; 3]; 12],
            },
            // Fitted 2026-10-06 against Lightroom's Adobe Standard exports of
            // five photos (mean dE 1.84). Refitted 2026-10-10 on Chris's 70-photo
            // Adobe Standard kit (`RapidOne-calibration-cameras-standard`: S5 II,
            // S5, GFX 100S, Z 6, D750, D7100), with per-model offsets and the
            // per-camera corrections, adding a 12-point colour table: held-out
            // half 2.61 -> 2.05 mean dE; all 70: 2.44 -> 1.81. Canon/Sony/other
            // per-make values not covered by the kit stay as they were.
            // Cyan to blue turned 6-10 degrees back toward teal (2026-10-10):
            // Lightroom's cyans sat 5.6 degrees greener than ours over the 70
            // Adobe Standard photos and the greens kit; now 0.5. High-key presets
            // 3.12 -> 3.10 (flash), kit 2.38 -> 2.41.
            Profile::ShinobiStandard | Profile::AdobeStandard => ProfileLook {
                baseline: [0.759, 0.973, 1.136, 1.050, 1.050, 1.05],
                curve: [2.7508, 0.0567, 2.4488, 0.3947, 0.1235],
                look_chroma: [0.866, 0.047, 0.026, -0.023, -0.004],
                look_hue: [-3.0, -1.25, 1.06, 0.25],
                look_light: [-0.016, -0.04, -0.098, 0.024],
                look_comp: 1.0,
                look_lut: [[4.125, 0.083, 0.135], [2.25, 0.032, 0.142], [-0.5, 0.009, 0.029], [-0.016, -0.046, 0.021], [2.688, 0.027, 0.022], [-0.5, 0.142, 0.244], [-6.0, 0.112, -0.019], [-5.5, 0.112, -0.079], [-1.0, 0.038, 0.139], [-1.0, 0.029, 0.112], [1.5, 0.077, 0.101], [1.875, 0.0, 0.142]],
            },
            // Adobe Color, refitted 2026-10-07 on 70 photos (19 test photos and 51 of the user's own):
            // mean dE 2.51 (2.92 before), with a fitted 12-point colour table. Refitted 2026-10-08 with
            // 18 round-4 bases added (88 photos, 4 more cameras): 2.574 -> 2.542; hold-out checked (new
            // photos the fit didn't see: 2.80 -> 2.72 and 2.85 -> 2.75). Per-make exposures refitted
            // with per-model offsets added (`baseline_for`): 2.542 -> 2.449. Refitted again
            // 2026-10-09 with Fuji's per-photo exposure bias and 52 more models: 2.087.
            Profile::ShinobiColour | Profile::AdobeColor => ProfileLook {
                baseline: [0.731, 0.974, 1.079, 1.004, 1.125, 1.05],
                curve: [3.1791, 0.0132, 2.2179, 0.5795, 0.1044],
                look_chroma: [0.933, -0.009, 0.044, -0.011, 0.013],
                look_hue: [1.19, -0.24, 1.04, -2.13],
                look_light: [-0.052, -0.026, 0.016, 0.124],
                look_comp: 0.000,
                look_lut: [[0.75, -0.019, 0.11], [0.0, 0.013, 0.004], [-4.0, 0.015, 0.102], [-2.188, -0.066, -0.043], [3.484, 0.023, 0.121], [-0.375, 0.097, 0.159], [-2.25, -0.106, 0.312], [2.813, -0.004, 0.334], [4.125, -0.037, 0.033], [2.25, -0.007, 0.233], [1.25, -0.007, 0.443], [-4.0, 0.03, 0.304]],
            },
        }
    }

    pub fn baseline_exposure(self, make: &str) -> f32 {
        let b = self.look().baseline;
        match make.to_ascii_lowercase().as_str() {
            m if m.starts_with("panasonic") => b[0],
            m if m.starts_with("fuji") => b[1],
            m if m.starts_with("nikon") => b[2],
            m if m.starts_with("canon") => b[3],
            m if m.starts_with("sony") => b[4],
            _ => b[5],
        }
    }

    /// The per-make baseline plus this model's own offset (Lightroom's starting
    /// brightness is per model; measured on the Adobe Color path). Shinobi
    /// Colour's per-make values were refitted with the offsets (88 photos:
    /// mean dE 2.542 -> 2.449); Shinobi Standard's too (2026-10-10, 70 photos).
    pub fn baseline_for(self, make: &str, camera: &str) -> f32 {
        if self == Profile::AsIs {
            return 0.0;
        }
        self.baseline_exposure(make) + crate::adobe::model_offset(make, camera)
    }

    /// The slider tuning with this profile's curve and colour character.
    pub fn tuning(self) -> Tuning {
        let l = self.look();
        // The profile's own part: its tone curve and colour character (as Adobe
        // Standard / Adobe Color are a camera profile and look). Everything else -
        // the sliders, their order and interactions, and the shown look - is the
        // engine's, shared with Adobe's profiles (`Tuning::adobe`), so a preset
        // behaves the same on either (Chris, 2026-10-14: like for like).
        let own = if self == Profile::ShinobiColour { Tuning::shinobi_colour() } else { Tuning::default() };
        // "As is": the sliders' tuning only, with none of the shown look
        // (display curves, the yellow boost) on top, so an untouched pixel is unchanged.
        let shown_look = |t: Tuning| if self == Profile::AsIs { t.without_look() } else { t };
        shown_look(Tuning {
            curve: l.curve,
            curve_pp: if self == Profile::AsIs { 0.0 } else { own.curve_pp },
            look_chroma: l.look_chroma,
            look_hue: l.look_hue,
            look_light: l.look_light,
            look_comp: l.look_comp,
            look_lut: l.look_lut,
            look_warm: if self == Profile::AsIs { [[0.0; 2]; 5] } else { own.look_warm },
            look_pp: own.look_pp,
            look_hsv: own.look_hsv,
            look_hsv_dark: own.look_hsv_dark,
            ..Tuning::adobe()
        })
    }

    /// The tuning as it was before the engine was shared (each Shinobi profile
    /// with its own slider tuning), for comparison.
    pub fn tuning_own(self) -> Tuning {
        let l = self.look();
        let base = if self == Profile::ShinobiColour { Tuning::shinobi_colour() } else { Tuning::default() };
        Tuning { curve: l.curve, look_chroma: l.look_chroma, look_hue: l.look_hue, look_light: l.look_light, look_comp: l.look_comp, look_lut: l.look_lut, ..base }
    }
}

/// Shinobi Colour's per-model colour correction: a white-preserving matrix on
/// scene light (each row sums to 1), fitted to Lightroom's Adobe Color exports
/// (`standardfit FIT=model`) for models with several varied photos. Hold-out
/// checked (half each model's photos fitted, half scored): Z 6 2.40 -> 2.34,
/// GFX 100S 2.53 -> 2.49, D750 4.54 -> 3.52, D610 3.87 -> 3.05 mean dE. Not
/// the D810 (three stock samples disagreed with the user's own) or the S5 II
/// (already 1.8, no gain). Other models: none.
pub fn camera_fix(camera: &str) -> Option<[[f32; 3]; 3]> {
    let m: [f32; 6] = match camera {
        "Fujifilm GFX 100S" => [-0.016, 0.014, -0.001, -0.007, 0.005, 0.0],
        "Nikon Z 6" => [0.02, 0.031, -0.018, 0.04, -0.002, -0.055],
        // D750 refitted 2026-10-09 on 15 photos (12 from the per-camera kit): unseen
        // halves 2.62 -> 2.52 and 1.86 -> 1.82 (the old 3-photo values made them worse).
        "Nikon D750" => [0.057, 0.024, -0.005, 0.033, -0.012, -0.003],
        "Nikon D610" => [-0.043, -0.037, 0.023, -0.038, -0.025, -0.031],
        // From the shadow tint kits' bases (CC0 raw.pixls.us samples, two
        // different scenes per model, each held out in turn; `FIT=models`):
        // only models whose correction stayed small (every value within 0.15)
        // and helped the unseen photo.
        "Canon EOS 90D" => [0.011, -0.012, -0.004, 0.022, 0.003, 0.011],
        "Canon EOS R6" => [0.091, 0.003, -0.056, 0.011, -0.018, -0.006],
        "Canon EOS RP" => [-0.114, -0.055, -0.006, -0.114, -0.002, 0.025],
        "Nikon D5600" => [0.115, 0.008, 0.099, 0.049, -0.059, 0.148],
        "Nikon D7500" => [-0.041, 0.035, -0.038, 0.063, 0.024, -0.067],
        "Olympus E-M1MarkIII" => [-0.103, 0.0, 0.037, -0.04, -0.002, -0.034],
        _ => return None,
    };
    Some([[1.0 - m[0] - m[1], m[0], m[1]], [m[2], 1.0 - m[2] - m[3], m[3]], [m[4], m[5], 1.0 - m[4] - m[5]]])
}

/// The default profile's (Shinobi Colour's) baseline exposure. The slider
/// fitting tools work on the default profile against Lightroom's Adobe Color
/// kits (also `Tuning::default()`'s curve and colour).
pub fn baseline_exposure(make: &str) -> f32 {
    Profile::default().baseline_exposure(make)
}

pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// A tone band: a smooth bump over a range of stops (relative to mid-grey).
fn bump(ev: f32, centre: f32, width: f32) -> f32 {
    (-0.5 * ((ev - centre) / width).powi(2)).exp()
}

/// Whites as a global curve (`Tuning::wb_curve`): `e` in stops over mid-grey.
fn whites_curve(e: f32, amount: f32, t: &Tuning) -> f32 {
    let a = (amount.abs() / 100.0).min(1.0);
    if a == 0.0 {
        return e;
    }
    if amount < 0.0 {
        let [k, p, b, c0, c1] = t.whites_minus_curve;
        e - k * a.powf(p) * (b * e).exp() / (1.0 + (b * (e - c0 - c1 * a)).exp())
    } else {
        let [k, p, b, b2, c0, c1] = t.whites_plus_curve;
        e + k * a.powf(p) * (b * e).exp() / (1.0 + (b2 * (e - c0 - c1 * a)).exp())
    }
}

/// Blacks as a global curve (`Tuning::wb_curve`): `e` in stops over mid-grey.
fn blacks_curve(e: f32, amount: f32, t: &Tuning) -> f32 {
    let a = (amount.abs() / 100.0).min(1.0);
    if a == 0.0 {
        return e;
    }
    let x = 0.18 * 2f32.powf(e);
    let y = if amount < 0.0 {
        let [k, p, soft] = t.blacks_minus_curve;
        let y = x - k * a.powf(p);
        (y + (y * y + soft * soft).sqrt()) / 2.0
    } else {
        let [k, p, q, x0, w] = t.blacks_plus_curve;
        let s = x.powf(q) / (x.powf(q) + x0.powf(q));
        x + k * a.powf(p) * s * (1.0 - x / w).max(0.0)
    };
    (y.max(1e-9) / 0.18).log2()
}

/// Photo-adaptive Highlights (`rising`) or Shadows curve (`Tuning::hs_adaptive`).
fn hs_curve(e: f32, amount: f32, m: &[f32; 11], st: &[f32; 4], rising: bool) -> f32 {
    if amount == 0.0 {
        return 0.0;
    }
    let a = (amount.abs() / 100.0).min(1.0);
    let [k, p, b, b2, c, w1, w2, w3, w4, m1, m2] = *m;
    let anchor = w1 * st[0] + w2 * st[1] + w3 * st[2] + w4 * st[3];
    let u = if rising { e - anchor } else { anchor - e };
    // Hold the curve at its peak beyond it: the data stop short of white (clipped
    // pixels are left out), and a slider never weakens towards the end it works on.
    // Shadows ease off at least as fast as they rise (b2 = b + |b2|: a plateau at
    // worst), so the deepest blacks aren't lifted without limit.
    let b2a = if rising { b2.abs() } else { b + b2.abs() };
    let u = if rising && b > 0.0 && b2a > b { u.min(c + (b / (b2a - b)).ln() / b2a) } else { u };
    let amp = k.abs() * a.powf(p.abs()) * 2f32.powf(m1 * (st[3] - st[1]) + m2 * st[0]);
    let g = amp * (b * u).exp() / (1.0 + (b2a * (u - c)).exp());
    amount.signum() * g.min(4.0)
}

/// Highlights' band weight at a position: the bell, with `highlights_tail` below it.
fn highlights_weight(p: f32, t: &Tuning) -> f32 {
    let b = bump(p, t.highlights_centre, t.highlights_width);
    if t.highlights_tail > 0.0 && p < t.highlights_centre {
        b.max(t.highlights_tail * ((p - t.highlights_centre) / t.highlights_tail_width.max(0.1)).exp())
    } else {
        b
    }
}

/// Monotone cubic (Fritsch-Carlson) through points sorted by x, on 0..1.
fn monotone_curve(v: f32, p: &[(f32, f32)]) -> f32 {
    let n = p.len();
    let x = v.clamp(0.0, 1.0);
    let d: Vec<f32> = (0..n - 1).map(|i| (p[i + 1].1 - p[i].1) / (p[i + 1].0 - p[i].0)).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { 2.0 / (1.0 / d[i - 1] + 1.0 / d[i]) };
    }
    let i = (0..n - 1).find(|&i| x < p[i + 1].0).unwrap_or(n - 2);
    let h = p[i + 1].0 - p[i].0;
    let t = (x - p[i].0) / h;
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * p[i].1 + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * p[i + 1].1 + (t3 - t2) * h * m[i + 1]
}

/// Monotone cubic (Fritsch-Carlson) through (0,0), (x1,y1), (x2,y2), (1,1).
fn display_curve(v: f32, c: &[f32; 4]) -> f32 {
    let xs = [0.0, c[0], c[2], 1.0];
    let ys = [0.0, c[1], c[3], 1.0];
    let x = v.clamp(0.0, 1.0);
    let d: [f32; 3] = std::array::from_fn(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]));
    let mut m = [d[0], 0.0, 0.0, d[2]];
    for i in 1..3 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { 2.0 / (1.0 / d[i - 1] + 1.0 / d[i]) };
    }
    let i = if x < xs[1] { 0 } else if x < xs[2] { 1 } else { 2 };
    let h = xs[i + 1] - xs[i];
    let t = (x - xs[i]) / h;
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * ys[i] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * ys[i + 1] + (t3 - t2) * h * m[i + 1]
}

/// A smooth step from 0 to 1 between two points in stops.
fn ramp(ev: f32, from: f32, to: f32) -> f32 {
    let t = ((ev - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The strengths and ranges of the tone sliders. Defaults are the values
/// fitted to Lightroom Classic's responses (see `examples/lrmatch.rs`); the
/// measuring tool can override them to search for better ones. Lightroom's
/// sliders aren't symmetric (Whites pushes harder up than down, for example),
/// so each direction has its own strength.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Contrast: how many stops one stop away from grey becomes at ±100.
    pub contrast_plus: f32,
    pub contrast_minus: f32,
    /// Contrast's fixed point, in stops over mid-grey; how strongly the bright
    /// side moves relative to the dark side; and where (stops over grey) the
    /// effect fades out towards white. See `contrast`.
    pub contrast_pivot: f32,
    pub contrast_bright: f32,
    pub contrast_fade_from: f32,
    pub contrast_fade_to: f32,
    /// Contrast's toe below the pivot (stops): the slope eases back to 1 from
    /// `contrast_toe_from` to `contrast_toe_to` (to <= from: no toe).
    pub contrast_toe_from: f32,
    pub contrast_toe_to: f32,
    /// Lightroom's Contrast also changes colour strength (measured on the Adobe
    /// Color tone kit: against our colour-keeping Contrast, Lightroom's chroma
    /// is x1.25 at +50, x1.5 at +100, x0.84 at -50): chroma scale
    /// 1 + amount/100 x `contrast_sat_plus` (or `_minus` below 0).
    pub contrast_sat_plus: f32,
    pub contrast_sat_minus: f32,
    /// 1 = Contrast applied as Adobe applies tone curves (the brightest and
    /// darkest channel through it, the middle one kept in proportion), which
    /// changes colour strength the way a curve does; 0 = on luminance only;
    /// 2 = the same on linear ProPhoto channels; 3 = each linear ProPhoto
    /// channel through the curve on its own (Adobe Color: refitted on the
    /// contrast, tone2 and combo kits, 70.9% vs 70.8% for a luminance refit;
    /// real edits 4.58 -> 4.53, contrast kit 3.10 -> 2.82, food flash 3.71
    /// -> 3.54 but indoor 2.57 -> 2.90). Shinobi Standard keeps 0 (45.6% vs 49.4%).
    pub contrast_rgb: f32,
    /// Contrast's colour change eases off for colours already strong: its
    /// share is `contrast_sat_soft` / (that + OKLab chroma); 0 = the same for all.
    /// Compared on the contrast kit and Chris's six presets (mean dE): none
    /// 2.77 / 3.21, full for every colour 1.49 / 3.74, half 1.88 / 2.94,
    /// curve on RGB (`contrast_rgb`) 1.57 / 3.45, easing off at 0.06 1.74 / 2.96.
    pub contrast_sat_soft: f32,
    /// Highlights / Shadows: shift in stops at +100 and -100, band centre and
    /// width (stops from grey).
    pub highlights_plus: f32,
    pub highlights_minus: f32,
    pub highlights_centre: f32,
    pub highlights_width: f32,
    pub shadows_plus: f32,
    pub shadows_minus: f32,
    pub shadows_centre: f32,
    pub shadows_width: f32,
    /// Whites: shift in stops at ±100, over a ramp between two points (stops
    /// from grey). Lightroom's + side reaches further into the mids than its
    /// - side, so each side has its own ramp.
    pub whites_plus: f32,
    pub whites_minus: f32,
    pub whites_from: f32,
    pub whites_to: f32,
    pub whites_minus_from: f32,
    pub whites_minus_to: f32,
    /// Blacks: measured from Lightroom, it behaves like a deeper, stronger
    /// Shadows - a hump peaking in the darkest tones - so it uses the same
    /// band shape: shift in stops at +100 and -100, centre and width.
    pub blacks_plus: f32,
    pub blacks_minus: f32,
    pub blacks_centre: f32,
    pub blacks_width: f32,
    /// Local, photo-adaptive tone sliders (Lightroom's Highlights / Shadows /
    /// Whites / Blacks act on areas, relative to the photo): each band is read
    /// at the pixel's brightness moved `tone_local` (Highlights, Shadows) or
    /// `tone_local_wb` (Whites, Blacks) of the way to its area's brightness -
    /// an edge-aware smoothing (guided filter, radius `tone_radius` of the long
    /// side, edge threshold `tone_eps` stops^2) - and shifted by `tone_key` of
    /// how far the photo's mean log brightness is from `tone_key_ref` stops.
    /// All 0 = the old global bands. Default: fully local (1, 1) over 2% of
    /// the long side: on Chris's six presets the edited renders had 80-87% of
    /// Lightroom's local contrast with global bands (unedited photos match:
    /// `examples/detail.rs`), 88-103% with these, and came closer overall
    /// (mean 3.32 -> 3.17 dE).
    pub tone_local: f32,
    pub tone_local_wb: f32,
    pub tone_radius: f32,
    pub tone_eps: f32,
    pub tone_key: f32,
    pub tone_key_ref: f32,
    /// `tone_key` for photos darker than `tone_key_ref` (0 = the same): the
    /// tone sliders follow a dark photo's brightness further than a bright one's.
    pub tone_key_dark: f32,
    /// How far below `tone_key_ref` (stops) the shift keeps growing (0 = no limit).
    pub tone_key_dark_span: f32,
    /// 1 = Highlights / Shadows / Whites / Blacks applied as Adobe applies tone
    /// curves, to the brightest and darkest linear ProPhoto channel (0 = on
    /// luminance, the colour scaled with it).
    pub tone_pp: f32,
    /// 1 = Vibrance / Saturation (and Contrast's colour change) scale the colour
    /// on linear ProPhoto (distance from the grey of the same luminance);
    /// 0 = OKLab chroma.
    pub sat_pp: f32,
    /// Positive Shadows strength grows as (amount / 100) ^ `shadows_pow` (1 =
    /// in proportion): Lightroom's +29 does far less than a third of +100.
    pub shadows_pow: f32,
    /// Positive Blacks' own band place and width (width 0 = the shared
    /// `blacks_centre` / `blacks_width`): Lightroom's +80 lifts the mid-tones
    /// nearly as much as the darks.
    pub blacks_plus_centre: f32,
    pub blacks_plus_width: f32,
    /// 1 = the profile tone curve on linear ProPhoto channels (brightest and
    /// darkest through it), as Adobe applies a profile's curve; 0 = on Rec.2020.
    pub curve_pp: f32,
    /// How much the bands stretch with the photo's own brightness range (its
    /// 2nd to 98th percentile, in stops) against `tone_span_ref` stops: 0 =
    /// fixed places, 1 = places scaled fully to the photo's range.
    pub tone_span: f32,
    pub tone_span_ref: f32,
    /// 1 = Highlights / Shadows / Whites / Blacks from the measured table
    /// (`tonetable`, on displayed lightness, strength adapted to the photo);
    /// 0 = the older fitted bands above (the default). The table wins on the
    /// single-slider kit (`RapidOne-calibration-tone2`: Shadows 0 -> 60%,
    /// Blacks 35 -> 80%, Chris's four together 43 -> 64%) but loses on Chris's
    /// real presets (food 2.1-3.2 -> 3.1-5.6 dE), which add big Exposure
    /// moves and curves: it needs a kit of combinations before it's the default.
    pub tone_table: f32,
    /// Photo-adaptive strength for the bands (as measured on the tone kit):
    /// Highlights x exp(-0.041 (median L* - 41.5)), Shadows x exp(0.025 (75th
    /// percentile - 59.4)), Whites + x exp(-0.05 (98th percentile - 88.2)),
    /// the exponent times `tone_adapt` (0 = fixed strength), each factor
    /// within 0.4..2.5. The photo is measured without these four sliders, and
    /// without Exposure when `tone_adapt_no_exposure` is 1. Off by default:
    /// on Chris's six presets it made things worse (2.96 -> 3.3-5.2 mean dE):
    /// the single-slider kit's rule doesn't carry to edits with big Exposure
    /// moves; it needs the combination kit.
    pub tone_adapt: f32,
    pub tone_adapt_no_exposure: f32,
    /// The adaptive rules' slopes per L* of the photo measure (Highlights by
    /// median, Shadows by 75th percentile, Whites + by 98th) and their centres.
    pub tone_adapt_h: f32,
    pub tone_adapt_s: f32,
    pub tone_adapt_w: f32,
    pub tone_adapt_hc: f32,
    pub tone_adapt_sc: f32,
    pub tone_adapt_wc: f32,
    /// The local tone map's working grid (cells along the long side): its
    /// finest detail. Lightroom's reaches down to ~0.3% of the long side
    /// (spatial chart: a 6 px disc in 2400 gets 37% of the full pull).
    pub tone_grid: f32,
    /// 1: the bright bands (Highlights, Whites) read the darker of a pixel
    /// and its area, the dark ones (Shadows, Blacks) the brighter - so being
    /// local only ever weakens a slider, never turns it on for the opposite
    /// kind of pixel. Spatial chart: Lightroom pulls a 6 px bright disc on
    /// dark with Highlights at 37% and lifts it with Shadows not at all.
    pub tone_minmax: f32,
    /// How much of a pixel's colour (its linear difference from grey) the
    /// tone sliders keep while they change its brightness: 0 = scaled with it
    /// (colour fades when a slider darkens), 1 = kept. Lightroom's Blacks -47
    /// on the drinks photo changes colour strength by 0.1, ours by 1.5.
    pub tone_chroma: f32,
    /// Negative Whites' ramp raised to this power (1 = the plain ramp; below 1
    /// it reaches further down into the mid-tones, as Lightroom's does: on the
    /// high key ladder its Whites -53 moved the middle grey 4-5 L*, ours 2).
    pub whites_minus_pow: f32,
    /// Positive Shadows' band width below its centre (0 = `shadows_width`):
    /// narrower lifts the deepest tones less (Lightroom's +29 lifted the
    /// darkest tenth of the high key photos about half as much as ours).
    pub shadows_plus_width_lo: f32,
    /// 1 = Contrast after Highlights / Shadows / Whites / Blacks, 0 = before them,
    /// 2 = Whites / Blacks, then Contrast, then Highlights / Shadows.
    /// Tested 2026-10-13 (refitted on all tone kits): held out 70.0% vs 69.5%,
    /// high key flash 6.38 vs 5.36 - not kept.
    pub contrast_after_tone: f32,
    /// Whites' band follows the photo's brightness by this much more than the
    /// other bands (stops per stop of mean log brightness from `tone_key_ref`):
    /// Lightroom's Whites -50 reaches further into the mid-tones of bright
    /// photos and less into dark ones (Standard kit and high key ladder).
    pub whites_key: f32,
    /// Whites' strength grows with positive Contrast: x (1 + this x Contrast/100).
    /// Lightroom's Whites -53 moved the high key photos' middle grey 1.5x as far
    /// with Contrast +85 on; ours didn't change.
    pub whites_contrast: f32,
    /// 1 = Whites and Blacks as fixed global curves of each pixel's light (no
    /// photo or area dependence), measured from Lightroom's exports with the
    /// profile's own curve taken out (`examples/slidercurve.rs`, 2026-10-13):
    /// their curves agreed across 12 photos within 0.03 stops and were the same
    /// through Adobe Standard and Adobe Color. a = |amount| / 100, L = stops over
    /// mid-grey, x = 0.18 x 2^L:
    /// - Whites -: L -= k a^p e^(bL) / (1 + e^(b(L - c0 - c1 a)))  [k, p, b, c0, c1]
    /// - Whites +: L += k a^p e^(bL) / (1 + e^(b2(L - c0 - c1 a)))  [k, p, b, b2, c0, c1]
    /// - Blacks -: x -> softfloor(x - k a^p, e)  [k, p, e] (a near-constant subtraction)
    /// - Blacks +: x += k a^p x^q / (x^q + x0^q) (1 - x/W)  [k, p, q, x0, W]
    pub wb_curve: f32,
    /// 1 = Highlights and Shadows each with their own locality and photo-
    /// brightness following (`highlights_local` / `_key`, `shadows_local` /
    /// `_key`) instead of the shared `tone_local` / `tone_key`. Measured
    /// (`slidercurve AREA=`, 12 photos): Lightroom's Shadows +40 follows the
    /// photo's mean brightness almost fully and its surroundings not at all;
    /// Highlights -60 follows it about half and is a little local.
    pub hs_own: f32,
    /// Whole-image parts of Highlights / Shadows, by the photo's brightness
    /// (mean log, `key`): every pixel moves by strength x `highlights_global` x
    /// ramp(key, from, from + width) (bright photos), or strength x
    /// `shadows_global` x (1 - ramp(key, from, from + width)) (dark photos).
    /// Lightroom's Highlights -100 on brightened photos (combo kit, Exposure
    /// +1.4) darkened even the deep shadows 0.03-0.07 stops, and -85 on the
    /// food indoor (+1.43) 0.1-0.3, with a tight spread (a global pull).
    /// Highlights' tail into the darks: below its centre the band's weight is
    /// at least `highlights_tail` x e^((pos - centre) / `highlights_tail_width`)
    /// (0 = the plain bell). Lightroom's Highlights -100 darkened the shadows of
    /// all 12 brightened-kit photos 0.03-0.15 stops (Z 6: -0.06 at -7.8 stops);
    /// a bell can't reach there.
    /// 1 = Highlights and Shadows as photo-adaptive global curves (no local
    /// part): measured per photo from Lightroom's exports (`slidercurve`, 102
    /// photo/amount pairs over the Standard, brightened, Adobe Color tone kits and
    /// the food ladder), one curve shape each that Lightroom places and
    /// scales from the photo's own brightness distribution. With a free
    /// position and strength per photo the photos agreed within 0.02-0.03 stops;
    /// this formula: Highlights - 0.086 rms (held-out set 0.077), Shadows + 0.067
    /// (0.115). Per sign, [k, p, b, b2, c, w1, w2, w3, w4, m1, m2]:
    /// u = (L - (w1 mean + w2 p50 + w3 p90 + w4 p99.5)) (Shadows: negated),
    /// gain = sign k a^p 2^(m1 (p99.5 - p50) + m2 mean) e^(bu) / (1 + e^(b2(u - c))).
    pub hs_adaptive: f32,
    pub highlights_minus_model: [f32; 11],
    pub highlights_plus_model: [f32; 11],
    pub shadows_plus_model: [f32; 11],
    pub shadows_minus_model: [f32; 11],
    /// The photo's brightness statistics for `hs_adaptive` (stops over mid-grey:
    /// mean, p50, p90, p99.5 of each pixel's light before the tone sliders), set
    /// per photo at render time.
    pub photo_stats: [f32; 4],
    /// Shadows' lift x (1 - this x max(0, -Highlights) / 100): in Lightroom a
    /// negative Highlights weakens Shadows (slider-mix kit: Highlights -85 cut
    /// Shadows +44's lift in the darks about 40%; food ladder about 55%).
    pub shadows_highlights_link: f32,
    pub highlights_tail: f32,
    pub highlights_tail_width: f32,
    pub highlights_global: f32,
    pub highlights_global_from: f32,
    pub highlights_global_width: f32,
    pub shadows_global: f32,
    pub shadows_global_from: f32,
    pub shadows_global_width: f32,
    pub highlights_local: f32,
    pub highlights_key: f32,
    pub shadows_local: f32,
    pub shadows_key: f32,
    pub whites_minus_curve: [f32; 5],
    pub whites_plus_curve: [f32; 6],
    pub blacks_minus_curve: [f32; 3],
    pub blacks_plus_curve: [f32; 5],
    /// Contrast's pivot follows the photo's own brightness: it moves by
    /// `contrast_key` of how far the mean log brightness is from `tone_key_ref`
    /// (Lightroom pivots lower on dark photos; 0 = a fixed pivot).
    pub contrast_key: f32,
    /// Detail up to `tone_texture` stops from the small copy's brightness (read
    /// bilinearly) is left out of the tone map's area brightness, so fine detail
    /// (skin texture) never enters it and the tone sliders keep it, as
    /// Lightroom does, while larger steps (edges) stay sharp; 0 = off.
    pub tone_texture: f32,
    /// Saturation: chroma scale at +100 is 1 + `saturation_plus` (times the
    /// hue weight); at -100
    /// it's 1 - `saturation_minus` (1.0 = black and white).
    pub saturation_plus: f32,
    pub saturation_minus: f32,
    /// How a boost's strength varies with hue (see `hue_weight`): Lightroom
    /// boosts oranges more than blues with Saturation.
    pub saturation_hue: [f32; 4],
    /// Vibrance: like saturation at ±100, but scaled down for colours that
    /// are already strong (half strength at chroma `vibrance_soft`), shaped
    /// by hue (`vibrance_hue`: little for skin, a lot for blues), and curved
    /// for boosts - +50 does (0.5^`vibrance_curve`) of +100.
    pub vibrance_plus: f32,
    pub vibrance_minus: f32,
    pub vibrance_soft: f32,
    pub vibrance_hue: [f32; 4],
    pub vibrance_curve: f32,
    /// RapidOne Standard's tone curve coefficients (see `filmic`).
    pub curve: [f32; 5],
    /// RapidOne Standard's colour character: a chroma scale around the hue
    /// circle (overall scale, then two harmonics as in `hue_weight`) and a
    /// hue shift in degrees (two harmonics), applied after the tone curve.
    pub look_chroma: [f32; 5],
    pub look_hue: [f32; 4],
    /// Lightness shift by hue (two harmonics, OKLab L per unit chroma) and a
    /// compression of strong colours (chroma / (1 + look_comp * chroma)).
    pub look_light: [f32; 4],
    pub look_comp: f32,
    /// A finer colour table: at 12 OKLab hues (every 30 degrees from a=+,
    /// b=0) a hue shift (degrees), a chroma change (fraction) and a lightness
    /// shift per unit chroma, interpolated around the circle. Fitted to
    /// Lightroom's renders, not taken from any profile file.
    pub look_lut: [[f32; 3]; 12],
    /// Warm colours (OKLab hue 85 +- 55 degrees) by lightness: at OKLab L 0,
    /// 0.25, 0.5, 0.75 and 1, [lightness per unit chroma, chroma factor - 1].
    pub look_warm: [[f32; 2]; 5],
    /// 1 = the colour character as `look_hsv` on linear ProPhoto (see
    /// `look_hsv()`), replacing the OKLab look above; 0 = the OKLab look.
    pub look_pp: f32,
    pub look_hsv: [[f32; 3]; 12],
    /// `look_hsv` at black (all zero = `look_hsv` at every value).
    pub look_hsv_dark: [[f32; 3]; 12],
    /// A gentle S curve on the shown (sRGB-encoded) blue channel: y = x -
    /// `blue_s` x sin(2 pi x) / (2 pi), slope 1 - `blue_s` at the ends and
    /// 1 + `blue_s` at the middle (0 = none). Strengthens yellows (low blue).
    pub blue_s: f32,
    /// A master curve on the shown sRGB values (each channel), as an Affinity /
    /// Photoshop RGB curve: through (0,0), (x1,y1), (x2,y2), (1,1), smooth and
    /// monotone. All zero = off. Chris's subtle S, 2026-10-14: (0.248, 0.22),
    /// (0.739, 0.765).
    pub display_curve: [f32; 4],
    /// After it, a curve on the shown blue channel only, through (0,0), (x,y),
    /// (1,1) (zero = off). Chris's, 2026-10-14: (0.241, 0.263), cooling the warm
    /// indoor shadows and mid-tones.
    pub display_blue_curve: [f32; 2],
    /// Added to the Mixer's Yellow Saturation on every render (as if the slider
    /// were moved this much on top of the edit). Chris, 2026-10-14: +10.
    pub yellow_sat_extra: f32,
    /// Which half of `blue_s`: 0 = both (the S), 1 = the dark half only (blue
    /// lowered below the middle), 2 = the bright half only, 3 = only where blue
    /// is the weakest channel, scaled by saturation ((max - min) / max), so
    /// greys and whites are left alone; 4 = as 3, only for yellows and greens
    /// (green at least 85% of red, full from 100%), so skin, browns and
    /// oranges are left alone too.
    pub blue_s_half: f32,
    /// Adobe Color path only: a measured correction for where our render of
    /// Adobe's profile differs from Lightroom's, by CIELAB lightness (L* 0, 10 ..
    /// 100): lightness offset in L* and colour-strength factor (0 = none).
    pub adobe_fix_l: [f32; 11],
    pub adobe_fix_c: [f32; 11],
    /// Fine tuning, Mixer, Curves, Grade and Vignette.
    pub tools: crate::tools::ToolTuning,
}

impl Tuning {
    /// Without the shown look steps (`display_curve`, `display_blue_curve`,
    /// `yellow_sat_extra`), for fitting a profile to Lightroom's own exports.
    pub fn without_look(self) -> Tuning {
        Tuning { display_curve: [0.0; 4], display_blue_curve: [0.0; 2], yellow_sat_extra: 0.0, ..self }
    }
}

impl Default for Tuning {
    /// Tone sliders fitted on Shinobi Colour to Lightroom Classic 14.5 exports
    /// with real Adobe Color (PV 15.4) of a Panasonic S5II studio portrait,
    /// 2026-10-07 (`examples/lrmatch.rs`). Every slider at ±100 lands within
    /// ~0.5-1.7 L* of Lightroom's tone response (76-96% of its change).
    fn default() -> Self {
        Self {
            contrast_plus: 1.6180,
            contrast_minus: 0.6285,
            contrast_pivot: 0.2383,
            contrast_bright: 0.6066,
            // The fit found no benefit in fading out before white, so the
            // fade sits beyond any real highlight.
            contrast_fade_from: 8.0,
            contrast_fade_to: 10.0,
            contrast_toe_from: 0.0,
            contrast_toe_to: 0.0,
            contrast_sat_plus: 0.5,
            contrast_sat_minus: 0.32,
            contrast_rgb: 0.0,
            contrast_sat_soft: 0.06,
            highlights_plus: 0.819,
            highlights_minus: 0.9227,
            highlights_centre: 3.2836,
            highlights_width: 2.1694,
            shadows_plus: 1.93,
            shadows_minus: 2.015,
            shadows_centre: -3.156,
            shadows_width: 1.91,
            whites_plus: 1.713,
            whites_minus: 1.1358,
            whites_from: -3.175,
            whites_to: 7.55,
            whites_minus_from: -3.2541,
            whites_minus_to: 6.5291,
            blacks_plus: 0.8918,
            blacks_minus: 2.831,
            blacks_centre: -3.125,
            blacks_width: 2.082,
            tone_local: 1.0,
            tone_local_wb: 1.0,
            tone_radius: 0.02,
            tone_eps: 0.25,
            tone_key: 0.0,
            tone_key_ref: -1.5,
            tone_key_dark: 0.0,
            tone_key_dark_span: 0.0,
            tone_pp: 0.0,
            sat_pp: 1.0,
            look_warm: [[-0.606, -0.364], [0.0, 0.086], [0.031, -0.069], [-0.05, 0.053], [0.05, 0.142]],
            look_pp: 0.0,
            blue_s: 0.0,
            display_curve: [0.0; 4],
            display_blue_curve: [0.0; 2],
            yellow_sat_extra: 0.0,
            blue_s_half: 0.0,
            look_hsv: [[1.25, -0.058, 0.015], [-1.18, -0.067, 0.012], [-0.625, -0.109, 0.023], [3.063, -0.083, 0.045], [0.0, -0.075, 0.0], [-1.0, -0.058, -0.03], [0.5, -0.075, 0.0], [-0.781, -0.083, 0.008], [-0.5, -0.092, 0.019], [0.625, -0.041, 0.004], [-2.0, -0.174, -0.03], [5.0, -0.03, 0.0]],
            look_hsv_dark: [[1.125, -0.058, 0.015], [-1.68, -0.082, 0.012], [-0.625, -0.109, 0.053], [4.063, -0.083, 0.045], [0.0, -0.075, 0.0], [-1.0, -0.058, -0.03], [0.5, -0.075, 0.0], [-0.281, -0.083, 0.008], [-0.25, -0.092, 0.019], [-1.375, -0.071, 0.034], [3.0, -0.174, -0.03], [5.0, -0.03, 0.0]],
            shadows_pow: 1.0,
            blacks_plus_centre: -3.7969,
            blacks_plus_width: 3.5,
            curve_pp: 1.0,
            tone_span: 0.0,
            tone_span_ref: 8.0,
            tone_table: 0.0,
            tone_adapt: 0.0,
            tone_adapt_no_exposure: 1.0,
            tone_adapt_h: -0.041,
            tone_adapt_s: 0.025,
            tone_adapt_w: -0.05,
            tone_adapt_hc: 41.5,
            tone_adapt_sc: 59.4,
            tone_adapt_wc: 88.2,
            tone_grid: 128.0,
            tone_minmax: 0.0,
            tone_chroma: 0.0,
            whites_minus_pow: 1.0,
            shadows_plus_width_lo: 0.0,
            contrast_after_tone: 0.0,
            whites_key: 0.0,
            whites_contrast: 0.0,
            wb_curve: 0.0,
            hs_own: 0.0,
            hs_adaptive: 0.0,
            highlights_minus_model: [0.0; 11],
            highlights_plus_model: [0.0; 11],
            shadows_plus_model: [0.0; 11],
            shadows_minus_model: [0.0; 11],
            photo_stats: [-1.4, -1.4, 0.0, 1.5],
            shadows_highlights_link: 0.0,
            highlights_tail: 0.0,
            highlights_tail_width: 3.0,
            highlights_global: 0.0,
            highlights_global_from: -1.0,
            highlights_global_width: 2.0,
            shadows_global: 0.0,
            shadows_global_from: -3.0,
            shadows_global_width: 2.0,
            highlights_local: 0.0,
            highlights_key: 0.0,
            shadows_local: 0.0,
            shadows_key: 0.0,
            whites_minus_curve: [0.0; 5],
            whites_plus_curve: [0.0; 6],
            blacks_minus_curve: [0.0; 3],
            blacks_plus_curve: [0.0; 5],
            contrast_key: 0.0,
            tone_texture: 0.1,
            // Vibrance/Saturation: fitted on Shinobi Colour to four Adobe Color
            // kits (S5II portrait, Fujifilm blue scene, D810 pool and garden
            // for cyans and greens); most cases within 1-3 C*.
            saturation_plus: 0.981,
            saturation_minus: 0.999,
            vibrance_plus: 1.0,
            vibrance_minus: 0.772,
            saturation_hue: [-0.125, 0.307, -0.1, 0.081],
            vibrance_soft: 0.083,
            vibrance_hue: [-0.188, -0.2, -0.188, -0.228],
            vibrance_curve: 1.873,
            // The default profile's (Shinobi Colour's) curve and colour; see
            // `Profile::look`.
            curve: [3.1791, 0.0132, 2.2179, 0.5795, 0.1044],
            look_chroma: [0.933, -0.009, 0.044, -0.011, 0.013],
            look_hue: [1.19, -0.24, 1.04, -2.13],
            look_light: [-0.052, -0.026, 0.016, 0.124],
            look_comp: 0.000,
            look_lut: [[0.75, -0.019, 0.11], [0.0, 0.013, 0.004], [-4.0, 0.015, 0.102], [-2.188, -0.066, -0.043], [3.484, 0.023, 0.121], [-0.375, 0.097, 0.159], [-2.25, -0.106, 0.312], [2.813, -0.004, 0.334], [4.125, -0.037, 0.033], [2.25, -0.007, 0.233], [1.25, -0.007, 0.443], [-4.0, 0.03, 0.304]],
            adobe_fix_l: [0.0; 11],
            adobe_fix_c: [0.0; 11],
            tools: crate::tools::ToolTuning::default(),
        }
    }
}

impl Tuning {
    /// Shinobi Colour's own tone sliders (until 2026-10-10 they shared Shinobi
    /// Standard's, which were fitted against what turned out to be Adobe
    /// Standard exports). Fitted against true Adobe Color exports.
    pub fn shinobi_colour() -> Tuning {
        Tuning {
            tone_key: 0.8522,
            tone_key_ref: -1.2966,
            tone_key_dark: 0.0,
            tone_key_dark_span: 0.0,
            tone_pp: 0.0,
            sat_pp: 1.0,
            look_warm: [[0.0; 2]; 5],
            look_pp: 0.0,
            blue_s: 0.0,
            blue_s_half: 0.0,
            look_hsv: [[0.0; 3]; 12],
            look_hsv_dark: [[0.0; 3]; 12],
            shadows_pow: 1.0,
            blacks_plus_centre: 0.0,
            blacks_plus_width: 0.0,
            curve_pp: 1.0,
            tone_local: 1.6281,
            tone_local_wb: 0.5622,
            tone_radius: 0.0481,
            highlights_plus: 1.4074,
            highlights_minus: 1.5088,
            highlights_centre: 3.1939,
            highlights_width: 2.048,
            shadows_plus: 1.7013,
            shadows_minus: 1.6762,
            shadows_centre: -4.2603,
            shadows_width: 1.7119,
            whites_plus: 2.0249,
            whites_minus: 1.5112,
            whites_from: -5.6123,
            whites_to: 8.1043,
            whites_minus_from: -3.2512,
            whites_minus_to: 7.0817,
            blacks_plus: 0.949,
            blacks_minus: 1.4561,
            blacks_centre: -3.7508,
            blacks_width: 2.1438,
            contrast_plus: 1.5671,
            contrast_minus: 0.5978,
            contrast_pivot: 0.0144,
            contrast_bright: 0.5738,
            contrast_sat_plus: 0.5942,
            contrast_sat_minus: 0.4893,
            tone_chroma: 0.25,
            contrast_key: 0.0,
            tone_texture: 0.4,
            tone_span: -0.0625,
            tone_span_ref: 8.6992,
            ..Tuning::default()
        }
    }

    /// The sliders' tuning for Adobe Color (`render_adobe`): fitted to
    /// Lightroom's Adobe Color exports through Adobe's own profile in our
    /// engine (`lrmatch`, `colourmatch`, `toolmatch` with PROFILE=adobe), so
    /// each slider moves the photo as Lightroom's does on that profile.
    pub fn adobe() -> Tuning {
        Tuning {
            // Tone sliders fitted 2026-10-08 on Adobe Color through Adobe's own
            // profile (S5II tone kit, `lrmatch` PROFILE=adobe): every slider at
            // +-100 within 0.4-1.8 L* of Lightroom (Shinobi's values were 2-8 L*
            // off on this path).
            contrast_plus: 1.5781,
            contrast_minus: 0.6425,
            contrast_pivot: -0.8045,
            contrast_bright: 0.7684,
            contrast_fade_from: 8.0,
            contrast_fade_to: 10.0,
            contrast_toe_from: 0.0,
            contrast_toe_to: 0.0,
            contrast_sat_plus: 0.168,
            contrast_sat_minus: -0.1348,
            contrast_rgb: 3.0,
            contrast_sat_soft: 0.06,
            highlights_plus: 1.6894,
            // Highlights - strengthened 2026-10-10 on the slider-mix and band-area
            // kits (our pull on the brightest tones was ~1.7 L* short): food
            // flash 2.71 -> 2.64, outdoor 2.19 -> 2.16, real edits 4.75 -> 4.73.
            highlights_minus: 1.594,
            highlights_centre: 3.1119,
            highlights_width: 2.0588,
            shadows_plus: 1.6795,
            shadows_minus: 1.7527,
            shadows_centre: -4.551,
            shadows_width: 1.6495,
            whites_plus: 2.1023,
            whites_minus: 1.4033,
            whites_from: -5.563,
            whites_to: 8.1021,
            whites_minus_from: -3.9097,
            whites_minus_to: 9.4372,
            blacks_plus: 0.9819,
            blacks_minus: 1.7967,
            blacks_centre: -3.9476,
            blacks_width: 2.2235,
            // Placement refitted 2026-10-10 with the flash-faces kit (four flash
            // photos, one slider at a time from the food drinks settings):
            // Lightroom places the bands by the photo's overall brightness about
            // twice as much as we had, so in a dark flash photo the lit faces count
            // as highlights (Shadows lifts them less, Highlights pulls them more).
            // Held out by photo: 61-64% -> 66%.
            tone_local: 1.1688,
            tone_local_wb: 0.6387,
            tone_radius: 0.0506,
            tone_eps: 0.25,
            tone_key: 0.8974,
            tone_key_ref: -1.3981,
            tone_key_dark: 1.3,
            tone_key_dark_span: 0.9,
            tone_pp: 0.0,
            sat_pp: 1.0,
            look_warm: [[0.0; 2]; 5],
            look_pp: 0.0,
            blue_s: 0.0,
            blue_s_half: 0.0,
            look_hsv: [[0.0; 3]; 12],
            look_hsv_dark: [[0.0; 3]; 12],
            shadows_pow: 1.0188,
            blacks_plus_centre: 0.0,
            blacks_plus_width: 0.0,
            curve_pp: 0.0,
            tone_span: -0.2125,
            tone_span_ref: 9.3281,
            tone_table: 0.0,
            tone_adapt: 0.0,
            tone_adapt_no_exposure: 1.0,
            tone_adapt_h: -0.041,
            tone_adapt_s: 0.025,
            tone_adapt_w: -0.05,
            tone_adapt_hc: 41.5,
            tone_adapt_sc: 59.4,
            tone_adapt_wc: 88.2,
            tone_grid: 128.0,
            tone_minmax: 0.0,
            // Blacks keeps half the colour it would otherwise take with it (colour
            // kit, true Adobe Color: our warm colours were 81% of Lightroom's with
            // the tone sliders on, 93% without; Blacks -47 changed colour 1.5 in
            // ours, 0.1 in Lightroom). Food 3.94/2.67/2.80 -> 3.68/2.48/2.44,
            // real edits 4.74 -> 4.57.
            tone_chroma: 0.6625,
            contrast_key: 0.0,
            tone_texture: 0.4,
            // Where our render of Adobe's profile differs from Lightroom's, by L*
            // (measured 2026-10-09 on 85 photos at each one's best exposure: ours
            // was a little more contrasty and saturated in the midtones). Fitted on
            // 34 photos, the other 51 went 1.60 -> 1.27 dE; these are from all 85.
            adobe_fix_l: [-0.34, -0.12, 0.34, 0.60, 0.41, -0.08, -0.58, -0.87, -0.96, -1.03, -1.08],
            adobe_fix_c: [0.036, 0.047, 0.048, 0.015, -0.030, -0.054, -0.053, -0.038, -0.020, 0.017, 0.046],
            whites_minus_pow: 1.0,
            shadows_plus_width_lo: 0.0,
            whites_key: 0.0,
            whites_contrast: 0.0,
            // Measured global Whites / Blacks curves (see `wb_curve`), fitted to the
            // Adobe Color and Adobe Standard kits' single-slider exports.
            wb_curve: 1.0,
            whites_minus_curve: [0.6166, 0.6, 1.0601, -0.5512, 0.924],
            whites_plus_curve: [2.3277, 2.6103, 0.53, 1.8589, 2.9687, -2.9657],
            blacks_minus_curve: [0.0398, 1.6258, 0.0038],
            blacks_plus_curve: [0.1019, 1.1424, 0.5579, 0.4969, 0.7168],
            contrast_after_tone: 1.0,
            hs_own: 0.0,
            highlights_local: 0.5,
            highlights_key: 0.5,
            shadows_local: 0.0,
            shadows_key: 1.0,
            highlights_global: -0.0141,
            highlights_global_from: -0.5312,
            highlights_global_width: 0.7031,
            shadows_global: 0.0938,
            shadows_global_from: -2.3438,
            shadows_global_width: 1.375,
            highlights_tail: 0.0,
            highlights_tail_width: 3.0,
            // Photo-adaptive Highlights / Shadows (see `hs_adaptive`), fitted to the
            // measured per-photo curves; + Highlights and - Shadows from 8 photos only,
            // so their simpler form (anchor and strength from mean and p99.5).
            hs_adaptive: 0.0,
            highlights_minus_model: [0.251, 0.984, 0.903, 1.421, 2.263, 0.495, -0.364, 0.319, -0.387, -0.465, -0.958],
            highlights_plus_model: [0.360, 0.971, 0.897, 1.819, 1.156, 0.830, 0.0, 0.0, 0.942, 0.297, 0.0],
            shadows_plus_model: [0.112, 0.856, 0.939, 0.0, 2.702, 0.437, -0.006, 0.982, -0.259, 0.170, -0.053],
            shadows_minus_model: [0.071, 1.038, 1.014, 0.0, 3.501, 0.882, 0.0, 0.0, 0.860, 0.002, 0.0],
            shadows_highlights_link: 0.6,
            display_curve: [0.248, 0.22, 0.739, 0.765],
            display_blue_curve: [0.241, 0.263],
            yellow_sat_extra: 10.0,
            ..Tuning::default()
        }
    }
}

/// Highlights / Shadows / Whites / Blacks, like Lightroom's: each shifts the
/// tones in its band by some stops.
/// Bands overlap smoothly and strengths are limited so no combination can
/// fold the tones back on themselves (tested below).
///
/// This is the global version. Lightroom's Highlights/Shadows are partly
/// *local* (they protect local contrast); that refinement comes later, with
/// an edge-aware filter, without changing what the sliders mean.
pub fn tone_bands(c: [f32; 3], s: &RenderSettings, t: &Tuning) -> [f32; 3] {
    tone_bands_at(c, s, t, 0.0, t.tone_key_ref)
}

/// `tone_bands` for a pixel whose area is `offset` stops brighter than it
/// (from `ToneMap`) in a photo whose mean log brightness is `key` stops.
pub fn tone_bands_at(c: [f32; 3], s: &RenderSettings, t: &Tuning, offset: f32, key: f32) -> [f32; 3] {
    tone_bands_span(c, s, t, offset, key, t.tone_span_ref)
}

/// `tone_bands_at` in a photo whose brightness range is `span` stops.
pub fn tone_bands_span(c: [f32; 3], s: &RenderSettings, t: &Tuning, offset: f32, key: f32, span: f32) -> [f32; 3] {
    if s.highlights == 0.0 && s.shadows == 0.0 && s.whites == 0.0 && s.blacks == 0.0 {
        return c;
    }
    let l = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    if l <= 1e-7 {
        return c;
    }
    let strength = |amount: f32, plus: f32, minus: f32| amount / 100.0 * if amount >= 0.0 { plus } else { minus };
    // The bands are applied one after another rather than summed: each on its
    // own keeps tones in order, and a chain of order-keeping steps keeps them
    // in order too - so no combination of sliders can invert tones. (With one
    // slider moved it's identical to the fitted, single-slider response.)
    let mut ev = (l / 0.18).log2();
    let start = ev;
    // Where each band reads the pixel: towards its area, relative to the photo.
    // Darker photos than the reference follow further (`tone_key_dark`), the
    // extra growing only over `tone_key_dark_span` stops.
    let below = (key - t.tone_key_ref).min(0.0);
    let below = if t.tone_key_dark_span > 0.0 { below.max(-t.tone_key_dark_span) } else { below };
    let extra = if t.tone_key_dark != 0.0 { t.tone_key_dark - t.tone_key } else { 0.0 };
    let key_shift = t.tone_key * (key - t.tone_key_ref) + extra * below;
    let at = t.tone_local * offset - key_shift;
    let at_wb = t.tone_local_wb * offset - key_shift;
    // Positions measured from the photo's middle, scaled to its range.
    let stretch = (t.tone_span_ref / span.max(1.0)).powf(t.tone_span);
    let pos = |e: f32| t.tone_key_ref + (e - t.tone_key_ref) * stretch;
    // Where each band reads the pixel (see `tone_minmax`).
    let (at_hi, at_lo, wb_hi, wb_lo) = if t.tone_minmax > 0.5 {
        let (o, ow) = (t.tone_local * offset, t.tone_local_wb * offset);
        (o.min(0.0) - key_shift, o.max(0.0) - key_shift, ow.min(0.0) - key_shift, ow.max(0.0) - key_shift)
    } else {
        (at, at, at_wb, at_wb)
    };
    let (at_hi, at_lo) = if t.hs_own > 0.5 {
        (t.highlights_local * offset - t.highlights_key * (key - t.tone_key_ref), t.shadows_local * offset - t.shadows_key * (key - t.tone_key_ref))
    } else {
        (at_hi, at_lo)
    };
    let (wf, wt) = if s.whites >= 0.0 { (t.whites_from, t.whites_to) } else { (t.whites_minus_from, t.whites_minus_to) };
    // The four bands in turn from a brightness `e` (stops over grey): the end
    // brightness, and the brightness before Blacks.
    let chain = |mut e: f32| -> (f32, f32) {
        if t.hs_adaptive > 0.5 {
            e += hs_curve(e, s.highlights, if s.highlights < 0.0 { &t.highlights_minus_model } else { &t.highlights_plus_model }, &t.photo_stats, true);
            let link = if s.shadows > 0.0 { (1.0 - t.shadows_highlights_link * (-s.highlights).max(0.0) / 100.0).max(0.0) } else { 1.0 };
            e += link * hs_curve(e, s.shadows, if s.shadows > 0.0 { &t.shadows_plus_model } else { &t.shadows_minus_model }, &t.photo_stats, false);
        } else {
        e += strength(s.highlights, t.highlights_plus, t.highlights_minus) * (highlights_weight(pos(e + at_hi), t) + t.highlights_global * ramp(key, t.highlights_global_from, t.highlights_global_from + t.highlights_global_width.max(0.1)));
        let sh = if s.shadows > 0.0 && t.shadows_pow != 1.0 { 100.0 * (s.shadows / 100.0).powf(t.shadows_pow) } else { s.shadows };
        let sp = pos(e + at_lo);
        let sw = if sh > 0.0 && t.shadows_plus_width_lo > 0.0 && sp < t.shadows_centre { t.shadows_plus_width_lo } else { t.shadows_width };
        let link = if sh > 0.0 { (1.0 - t.shadows_highlights_link * (-s.highlights).max(0.0) / 100.0).max(0.0) } else { 1.0 };
        e += link * strength(sh, t.shadows_plus, t.shadows_minus) * (bump(sp, t.shadows_centre, sw) + t.shadows_global * (1.0 - ramp(key, t.shadows_global_from, t.shadows_global_from + t.shadows_global_width.max(0.1))));
        }
        if t.wb_curve > 0.5 {
            let e = whites_curve(e, s.whites, t);
            return (blacks_curve(e, s.blacks, t), e);
        }
        let wr = ramp(pos(e + wb_hi + t.whites_key * (key - t.tone_key_ref)), wf, wt);
        let wr = if s.whites < 0.0 && t.whites_minus_pow != 1.0 { wr.powf(t.whites_minus_pow) } else { wr };
        let wc = 1.0 + t.whites_contrast * s.contrast.max(0.0) / 100.0;
        e += strength(s.whites, t.whites_plus, t.whites_minus) * wc * wr;
        let before_blacks = e;
        let (bc, bw) = if s.blacks > 0.0 && t.blacks_plus_width > 0.0 { (t.blacks_plus_centre, t.blacks_plus_width) } else { (t.blacks_centre, t.blacks_width) };
        e += strength(s.blacks, t.blacks_plus, t.blacks_minus) * bump(pos(e + wb_lo), bc, bw);
        (e, before_blacks)
    };
    if t.tone_pp > 0.5 {
        // As Adobe applies tone curves, on linear ProPhoto (its working space):
        // the brightest and darkest channel each through the bands, the middle
        // one at the same proportion between them - hue holds, colour strength
        // follows the curve.
        static M: std::sync::OnceLock<([[f32; 3]; 3], [[f32; 3]; 3])> = std::sync::OnceLock::new();
        let (to, from) = *M.get_or_init(|| {
            let to = crate::tools::rec2020_to_prophoto();
            (to, crate::whitebalance::invert3(&to))
        });
        let p = apply3(&to, c).map(|v| v.max(0.0));
        let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
        if hi <= 1e-7 {
            return c;
        }
        let through = |v: f32| if v <= 1e-7 { v } else { let e = (v / 0.18).log2(); v * 2f32.powf(chain(e).0 - e) };
        let (nh, nl) = (through(hi), through(lo));
        let out = p.map(|v| if hi - lo < 1e-9 { nh } else { nl + (nh - nl) * (v - lo) / (hi - lo) });
        return apply3(&from, out);
    }
    let (end, before_blacks) = chain(ev);
    ev = end;
    let scale = 2f32.powf(ev - start);
    if t.tone_chroma == 0.0 {
        return c.map(|v| v * scale);
    }
    // Brightness scaled; the colour (difference from grey) follows the other
    // bands fully but only (1 - tone_chroma) of Blacks.
    let keep = 2f32.powf(before_blacks - start) * 2f32.powf(ev - before_blacks).powf(1.0 - t.tone_chroma);
    c.map(|v| (l * scale + (v - l) * keep).max(0.0))
}

/// Each pixel's area brightness relative to its own (stops), and the photo's
/// mean log brightness, for the local tone sliders (`tone_bands_at`). The area
/// brightness is an edge-aware smoothing of log brightness - a guided filter,
/// worked out on a small copy and applied at full size - so an area moves
/// together (a face keeps its shaping) without halos at strong edges.
#[derive(Clone, Debug)]
pub struct ToneMap {
    pub offset: Vec<f32>,
    /// The small map it was made from, for reading a part of the photo off it.
    pub grid: ToneGrid,
    pub key: f32,
    /// The photo's brightness range: 2nd to 98th percentile, in stops.
    pub span: f32,
    /// Mean, p50, p90 and p99.5 of each pixel's light (stops over mid-grey).
    pub stats: [f32; 4],
}

/// The tone map's small copy (`ToneMap::build`): mean log brightness per
/// cell and the guided filter's a and b, `scale` image pixels per cell of an
/// image `w` x `h`.
#[derive(Clone, Debug, Default)]
pub struct ToneGrid {
    pub(crate) g: Vec<f32>,
    pub(crate) ma: Vec<f32>,
    pub(crate) mb: Vec<f32>,
    pub(crate) gw: usize,
    pub(crate) gh: usize,
    pub(crate) scale: f32,
    pub(crate) w: usize,
    pub(crate) h: usize,
}

impl ToneGrid {
    fn sample(&self, v: &[f32], x: f32, y: f32) -> f32 {
        let (gw, gh) = (self.gw, self.gh);
        let (fx, fy) = ((x - 0.5).clamp(0.0, (gw - 1) as f32), (y - 0.5).clamp(0.0, (gh - 1) as f32));
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(gw - 1), (y0 + 1).min(gh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = v[y0 * gw + x0] * (1.0 - tx) + v[y0 * gw + x1] * tx;
        let bot = v[y1 * gw + x0] * (1.0 - tx) + v[y1 * gw + x1] * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// A pixel's offset from its own brightness `e` (stops) at grid place (x, y).
    fn offset_at(&self, e: f32, x: f32, y: f32, t: &Tuning) -> f32 {
        let guide = if t.tone_texture > 0.0 {
            let gs = self.sample(&self.g, x, y);
            e - (e - gs).clamp(-t.tone_texture, t.tone_texture)
        } else {
            e
        };
        self.sample(&self.ma, x, y) * guide + self.sample(&self.mb, x, y) - e
    }
}

impl ToneGrid {
    /// Each pixel's offset, for the whole picture the grid was made from (its scene light `lit`).
    pub fn offsets(&self, lit: &[f32], t: &Tuning) -> Vec<f32> {
        use rayon::prelude::*;
        let ev = |l: f32| (l.max(1e-5) / 0.18).log2();
        let (w, scale) = (self.w, self.scale);
        (0..lit.len()).into_par_iter().map(|i| self.offset_at(ev(lit[i]), (i % w) as f32 / scale, (i / w) as f32 / scale, t)).collect()
    }
}

impl ToneMap {
    /// The same map read for a part of the photo (`frame`), from each of its
    /// pixels' scene light: the offsets the whole-photo map gives there.
    pub fn for_part(&self, lit: &[f32], w: usize, frame: &Frame, t: &Tuning) -> ToneMap {
        use rayon::prelude::*;
        let ev = |l: f32| (l.max(1e-5) / 0.18).log2();
        let g = &self.grid;
        let offset = (0..lit.len())
            .into_par_iter()
            .map(|i| {
                let (fx, fy) = frame.frac(i, w);
                g.offset_at(ev(lit[i]), fx * g.w as f32 / g.scale, fy * g.h as f32 / g.scale, t)
            })
            .collect();
        ToneMap { offset, grid: ToneGrid::default(), key: self.key, span: self.span, stats: self.stats }
    }

    /// Whether these settings need one.
    pub fn wanted(s: &RenderSettings, t: &Tuning) -> bool {
        ((s.highlights != 0.0 || s.shadows != 0.0 || s.whites != 0.0 || s.blacks != 0.0)
            && (t.tone_local != 0.0 || t.tone_local_wb != 0.0 || t.tone_key != 0.0 || t.tone_span != 0.0 || t.hs_adaptive > 0.5))
            || (s.contrast != 0.0 && t.contrast_key != 0.0)
    }

    /// From each pixel's scene light (luminance after exposure and white balance).
    pub fn build(lit: &[f32], w: usize, h: usize, t: &Tuning) -> ToneMap {
        Self::build_with(lit, w, h, t, true)
    }

    /// `build`, with each pixel's offset worked out (`offsets`) or left for later (`offsets_for`;
    /// the GPU works them out from the grid itself).
    pub fn build_with(lit: &[f32], w: usize, h: usize, t: &Tuning, offsets: bool) -> ToneMap {
        let grid = (t.tone_grid.round() as usize).clamp(16, 2048);
        let ev = |l: f32| (l.max(1e-5) / 0.18).log2();
        let scale = (w.max(h) as f32 / grid as f32).max(1.0);
        let (gw, gh) = (((w as f32 / scale).ceil() as usize).max(1), ((h as f32 / scale).ceil() as usize).max(1));
        // The small copy: mean log brightness per cell. A row of cells at a time on all cores, each
        // cell's pixels added in the same order as one pass over the picture would (rows, then
        // columns), so the same sums.
        let mut sum = vec![0.0f32; gw * gh];
        let mut cnt = vec![0.0f32; gw * gh];
        {
            use rayon::prelude::*;
            sum.par_chunks_mut(gw).zip(cnt.par_chunks_mut(gw)).enumerate().for_each(|(gy, (srow, crow))| {
                for y in 0..h {
                    if ((y as f32 / scale) as usize).min(gh - 1) != gy {
                        continue;
                    }
                    for x in 0..w {
                        let gx = ((x as f32 / scale) as usize).min(gw - 1);
                        srow[gx] += ev(lit[y * w + x]);
                        crow[gx] += 1.0;
                    }
                }
            });
        }
        let g: Vec<f32> = sum.iter().zip(&cnt).map(|(s, c)| s / c.max(1.0)).collect();
        let mut sorted = g.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        // The photo's key: its mean log brightness (a median jumps between the
        // halves of a photo split into dark and bright, e.g. a dark room with a
        // bright window).
        let key = sorted.iter().sum::<f32>() / sorted.len() as f32;
        let span = sorted[sorted.len() * 98 / 100] - sorted[sorted.len() * 2 / 100];
        // Guided filter (the image guiding itself) on the small copy.
        let r = ((t.tone_radius * grid as f32).round() as usize).max(1);
        let boxf = |v: &[f32]| -> Vec<f32> {
            let mut tmp = vec![0.0f32; gw * gh];
            for y in 0..gh {
                for x in 0..gw {
                    let (x0, x1) = (x.saturating_sub(r), (x + r).min(gw - 1));
                    tmp[y * gw + x] = (x0..=x1).map(|k| v[y * gw + k]).sum::<f32>() / (x1 - x0 + 1) as f32;
                }
            }
            let mut out = vec![0.0f32; gw * gh];
            for y in 0..gh {
                let (y0, y1) = (y.saturating_sub(r), (y + r).min(gh - 1));
                for x in 0..gw {
                    out[y * gw + x] = (y0..=y1).map(|k| tmp[k * gw + x]).sum::<f32>() / (y1 - y0 + 1) as f32;
                }
            }
            out
        };
        let mean = boxf(&g);
        let sq: Vec<f32> = g.iter().map(|v| v * v).collect();
        let mean_sq = boxf(&sq);
        let eps = t.tone_eps.max(1e-4);
        let a: Vec<f32> = mean.iter().zip(&mean_sq).map(|(m, m2)| {
            let var = (m2 - m * m).max(0.0);
            var / (var + eps)
        }).collect();
        let b: Vec<f32> = mean.iter().zip(&a).map(|(m, a)| m - a * m).collect();
        let (ma, mb) = (boxf(&a), boxf(&b));
        // Full size: a and b read bilinearly, applied to each pixel's own brightness.
        let grid = ToneGrid { g, ma, mb, gw, gh, scale, w, h };
        use rayon::prelude::*;
        let offset = if offsets { grid.offsets(lit, t) } else { Vec::new() };
        // Per-pixel brightness statistics (every few pixels is plenty).
        let step = (lit.len() / 250_000).max(1);
        // Capped where a render reaches white (about 2.5 stops over mid-grey), as
        // the measurements behind `hs_adaptive` read them from exports.
        let mut lv: Vec<f32> = lit.iter().step_by(step).map(|&l| ev(l).min(2.5)).collect();
        // On all cores (a sort's result is the same however it's done).
        lv.par_sort_unstable_by(|a, b| a.total_cmp(b));
        let q = |f: f32| lv[((lv.len() - 1) as f32 * f) as usize];
        let stats = [lv.iter().sum::<f32>() / lv.len() as f32, q(0.5), q(0.9), q(0.995)];
        ToneMap { offset, grid, key, span, stats }
    }
}

/// Conversions between display-linear Rec.2020 and OKLab, a perceptual colour
/// space where chroma can be scaled without shifting hue or lightness.
pub struct Oklab {
    pub(crate) to_lms: [[f32; 3]; 3],
    pub(crate) from_lms: [[f32; 3]; 3],
    pub(crate) lms_to_lab: [[f32; 3]; 3],
    pub(crate) lab_to_lms: [[f32; 3]; 3],
}

impl Oklab {
    pub fn new() -> Self {
        // Björn Ottosson's OKLab matrices (XYZ D65 -> LMS, LMS' -> Lab).
        const XYZ_TO_LMS: [[f32; 3]; 3] = [
            [0.818_933, 0.361_866_74, -0.128_859_71],
            [0.032_984_544, 0.929_311_9, 0.036_145_64],
            [0.048_200_3, 0.264_366_27, 0.633_851_7],
        ];
        const LMS_TO_LAB: [[f32; 3]; 3] = [
            [0.210_454_26, 0.793_617_8, -0.004_072_047],
            [1.977_998_5, -2.428_592_2, 0.450_593_7],
            [0.025_904_037, 0.782_771_77, -0.808_675_77],
        ];
        let to_lms = mul3(&XYZ_TO_LMS, &REC2020_TO_XYZ);
        Self {
            to_lms,
            from_lms: crate::whitebalance::invert3(&to_lms),
            lms_to_lab: LMS_TO_LAB,
            lab_to_lms: crate::whitebalance::invert3(&LMS_TO_LAB),
        }
    }

    pub fn to_lab(&self, rgb: [f32; 3]) -> [f32; 3] {
        apply3(&self.lms_to_lab, apply3(&self.to_lms, rgb).map(f32::cbrt))
    }

    pub fn to_rgb(&self, lab: [f32; 3]) -> [f32; 3] {
        apply3(&self.from_lms, apply3(&self.lab_to_lms, lab).map(|v| v * v * v))
    }
}

impl Default for Oklab {
    fn default() -> Self {
        Self::new()
    }
}

/// A smooth strength multiplier around the hue circle (radians): 1 plus the
/// first two harmonics, so four numbers can describe e.g. "strong on
/// oranges, weak on blues".
fn hue_weight(hue: f32, k: &[f32; 4]) -> f32 {
    let (s1, c1) = hue.sin_cos();
    let (s2, c2) = (2.0 * hue).sin_cos();
    hue_weight_of(s1, c1, s2, c2, k)
}

/// `hue_weight` from the hue's sine and cosine and its double's, already worked out.
fn hue_weight_of(s1: f32, c1: f32, s2: f32, c2: f32, k: &[f32; 4]) -> f32 {
    (1.0 + k[0] * c1 + k[1] * s1 + k[2] * c2 + k[3] * s2).max(0.0)
}

/// Vibrance and Saturation, after the tone curve (Lightroom also applies them
/// on the rendered image). Both scale OKLab chroma - colour intensity - so
/// hue and lightness hold. Saturation scales every colour alike; Vibrance
/// eases off for colours already strong and for skin tones.
///
/// RapidOne Standard's own colour character (`look_chroma`, `look_hue`) is
/// applied here first, in the same OKLab pass.
/// The Shinobi colour character on linear ProPhoto (`look_pp`), the way
/// camera profiles store theirs: a hue shift (degrees), saturation scale and
/// value scale at 12 hues (`look_hsv`, every 30 degrees of ProPhoto HSV hue),
/// read linearly between them; the value change grows with saturation, so
/// greys stay put.
fn look_hsv(c: [f32; 3], t: &Tuning) -> [f32; 3] {
    static M: std::sync::OnceLock<([[f32; 3]; 3], [[f32; 3]; 3])> = std::sync::OnceLock::new();
    let (to, from) = *M.get_or_init(|| {
        let to = crate::tools::rec2020_to_prophoto();
        (to, crate::whitebalance::invert3(&to))
    });
    let p = apply3(&to, c).map(|v| v.max(0.0));
    let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
    let d = mx - mn;
    if mx <= 1e-7 || d <= 1e-7 {
        return c;
    }
    let h = 60.0 * if mx == p[0] { ((p[1] - p[2]) / d).rem_euclid(6.0) } else if mx == p[1] { (p[2] - p[0]) / d + 2.0 } else { (p[0] - p[1]) / d + 4.0 };
    let (s, v) = (d / mx, mx);
    let pos = h / 30.0;
    let (i0, f) = (pos.floor() as usize % 12, pos.fract());
    // Between the dark table (`look_hsv_dark`, at black) and `look_hsv` (at
    // white) by the value's gamma-2.2 encoding, when a dark table is set.
    let row = |tab: &[[f32; 3]; 12]| -> [f32; 3] { std::array::from_fn(|k| tab[i0][k] + (tab[(i0 + 1) % 12][k] - tab[i0][k]) * f) };
    let bright = row(&t.look_hsv);
    let e: [f32; 3] = if t.look_hsv_dark == [[0.0; 3]; 12] {
        bright
    } else {
        let dark = row(&t.look_hsv_dark);
        let m = v.clamp(0.0, 1.0).powf(1.0 / 2.2);
        std::array::from_fn(|k| dark[k] + (bright[k] - dark[k]) * m)
    };
    let (h2, s2, v2) = (h + e[0], (s * (1.0 + e[1])).clamp(0.0, 1.0), v * (1.0 + e[2] * s).max(0.0));
    let hp = h2.rem_euclid(360.0) / 60.0;
    let fr = hp - hp.floor();
    let (a, b, cc) = (v2 * (1.0 - s2), v2 * (1.0 - s2 * fr), v2 * (1.0 - s2 * (1.0 - fr)));
    let out = match hp as i32 {
        0 => [v2, cc, a],
        1 => [b, v2, a],
        2 => [a, v2, cc],
        3 => [a, b, v2],
        4 => [cc, a, v2],
        _ => [v2, a, b],
    };
    apply3(&from, out)
}

/// For comparing the Mixer's solves (`examples/mixsolve_check.rs`): 1 = the solve (always, unless
/// a comparison sets another), 0 = the one before 25 Oct 2026 (eight rounds of channel ratios, and a
/// colour whose solve ran more than 3x away kept as it was), 3 = the solve run to convergence. While
/// it isn't 1 the GPU engine leaves renders to the CPU.
pub static MIX_SOLVE_COMPARE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
/// With `MIX_COUNT` set, how many colour renders the solves did (`MIX_EVALS`) and how many solves.
pub static MIX_COUNT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static MIX_EVALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static MIX_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn count(what: &std::sync::atomic::AtomicU64) {
    if MIX_COUNT.load(std::sync::atomic::Ordering::Relaxed) {
        what.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The Mixer's solve as it was before 25 Oct 2026, for comparisons (`MIX_SOLVE_COMPARE` 0).
fn mixer_solve_before(lit: [f32; 3], shown_lit: [f32; 3], target: [f32; 3], shown_of: &dyn Fn([f32; 3]) -> [f32; 3]) -> [f32; 3] {
    count(&MIX_CALLS);
    let start = lit.map(|v| v.max(1e-4));
    let mut x = start;
    let mut known = (start == lit).then_some(shown_lit);
    for _ in 0..8 {
        let now = known.take().unwrap_or_else(|| {
            count(&MIX_EVALS);
            shown_of(x)
        });
        let next: [f32; 3] = std::array::from_fn(|k| if now[k] > 1e-6 { x[k] * (target[k].max(0.0) / now[k]).clamp(0.25, 4.0) } else { x[k] });
        if next == x {
            break;
        }
        x = next;
    }
    if (0..3).any(|k| x[k] > start[k] * 3.0 || x[k] < start[k] / 3.0) {
        return lit;
    }
    x
}

/// The Mixer's solve (`ToolTuning::mix_before_tone`): the scene colour that shows as `target`
/// through `shown_of` (the colour as shown without the tone sliders), starting from `lit`, which
/// shows as `shown_lit`.
///
/// A Levenberg-Marquardt solve on the log of each scene channel, its error measured in OKLab: the
/// first guess is that each shown channel follows its own scene channel, and Broyden's update
/// corrects that guess as the colour moves. A step is kept only when it brings a fair share of the
/// improvement it promised (`MIX_SOLVE_GAIN`), else the next is shorter, so the solve never jumps to
/// another colour that happens to be a little closer; where it stalls far from the target, steps of
/// the old ratio kind cross the plateau it is on. Most colours settle in a round or two (renders
/// with the Mixer about a quarter faster than with the old fixed eight rounds). A colour the target
/// can't be shown as (held at the edge of what the profile shows, Saturation or Vibrance far up)
/// ends at the nearest one it can reach, each channel within `MIX_SOLVE_BOX` times where it started.
/// (Before 25 Oct the solve often hadn't settled, and a colour whose channel ran more than 3x away
/// was dropped back to unmixed, pixel by pixel: speckle in deep navy and bright cyan, and whole
/// areas turned red or green with Saturation up. Checked on 97 photos from 24 of Chris's shoots,
/// half held out: PROGRESS.md, 25 Oct.)
fn mixer_solve(lit: [f32; 3], shown_lit: [f32; 3], target: [f32; 3], shown_of: &dyn Fn([f32; 3]) -> [f32; 3], ok: &Oklab) -> [f32; 3] {
    mixer_solve_with(lit, shown_lit, target, shown_of, ok, MIX_SOLVE_ROUNDS, MIX_SOLVE_TOL, MIX_SOLVE_BOX.ln())
}

/// Colour renders the Mixer's solve may do after its first (each one a step; most colours need one
/// or two).
const MIX_SOLVE_ROUNDS: usize = 10;
/// The solve has settled when the shown colour is within this of the target in OKLab (each of
/// L, a and b; an 8-bit level is about 0.004 in L).
const MIX_SOLVE_TOL: f32 = 2e-4;
/// How far a scene channel may move in the solve: up to this many times its starting level, or
/// down to one over it.
const MIX_SOLVE_BOX: f32 = 8.0;
/// The largest step a channel takes in one round (log: x4 or /4).
const MIX_SOLVE_STEP: f32 = 1.386_294_4;
/// A step is kept when the error falls by at least this share of what the step's guess promised.
const MIX_SOLVE_GAIN: f32 = 0.25;
/// The solve has stalled when a round takes less than this share off the (squared) error while the
/// colour is still this far (OKLab) from the target: then ratio steps, while each clearly helps.
const MIX_SOLVE_STALL: f32 = 0.05;
const MIX_SOLVE_FAR: f32 = 0.01;
/// A ratio step is kept when it takes at least this share off the (squared) error: clearly, so the
/// rounding of the arithmetic (the CPU's or a GPU's) never decides it.
const MIX_SOLVE_CLEAR: f32 = 0.01;

/// The Mixer solve's numbers, for the GPU engine's copy of it (`gpu::params`).
#[derive(Clone, Copy)]
pub(crate) struct MixSolveConstants {
    pub rounds: usize,
    pub tol: f32,
    pub step: f32,
    pub bound: f32,
    pub gain: f32,
    pub stall: f32,
    pub far: f32,
    pub clear: f32,
}

pub(crate) const MIX_SOLVE_CONSTANTS: MixSolveConstants = MixSolveConstants { rounds: MIX_SOLVE_ROUNDS, tol: MIX_SOLVE_TOL, step: MIX_SOLVE_STEP, bound: MIX_SOLVE_BOX, gain: MIX_SOLVE_GAIN, stall: MIX_SOLVE_STALL, far: MIX_SOLVE_FAR, clear: MIX_SOLVE_CLEAR };

/// `mixer_solve` with its limits: rounds, tolerance and how far (log) a channel may move.
#[allow(clippy::too_many_arguments)]
fn mixer_solve_with(lit: [f32; 3], shown_lit: [f32; 3], target: [f32; 3], shown_of: &dyn Fn([f32; 3]) -> [f32; 3], ok: &Oklab, rounds: usize, tol: f32, bound: f32) -> [f32; 3] {
    count(&MIX_CALLS);
    let start = lit.map(|v| v.max(1e-4));
    let goal = ok.to_lab(target.map(|v| v.max(0.0)));
    let now = if start == lit {
        shown_lit
    } else {
        count(&MIX_EVALS);
        shown_of(start)
    };
    let miss = |shown: [f32; 3]| -> [f32; 3] {
        let l = ok.to_lab(shown.map(|v| v.max(0.0)));
        std::array::from_fn(|k| l[k] - goal[k])
    };
    let norm2 = |r: &[f32; 3]| r[0] * r[0] + r[1] * r[1] + r[2] * r[2];
    let worst = |r: &[f32; 3]| r[0].abs().max(r[1].abs()).max(r[2].abs());
    let mut r = miss(now);
    if worst(&r) < tol {
        return start;
    }
    // Unknowns: the log of each scene channel. The first guess at how the shown colour (in OKLab)
    // follows them: each shown channel in proportion to its own scene channel.
    let u0 = start.map(f32::ln);
    let mut u = u0;
    let mut jac = oklab_jacobian(ok, now);
    for row in jac.iter_mut() {
        for k in 0..3 {
            row[k] *= now[k].max(0.0);
        }
    }
    // The damping (Nielsen's rule): set from the first guess, then eased as steps work out.
    let mut mu = -1.0f32;
    let mut nu = 2.0f32;
    // The colour shown at `u`, for the ratio step (below).
    let mut shown = now;
    let (mut ratio_left, mut use_ratio) = (true, false);
    for _ in 0..rounds {
        let jt = transpose3(&jac);
        let mut a = mul3(&jt, &jac);
        if mu < 0.0 {
            mu = 1e-3 * a[0][0].max(a[1][1]).max(a[2][2]);
        }
        for k in 0..3 {
            a[k][k] += mu;
        }
        let g = apply3(&jt, r);
        let inv = crate::whitebalance::invert3(&a);
        // The step, no more than MIX_SOLVE_STEP a channel, and each channel kept in its box. Once
        // the solve stalls far from its target, the old solve's step once instead: each channel
        // moved by how far its shown channel is from the target's. It crosses plateaus the colour
        // has where Saturation or Vibrance hold it at the edge of what can be shown, so the shown
        // colour doesn't follow the scene's at all for a while.
        let ratio = std::mem::take(&mut use_ratio);
        let step: [f32; 3] = std::array::from_fn(|k| {
            let s = if ratio {
                if shown[k] > 1e-6 { (target[k].max(0.0) / shown[k]).clamp(0.25, 4.0).ln() } else { 0.0 }
            } else {
                (-(inv[k][0] * g[0] + inv[k][1] * g[1] + inv[k][2] * g[2])).clamp(-MIX_SOLVE_STEP, MIX_SOLVE_STEP)
            };
            (u[k] + s).clamp(u0[k] - bound, u0[k] + bound) - u[k]
        });
        if !step.iter().all(|v| v.is_finite()) || norm2(&step) < 1e-12 {
            break;
        }
        let u2: [f32; 3] = std::array::from_fn(|k| u[k] + step[k]);
        count(&MIX_EVALS);
        let shown2 = shown_of(u2.map(f32::exp));
        let r2 = miss(shown2);
        if !r2.iter().all(|v| v.is_finite()) {
            break;
        }
        // What the guess promised, and what the step did.
        let pred = apply3(&jac, step);
        let model: [f32; 3] = std::array::from_fn(|k| r[k] + pred[k]);
        let before = norm2(&r);
        let promised = before - norm2(&model);
        let gained = before - norm2(&r2);
        // Broyden's update: the guess corrected by how the colour really moved.
        let ss = norm2(&step);
        for i in 0..3 {
            for k in 0..3 {
                jac[i][k] += (r2[i] - r[i] - pred[i]) * step[k] / ss;
            }
        }
        let rho = if promised > 0.0 { gained / promised } else { -1.0 };
        let kept = if ratio { gained > MIX_SOLVE_CLEAR * before } else { rho > MIX_SOLVE_GAIN };
        if kept {
            u = u2;
            r = r2;
            shown = shown2;
            if !ratio {
                mu *= (1.0 - (2.0 * rho - 1.0).powi(3)).max(1.0 / 3.0);
                nu = 2.0;
            }
            if worst(&r) < tol {
                break;
            }
        } else if !ratio {
            mu *= nu;
            nu *= 2.0;
        }
        // Stalled far from the target: ratio steps, as long as each clearly helps.
        if ratio && !kept {
            ratio_left = false;
        }
        if ratio_left && (!kept || gained < MIX_SOLVE_STALL * before) && worst(&r) > MIX_SOLVE_FAR {
            use_ratio = true;
        }
    }
    u.map(f32::exp)
}

fn transpose3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

/// How OKLab (L, a, b) moves with each display-linear Rec.2020 channel at `rgb`.
fn oklab_jacobian(ok: &Oklab, rgb: [f32; 3]) -> [[f32; 3]; 3] {
    let lms = apply3(&ok.to_lms, rgb.map(|v| v.max(0.0)));
    let d = lms.map(|v| 1.0 / (3.0 * v.max(1e-6).cbrt().powi(2)));
    let mid: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| d[i] * ok.to_lms[i][j]));
    mul3(&ok.lms_to_lab, &mid)
}

/// A colour with its chroma boosted by `k` (`colour_at(k)`), held inside the colours the scene
/// space can hold. If the full boost would push a channel negative, the strength is eased off
/// along the same hue and lightness to the largest that fits. (Clipping the channel instead, or
/// letting OKLab run on with a huge chroma, turns the hue - orange skin goes green.) Colours the
/// boost leaves inside are exactly `colour_at(k)`, so the slider's strength is unchanged for them.
fn limit_boost(colour_at: impl Fn(f32) -> [f32; 3], k: f32) -> [f32; 3] {
    let full = colour_at(k);
    let at_one = colour_at(1.0);
    // Only when the boost is what takes the colour out: one already outside before it is left as it was.
    if full[0].min(full[1]).min(full[2]) >= 0.0 || k <= 1.0 || at_one[0].min(at_one[1]).min(at_one[2]) < 0.0 {
        return full.map(|v| v.max(0.0));
    }
    // The first strength above 1 (the colour as it was) at which it no longer fits: stepped
    // up, because at a huge chroma the maths can wrap back inside with the hue turned.
    let fits = |kk: f32| { let c = colour_at(kk); c[0].min(c[1]).min(c[2]) >= 0.0 };
    let (mut lo, mut hi) = (1.0f32, k);
    let steps = 24;
    for n in 1..=steps {
        let kk = 1.0 + (k - 1.0) * n as f32 / steps as f32;
        if fits(kk) { lo = kk } else { hi = kk; break }
    }
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        let c = colour_at(mid);
        if c[0].min(c[1]).min(c[2]) >= 0.0 { lo = mid } else { hi = mid }
    }
    colour_at(lo).map(|v| v.max(0.0))
}

pub fn colour_adjust(rgb: [f32; 3], s: &RenderSettings, t: &Tuning, ok: &Oklab) -> [f32; 3] {
    ColourAdjust::new(s, t).apply(rgb, ok)
}

/// `colour_adjust` with what's the same for every pixel worked out once (whether the profile has a
/// look, Contrast's colour change, Vibrance's strength): a render calls it for every pixel, up to
/// nine times with the Mixer's solve. The same sums as before, in the same order.
pub struct ColourAdjust<'a> {
    s: &'a RenderSettings,
    t: &'a Tuning,
    pub(crate) has_look: bool,
    pub(crate) kc_full: f32,
    /// Vibrance's strength, `(vibrance / 100) ^ vibrance_curve * vibrance_plus` (boosts only).
    pub(crate) vibrance_v: f32,
}

impl<'a> ColourAdjust<'a> {
    pub fn new(s: &'a RenderSettings, t: &'a Tuning) -> Self {
        let has_look = t.look_pp <= 0.5 && (t.look_chroma != [1.0, 0.0, 0.0, 0.0, 0.0]
            || t.look_hue != [0.0; 4]
            || t.look_light != [0.0; 4]
            || t.look_comp != 0.0
            || t.look_lut != [[0.0; 3]; 12]
            || t.look_warm != [[0.0; 2]; 5]);
        // Contrast's colour-strength change (see `contrast_sat_plus`).
        let kc_full = s.contrast / 100.0 * if s.contrast >= 0.0 { t.contrast_sat_plus } else { t.contrast_sat_minus };
        let vibrance_v = if s.vibrance > 0.0 { (s.vibrance / 100.0).powf(t.vibrance_curve) * t.vibrance_plus } else { 0.0 };
        Self { s, t, has_look, kc_full, vibrance_v }
    }

    pub fn apply(&self, rgb: [f32; 3], ok: &Oklab) -> [f32; 3] {
        let (s, t, has_look, kc_full) = (self.s, self.t, self.has_look, self.kc_full);
        let rgb = if t.look_pp > 0.5 { look_hsv(rgb, t) } else { rgb };
        if s.vibrance == 0.0 && s.saturation == 0.0 && !has_look && s.contrast == 0.0 {
            return rgb;
        }
        let mut lab = ok.to_lab(rgb);
        let mut chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
        if chroma < 1e-6 {
            return rgb;
        }
        let mut hue = lab[2].atan2(lab[1]);
        let kc = (1.0 + kc_full * if t.contrast_sat_soft > 0.0 { t.contrast_sat_soft / (t.contrast_sat_soft + chroma) } else { 1.0 }).max(0.0);
        if has_look {
            let [c0, c1, c2, c3, c4] = t.look_chroma;
            let (s1, co1) = hue.sin_cos();
            let (s2, co2) = (2.0 * hue).sin_cos();
            chroma *= c0 * hue_weight_of(s1, co1, s2, co2, &[c1, c2, c3, c4]);
            let [h1, h2, h3, h4] = t.look_hue;
            let [l1, l2, l3, l4] = t.look_light;
            let mut light = lab[0] + chroma * (l1 * co1 + l2 * s1 + l3 * co2 + l4 * s2);
            // The finer 12-point table, linear between its points.
            let pos = hue.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 12.0;
            let (i0, f) = (pos.floor() as usize % 12, pos.fract());
            let entry: [f32; 3] = std::array::from_fn(|k| t.look_lut[i0][k] + (t.look_lut[(i0 + 1) % 12][k] - t.look_lut[i0][k]) * f);
            light += chroma * entry[2];
            // Warm colours by lightness (`look_warm`): strong yellows and oranges
            // made lighter or darker and stronger or weaker depending on how
            // light they are.
            if t.look_warm != [[0.0; 2]; 5] {
                let d = ((hue.to_degrees() - 85.0 + 540.0).rem_euclid(360.0) - 180.0) / 55.0;
                let w = (1.0 - d * d).max(0.0);
                if w > 0.0 {
                    let p = (lab[0] * 4.0).clamp(0.0, 4.0);
                    let i = (p.floor() as usize).min(3);
                    let f = p - i as f32;
                    let row: [f32; 2] = std::array::from_fn(|k| t.look_warm[i][k] + (t.look_warm[i + 1][k] - t.look_warm[i][k]) * f);
                    light += chroma * w * row[0];
                    chroma *= (1.0 + w * row[1]).max(0.0);
                }
            }
            chroma *= (1.0 + entry[1]).max(0.0);
            hue += (h1 * co1 + h2 * s1 + h3 * co2 + h4 * s2 + entry[0]).to_radians();
            // Strong colours held back a little, as Adobe's looks do.
            chroma /= 1.0 + t.look_comp * chroma;
            chroma *= kc;
            lab = [light.max(0.0), chroma * hue.cos(), chroma * hue.sin()];
            if s.vibrance == 0.0 && s.saturation == 0.0 {
                return ok.to_rgb(lab).map(|c| c.max(0.0));
            }
        }
        // Reductions act on every colour alike (-100 Saturation = black and
        // white); only boosts are shaped by hue, chroma and the vibrance curve.
        let mut scale = if s.saturation >= 0.0 {
            1.0 + s.saturation / 100.0 * t.saturation_plus * hue_weight(hue, &t.saturation_hue)
        } else {
            1.0 + s.saturation / 100.0 * t.saturation_minus
        };
        if s.vibrance > 0.0 {
            let soft = t.vibrance_soft / (t.vibrance_soft + chroma); // 1 for muted colours, falls for strong ones
            scale *= 1.0 + self.vibrance_v * soft * 2.0 * hue_weight(hue, &t.vibrance_hue);
        } else if s.vibrance < 0.0 {
            scale *= 1.0 + s.vibrance / 100.0 * t.vibrance_minus;
        }
        // (With a look, Contrast's colour change is already in `chroma`.)
        let k = scale.max(0.0) * if has_look { 1.0 } else { kc };
        if t.sat_pp > 0.5 {
            // On linear ProPhoto (Lightroom's working space): the colour's distance
            // from the grey of the same luminance scaled by `k`.
            static M: std::sync::OnceLock<([[f32; 3]; 3], [[f32; 3]; 3])> = std::sync::OnceLock::new();
            let (to, from) = *M.get_or_init(|| {
                let to = crate::tools::rec2020_to_prophoto();
                (to, crate::whitebalance::invert3(&to))
            });
            let p = apply3(&to, ok.to_rgb(lab));
            let y = 0.2880 * p[0] + 0.7119 * p[1] + 0.0001 * p[2];
            return limit_boost(|kk| apply3(&from, p.map(|v| y + (v - y) * kk)), k);
        }
        limit_boost(|kk| ok.to_rgb([lab[0], lab[1] * kk, lab[2] * kk]), k)
    }
}

/// Rec.2020 luminance weights.
pub(crate) const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// CIELAB L* of a display-linear Rec.2020 colour (white = 1).
pub fn lightness(c: [f32; 3]) -> f32 {
    let y = (LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).max(0.0);
    if y > 0.008856 { 116.0 * y.cbrt() - 16.0 } else { 903.3 * y }
}

/// CIELAB C* of a display-linear Rec.2020 colour (D65 white = 1).
pub fn cielab_chroma(c: [f32; 3]) -> f32 {
    let xyz = apply3(&REC2020_TO_XYZ, c);
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(xyz[0].max(0.0) / 0.95047), f(xyz[1].max(0.0)), f(xyz[2].max(0.0) / 1.08883));
    let (a, b) = (500.0 * (fx - fy), 200.0 * (fy - fz));
    (a * a + b * b).sqrt()
}

/// A colour's chroma scaled by `k` in OKLab (lightness and hue kept).
fn set_chroma(c: [f32; 3], k: f32, ok: &Oklab) -> [f32; 3] {
    let lab = ok.to_lab(c);
    ok.to_rgb([lab[0], lab[1] * k, lab[2] * k]).map(|v| v.max(0.0))
}

/// Back from L* to luminance.
pub fn lightness_to_y(l: f32) -> f32 {
    if l > 8.0 { ((l + 16.0) / 116.0).powi(3) } else { l / 903.3 }
}

/// Contrast in scene light: each pixel's distance from a pivot, measured in
/// stops, is scaled - e.g. +1 stop over the pivot becomes +`contrast_plus`
/// stops at +100 - and the pivot itself never moves. As in Lightroom, the
/// pivot sits a little above mid-grey (`contrast_pivot` stops), the bright
/// side moves less than the dark side (`contrast_bright`), and the effect
/// fades out towards white (`contrast_fade_from`..`contrast_fade_to` stops
/// over grey) so whites stay white. It works on luminance and scales the
/// colour with it, so hues and saturation ratios stay put.
pub fn contrast(c: [f32; 3], amount: f32, t: &Tuning) -> [f32; 3] {
    contrast_at(c, amount, t, t.tone_key_ref)
}

/// `contrast` for a photo whose mean log brightness is `key` (see `contrast_key`).
pub fn contrast_at(c: [f32; 3], amount: f32, t: &Tuning, key: f32) -> [f32; 3] {
    if amount == 0.0 {
        return c;
    }
    let k = if amount > 0.0 {
        1.0 + amount / 100.0 * (t.contrast_plus - 1.0)
    } else {
        1.0 + amount / 100.0 * (1.0 - t.contrast_minus)
    };
    let pivot = t.contrast_pivot + t.contrast_key * (key - t.tone_key_ref);
    // The mapping for one value (scene light).
    let map = |v: f32| -> f32 {
        if v <= 1e-6 {
            return v;
        }
        let ev = (v / 0.18).log2();
        let d = ev - pivot;
        if d > 0.0 {
            let fade = 1.0 - ramp(ev, t.contrast_fade_from, t.contrast_fade_to);
            let kk = 1.0 + (k - 1.0) * t.contrast_bright * fade;
            return v * 2f32.powf(pivot + d * kk - ev);
        }
        // Below the pivot: slope k, easing back to 1 between `contrast_toe_from`
        // and `contrast_toe_to` stops down (a toe, so channels far below grey -
        // a strong colour's weakest - aren't pushed down without limit; the
        // curve keeps rising, so tones stay in order). Off when `to` <= `from`.
        let a = -d;
        let (f, to) = (t.contrast_toe_from, t.contrast_toe_to);
        let off = if to <= f || a <= f {
            k * a
        } else if a <= to {
            k * f + (a - f) * (k - (k - 1.0) * (a - f) / (2.0 * (to - f)))
        } else {
            k * f + (to - f) * (k + 1.0) / 2.0 + (a - to)
        };
        v * 2f32.powf(pivot - off - ev)
    };
    if t.contrast_rgb > 0.5 {
        // Largest and smallest channel through the curve, the middle one at the
        // same proportion between them (hue kept, colour strength follows).
        // `contrast_rgb` 2: on linear ProPhoto channels (Lightroom's working
        // space), where saturated colours' channels sit further apart.
        let pp = t.contrast_rgb > 1.5;
        let mats = pp.then(|| {
            static M: std::sync::OnceLock<([[f32; 3]; 3], [[f32; 3]; 3])> = std::sync::OnceLock::new();
            *M.get_or_init(|| {
                let to = crate::tools::rec2020_to_prophoto();
                (to, crate::whitebalance::invert3(&to))
            })
        });
        let c = mats.map_or(c, |(to, _)| apply3(&to, c));
        let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        if hi <= 1e-6 {
            return mats.map_or(c, |(_, from)| apply3(&from, c));
        }
        let (nh, nl) = (map(hi), map(lo.max(0.0)));
        // `contrast_rgb` 3: each ProPhoto channel through the curve on its own.
        let out = if t.contrast_rgb > 2.5 { c.map(|v| map(v.max(0.0))) } else { c.map(|v| if hi - lo < 1e-9 { nh } else { nl + (nh - nl) * (v - lo) / (hi - lo) }) };
        return mats.map_or(out, |(_, from)| apply3(&from, out));
    }
    let l = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    if l <= 1e-6 {
        return c;
    }
    let scale = map(l) / l;
    c.map(|v| v * scale)
}

/// Contrast and the four tone sliders, in the order `contrast_after_tone` sets.
fn contrast_and_tone(lit: [f32; 3], s: &RenderSettings, t: &Tuning, offset: f32, key: f32, span: f32) -> [f32; 3] {
    if t.contrast_after_tone > 1.5 {
        // 2: Whites and Blacks, then Contrast, then Highlights and Shadows.
        let wb = RenderSettings { highlights: 0.0, shadows: 0.0, ..s.clone() };
        let hs = RenderSettings { whites: 0.0, blacks: 0.0, ..s.clone() };
        tone_bands_span(contrast_at(tone_bands_span(lit, &wb, t, offset, key, span), s.contrast, t, key), &hs, t, offset, key, span)
    } else if t.contrast_after_tone > 0.5 {
        contrast_at(tone_bands_span(lit, s, t, offset, key, span), s.contrast, t, key)
    } else {
        tone_bands_span(contrast_at(lit, s.contrast, t, key), s, t, offset, key, span)
    }
}

/// The Kelvin a photo with no camera white balance (a TIFF or JPEG) counts as
/// neutral at, with tint 0.
pub const NEUTRAL_KELVIN: f32 = 5500.0;

/// White balance for a photo with no camera data: Temperature as a warm / cool
/// gain on red and blue (higher Kelvin = warmer, as for RAWs; per mired away
/// from `NEUTRAL_KELVIN`), Tint as a gain on green (positive = magenta).
/// A diagonal matrix on linear light.
pub fn simple_rebalance(kelvin: Option<f32>, tint: Option<f32>) -> [[f32; 3]; 3] {
    let kelvin = kelvin.unwrap_or(NEUTRAL_KELVIN).clamp(2000.0, 50000.0);
    let mireds_off = 1.0e6 / NEUTRAL_KELVIN - 1.0e6 / kelvin;
    let warm = (mireds_off * 0.0035).exp2();
    let green = (-tint.unwrap_or(0.0) * 0.003).exp2();
    [[warm, 0.0, 0.0], [0.0, green, 0.0], [0.0, 0.0, 1.0 / warm]]
}

pub fn render(scene: &SceneImage, settings: &RenderSettings, baseline: f32, wb: Option<&WhiteBalance>) -> RgbImage {
    render_with(scene, settings, baseline, wb, &settings.profile.tuning())
}

pub fn render_with(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    tuning: &Tuning,
) -> RgbImage {
    render_core(scene, settings, baseline, wb, tuning, None)
}

/// Renders with Adobe's own profile (`crate::adobe`) in place of a Shinobi
/// profile's tone curve and colour character: its matrices and HueSatMap
/// before exposure, its LookTable and tone curve after the tone sliders.
/// White balance comes from the profile render (built for the settings'
/// Kelvin and tint); every slider and tool works as with Shinobi.
pub fn render_adobe(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    adobe: &crate::dngcolour::ProfileRender,
) -> RgbImage {
    render_core(scene, settings, baseline, wb, &Tuning::adobe(), Some(adobe))
}

/// `render_adobe` with given slider tuning (for fitting the sliders to
/// Lightroom's Adobe Color).
pub fn render_adobe_with(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    adobe: &crate::dngcolour::ProfileRender,
    tuning: &Tuning,
) -> RgbImage {
    render_core(scene, settings, baseline, wb, tuning, Some(adobe))
}

fn render_core(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    tuning: &Tuning,
    adobe: Option<&crate::dngcolour::ProfileRender>,
) -> RgbImage {
    let bytes = render_core_q(scene, settings, baseline, wb, tuning, adobe, |v| (v * 255.0 + 0.5) as u8);
    RgbImage::from_raw(scene.width as u32, scene.height as u32, bytes).expect("buffer size")
}

/// As `render` / `render_adobe`, but 16 bits per channel: the same
/// picture, rounded to 0..65535 rather than 0..255 (for the hand-off to
/// RapidRetouch and 16-bit TIFF export).
pub fn render16(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    adobe: Option<&crate::dngcolour::ProfileRender>,
) -> image::ImageBuffer<image::Rgb<u16>, Vec<u16>> {
    let tuning = if adobe.is_some() { Tuning::adobe() } else { settings.profile.tuning() };
    let px = render_core_q(scene, settings, baseline, wb, &tuning, adobe, |v| (v * 65535.0 + 0.5) as u16);
    image::ImageBuffer::from_raw(scene.width as u32, scene.height as u32, px).expect("buffer size")
}

/// The render, each channel (display-encoded, 0..1) turned into the output
/// type by `q` (`as` saturates, so out-of-range values clip).
fn render_core_q<T: Copy + Send + 'static>(
    scene: &SceneImage,
    settings: &RenderSettings,
    baseline: f32,
    wb: Option<&WhiteBalance>,
    tuning: &Tuning,
    adobe: Option<&crate::dngcolour::ProfileRender>,
    q: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    // `Tuning::yellow_sat_extra`: the Mixer's Yellow Saturation moved on top of the edit.
    let with_yellow;
    let settings = if tuning.yellow_sat_extra != 0.0 {
        let mut s = settings.clone();
        s.tools.mixer[2][1] = (s.tools.mixer[2][1] + tuning.yellow_sat_extra).clamp(-100.0, 100.0);
        with_yellow = s;
        &with_yellow
    } else {
        settings
    };
    let gain = 2f32.powf(settings.exposure + baseline);
    // White balance and exposure are both linear, so they fold into one
    // matrix (with Adobe's profile its own render does the white balance).
    let rebalance = match wb {
        Some(wb) if adobe.is_none() && (settings.temperature.is_some() || settings.tint.is_some()) => wb.rebalance(
            settings.temperature.unwrap_or(wb.shot_kelvin),
            settings.tint.unwrap_or(wb.shot_tint),
        ),
        // A photo with no camera white balance (TIFF, JPEG): a plain warm / cool
        // and green / magenta gain.
        None if settings.profile == Profile::AsIs && (settings.temperature.is_some() || settings.tint.is_some()) => {
            simple_rebalance(settings.temperature, settings.tint)
        }
        _ => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    let mut light = rebalance.map(|row| row.map(|v| v * gain));
    // Shinobi Colour's correction for this camera model comes first.
    if let (Some(fix), None) = (settings.camera_fix, adobe) {
        light = mul3(&light, &fix);
    }
    // The as-shot red x blue white-balance multipliers, for the shadow tint.
    let wb_gain = wb.map(|w| 1.0 / (w.neutral_shot[0] * w.neutral_shot[2]).max(1e-3)).unwrap_or(3.0);
    // The camera's own channels, for Fine tuning.
    let camera = wb.map(|wb| {
        let from_cam = wb.camera_to_rec2020(wb.shot_kelvin, wb.shot_tint);
        (crate::whitebalance::invert3(&from_cam), from_cam)
    });
    // Fine tuning's primaries matrix is linear too, so it folds in as well.
    if let Some(cal) = crate::tools::calibration_matrix(&settings.tools, &tuning.tools, camera.as_ref()) {
        light = mul3(&cal, &light);
    }
    let curve = ToneCurve::with(&tuning.curve);
    // The profile curve on linear ProPhoto channels (`curve_pp`), as Adobe
    // applies a profile's tone curve, or on Rec.2020's.
    let (pp_to, pp_from) = { let to = crate::tools::rec2020_to_prophoto(); (to, crate::whitebalance::invert3(&to)) };
    let base_curve = |x: [f32; 3]| if tuning.curve_pp > 0.5 { apply3(&pp_from, tone_rgb(&curve, apply3(&pp_to, x).map(|v| v.max(0.0)))) } else { tone_rgb(&curve, x) };
    let to_srgb = mul3(&XYZ_TO_SRGB, &REC2020_TO_XYZ);
    let oklab = Oklab::new();
    let mut display_tools = crate::tools::Prepared::new(&settings.tools, &tuning.tools);
    // A part of the photo (zoomed-in detail): placed in the whole photo, with
    // what was measured on the whole photo.
    let region = settings.region.clone();
    let frame = region.as_ref().map(|r| r.frame);
    display_tools.aspect = match &frame {
        Some(f) => f.full_w / f.full_h.max(1.0),
        None => scene.width as f32 / scene.height.max(1) as f32,
    };
    // Mixer Hue and Saturation before the tone controls, as Lightroom (see
    // `ToolTuning::mix_before_tone`); Luminance stays on the shown colour.
    let mix_scene = tuning.tools.mix_before_tone > 0.5;
    let (w, h) = (scene.width, scene.height);
    let grads = match &frame {
        Some(f) => crate::gradient::Prepared::new(&settings.gradients, f.full_w.round() as usize, f.full_h.round() as usize, settings.subject_mask.as_deref()),
        None => crate::gradient::Prepared::new(&settings.gradients, w, h, settings.subject_mask.as_deref()),
    };
    // Pixel i as fractions of the whole photo.
    let frac = |i: usize| match &frame {
        Some(f) => f.frac(i, w),
        None => (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32),
    };
    // With Adobe's profile: its working space and back, and Vibrance /
    // Saturation without a Shinobi colour character on top.
    let to_pp = crate::tools::rec2020_to_prophoto();
    let from_pp = crate::whitebalance::invert3(&to_pp);
    let plain = Tuning { look_chroma: [1.0, 0.0, 0.0, 0.0, 0.0], look_hue: [0.0; 4], look_light: [0.0; 4], look_comp: 0.0, look_lut: [[0.0; 3]; 12], look_warm: [[0.0; 2]; 5], ..*tuning };
    // The local tone sliders' map of area brightness, when they need one.
    let _p_map = crate::prof::span("  tone map (area brightness)");
    // A render the GPU will try works out each pixel's offset from the map's grid itself; the CPU
    // works them out only if it renders after all (`lazy_offsets`).
    let gpu_first = crate::gpu::mode() != crate::gpu::RenderMode::Cpu && crate::gpu::unsupported(tuning).is_none();
    let mut tone_lit: Option<Vec<f32>> = None;
    let tone_map = if ToneMap::wanted(settings, tuning) {
        let lit: Vec<f32> = scene.px.par_iter().map(|&p| {
            let c = apply3(&light, p);
            LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]
        }).collect();
        // A part of the photo reads the whole photo's map at its place.
        let m = match (&region, &frame) {
            (Some(r), Some(f)) if r.measured.tone_map.is_some() => r.measured.tone_map.as_ref().unwrap().for_part(&lit, scene.width, f, tuning),
            _ => ToneMap::build_with(&lit, scene.width, scene.height, tuning, !gpu_first),
        };
        tone_lit = Some(lit);
        Some(m)
    } else {
        None
    };
    let lazy_offsets = std::sync::OnceLock::new();
    let at = |i: usize| {
        tone_map.as_ref().map_or((0.0, tuning.tone_key_ref, tuning.tone_span_ref), |m| {
            let offset = if m.offset.is_empty() { lazy_offsets.get_or_init(|| m.grid.offsets(tone_lit.as_deref().unwrap_or(&[]), tuning)) } else { &m.offset };
            (offset[i], m.key, m.span)
        })
    };
    // The photo's brightness statistics, for photo-adaptive Highlights / Shadows.
    let with_stats;
    let tuning = match &tone_map {
        Some(m) if tuning.hs_adaptive > 0.5 => { with_stats = Tuning { photo_stats: m.stats, ..*tuning }; &with_stats }
        _ => tuning,
    };
    // The measured tone table: the four tone sliders come off the scene-light
    // bands and go on the displayed lightness afterwards, their strength set
    // by the photo's own lightness (measured below, without them).
    let amounts = [settings.highlights, settings.shadows, settings.whites, settings.blacks];
    let table = (tuning.tone_table > 0.5 && amounts.iter().any(|&a| a != 0.0)).then(crate::tonetable::table).flatten();
    let adapt = table.is_none() && tuning.tone_adapt != 0.0 && (settings.highlights != 0.0 || settings.shadows != 0.0 || settings.whites > 0.0);
    let zeroed = RenderSettings { highlights: 0.0, shadows: 0.0, whites: 0.0, blacks: 0.0, ..settings.clone() };
    // Everything up to the rendered colour, for one pixel, before the table.
    // Mixer Hue and Saturation where Lightroom seems to apply them: on the
    // colour as it would be shown without the tone sliders (base curve, look,
    // Vibrance - where the Mixer was measured), carried back to scene light by
    // solving for the scene colour that shows as the mixed one, so the tone
    // sliders then act on the mixed colour.
    // No Mixer Hue or Saturation: the target is the colour itself, so a colour the solve starts from
    // unchanged (no channel under its 1e-4 floor) comes back exactly as it went in, and the eight
    // rounds (nine colour renders a pixel, most of a render's time, 24 Oct) are skipped.
    let mixer_moves = !display_tools.mixer_part_is_identity([true, true, false]);
    // Vibrance, Saturation and the look's colour, set up once for every pixel (the tone settings
    // the closures below pass on differ only in the four tone sliders, which this doesn't read).
    let adjust = ColourAdjust::new(settings, tuning);
    let adjust_plain = ColourAdjust::new(settings, &plain);
    let mixed_scene = |lit: [f32; 3], shown_of: &dyn Fn([f32; 3]) -> [f32; 3]| -> [f32; 3] {
        if !mixer_moves && lit.iter().all(|&v| v >= 1e-4) {
            return lit;
        }
        let shown_lit = shown_of(lit);
        let target = display_tools.apply_mixer_part(shown_lit, &oklab, [true, true, false]);
        match MIX_SOLVE_COMPARE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => mixer_solve(lit, shown_lit, target, shown_of, &oklab),
            0 => mixer_solve_before(lit, shown_lit, target, shown_of),
            _ => mixer_solve_with(lit, shown_lit, target, shown_of, &oklab, 60, 1e-6, f32::INFINITY),
        }
    };
    let tone_with = |settings: &RenderSettings, i: usize, p: [f32; 3]| -> [f32; 3] {
        let (offset, key, span) = at(i);
        if let Some(pr) = adobe {
            // Fine tuning (in `light`, with exposure), then the profile's
            // matrices and HueSatMap - which come before exposure in the DNG
            // order, so exposure is divided out and put back after them.
            let pre = apply3(&light, p).map(|v| v.max(0.0) / gain);
            let lit = apply3(&from_pp, pr.working(pre).map(|v| v * gain));
            let lit = crate::tools::shadow_tint(lit, settings.tools.cal_shadow_tint * settings.shadow_tint_scale.unwrap_or(1.0), &tuning.tools, camera.as_ref(), wb_gain);
            let lit = if mix_scene { mixed_scene(lit, &|x| adjust_plain.apply(adobe_fix(apply3(&from_pp, pr.finish(apply3(&to_pp, x))), tuning), &oklab)) } else { lit };
            let shaped = contrast_and_tone(lit, settings, tuning, offset, key, span);
            return adobe_fix(apply3(&from_pp, pr.finish(apply3(&to_pp, shaped))), tuning);
        }
        let lit = apply3(&light, p).map(|v| v.max(0.0));
        let lit = crate::tools::shadow_tint(lit, settings.tools.cal_shadow_tint * settings.shadow_tint_scale.unwrap_or(1.0), &tuning.tools, camera.as_ref(), wb_gain);
        let lit = if mix_scene { mixed_scene(lit, &|x| adjust.apply(base_curve(x), &oklab)) } else { lit };
        let shaped = contrast_and_tone(lit, settings, tuning, offset, key, span);
        base_curve(shaped)
    };
    // The photo's lightness without the tone sliders, from every 7th pixel.
    drop(_p_map);
    let _p_stats = crate::prof::span("  lightness stats");
    let stats = (table.is_some() || adapt).then(|| {
        // A part of the photo: the whole photo's (measured with it).
        if let Some(st) = region.as_ref().and_then(|r| r.measured.stats.clone()) {
            return st;
        }
        // Measured without Exposure (it scaled the measures out of the kit's
        // range on edits with big Exposure moves).
        let k = if table.is_some() || tuning.tone_adapt_no_exposure > 0.5 { 2f32.powf(-settings.exposure) } else { 1.0 };
        let l: Vec<f32> = (0..scene.px.len()).into_par_iter().step_by(7).map(|i| lightness(tone_with(&zeroed, i, scene.px[i].map(|v| v * k)))).collect();
        crate::tonetable::PhotoStats::of(l)
    });
    if let Some(slot) = &settings.measure_into {
        if region.is_none() {
            *slot.lock().unwrap() = Some(Measured { tone_map: tone_map.clone(), stats: stats.clone(), definition: None });
        }
    }
    let bands = match (&table, &stats) {
        (Some(_), _) => zeroed.clone(),
        (None, Some(st)) if adapt => {
            let a = tuning.tone_adapt;
            let f = |x: f32| x.exp().clamp(0.4, 2.5);
            RenderSettings {
                highlights: settings.highlights * f(tuning.tone_adapt_h * a * (st.median - tuning.tone_adapt_hc)),
                shadows: settings.shadows * f(tuning.tone_adapt_s * a * (st.p75 - tuning.tone_adapt_sc)),
                whites: if settings.whites > 0.0 { settings.whites * f(tuning.tone_adapt_w * a * (st.p98 - tuning.tone_adapt_wc)) } else { settings.whites },
                ..settings.clone()
            }
        }
        _ => settings.clone(),
    };
    let tone_base = |i: usize, p: [f32; 3]| tone_with(&bands, i, p);
    // Everything up to the rendered colour, for one pixel.
    let tone = |i: usize, p: [f32; 3]| {
        // Gradients' Exposure and white balance, on the scene light.
        let p = match &grads { Some(g) => { let (fx, fy) = frac(i); g.at_frac(fx, fy).scene(p) } None => p };
        let mut shown = tone_base(i, p);
        if let (Some(t), Some(st)) = (table, &stats) {
            // New lightness from the table, colour scaled with it; then the
            // colour strength set to the table's change from the original.
            let l = lightness(shown);
            let (target, chroma) = t.apply_both(l, amounts, st);
            let (y0, y1) = (lightness_to_y(l), lightness_to_y(target));
            if y0 > 1e-7 {
                let c0 = cielab_chroma(shown);
                let scaled = shown.map(|v| v * y1 / y0);
                let c1 = cielab_chroma(scaled);
                shown = if c1 > 0.5 { set_chroma(scaled, (c0 * chroma / c1).clamp(0.0, 4.0), &oklab) } else { scaled };
            }
        }
        if adobe.is_some() {
            adjust_plain.apply(shown, &oklab)
        } else {
            adjust.apply(shown, &oklab)
        }
    };
    // The display tools, then sRGB bytes.
    let finish = |i: usize, mut toned: [f32; 3]| {
        if let Some(g) = &grads {
            let (fx, fy) = frac(i);
            toned = g.at_frac(fx, fy).display(toned);
        }
        if !display_tools.is_identity() {
            // Position relative to the frame: -1..1 across each axis, so a
            // radius of 1 is an ellipse touching the edges (like Lightroom's
            // vignette at Roundness 0).
            let (fx, fy) = frac(i);
            let (u, v) = (fx * 2.0 - 1.0, fy * 2.0 - 1.0);
            toned = if mix_scene {
                let lum = display_tools.apply_mixer_part(toned, &oklab, [false, false, true]);
                display_tools.apply_after_mixer(lum, &oklab, u, v)
            } else {
                display_tools.apply(toned, &oklab, u, v)
            };
        }
        // Colours outside sRGB (negative in sRGB) are clipped, as Lightroom's relative
        // colorimetric export does; Saturation / Vibrance are capped earlier (`limit_boost`).
        let mut out = apply3(&to_srgb, toned).map(srgb_encode);
        if tuning.display_curve != [0.0; 4] {
            out = out.map(|v| display_curve(v, &tuning.display_curve));
        }
        if tuning.display_blue_curve != [0.0; 2] {
            let [x, y] = tuning.display_blue_curve;
            out[2] = monotone_curve(out[2], &[(0.0, 0.0), (x, y), (1.0, 1.0)]);
        }
        if tuning.blue_s != 0.0 {
            let x = out[2].clamp(0.0, 1.0);
            let half = tuning.blue_s_half as i32;
            let on = match half { 1 => x < 0.5, 2 => x > 0.5, _ => true };
            let (hi, lo) = (out[0].max(out[1]).max(out[2]), out[0].min(out[1]).min(out[2]));
            let w = if half >= 3 { if out[2] <= lo && hi > 1e-4 { (hi - lo) / hi } else { 0.0 } } else { 1.0 };
            let w = if half == 4 && out[0] > 1e-4 { w * ((out[1] / out[0] - 0.85) / 0.15).clamp(0.0, 1.0) } else { w };
            if on && w > 0.0 {
                out[2] = x - w * tuning.blue_s * (std::f32::consts::TAU * x).sin() / std::f32::consts::TAU;
            }
        }
        out.map(&q)
    };
    drop(_p_stats);
    // The GPU engine (`gpu`) does the per-pixel steps when the Render setting lets it and it covers
    // everything this render needs; else (and whenever the GPU fails) the CPU does, below.
    let gpu_job = (crate::gpu::mode() != crate::gpu::RenderMode::Cpu)
        .then(|| match crate::gpu::unsupported(tuning).or(table.is_some().then_some("the measured tone table")) {
            Some(why) => {
                crate::gpu::note_cpu(why);
                None
            }
            None => Some(crate::gpu::Job {
                scene,
                settings,
                bands: &bands,
                tuning,
                light,
                gain,
                wb_gain,
                camera: camera.as_ref(),
                curve: &curve,
                display_tools: &display_tools,
                has_look: adjust.has_look,
                kc_full: adjust.kc_full,
                vibrance_v: adjust.vibrance_v,
                mix_scene,
                mixer_moves,
                frame,
                tone_map: tone_map.as_ref(),
                adobe,
                grads: grads.as_ref(),
                sixteen: std::any::TypeId::of::<T>() == std::any::TypeId::of::<u16>(),
                detail: crate::gpu::wanted_detail(),
            }),
        })
        .flatten();
    let local_def = grads.as_ref().is_some_and(|g| g.has_definition());
    let px: Vec<T> = if crate::tools::definition_active(&settings.tools) || local_def {
        // Texture, Clarity and Dehaze need each pixel's neighbourhood, so
        // render the whole image first, then finish it.
        let def_part = || crate::tools::DefinitionPart {
            frame: frame.as_ref(),
            measured: region.as_ref().map(|r| &r.measured).or(settings.reuse_measured.as_ref()).and_then(|m| m.definition.as_ref()),
        };
        // All of it on the GPU when it can (the toned picture never leaves it), the masks' too.
        if let Some(j) = &gpu_job {
            if let Some((out, measured)) = crate::gpu::render_definition(j, &settings.tools, &tuning.tools, scene.source_long, def_part()) {
                if let Some(px) = gpu_pixels(out) {
                    if let (Some(slot), Some(measured)) = (&settings.measure_into, measured) {
                        if let Some(m) = slot.lock().unwrap().as_mut() {
                            m.definition = Some(measured);
                        }
                    }
                    return px;
                }
            }
        }
        let p_tone = crate::prof::span("  per-pixel tone (before Definition)");
        let on_gpu = gpu_job.as_ref().and_then(|j| match crate::gpu::render(j, crate::gpu::Pass::Tone, None) {
            Some(crate::gpu::Output::Toned(t)) => Some(t),
            _ => None,
        });
        let mut toned: Vec<[f32; 3]> = on_gpu.unwrap_or_else(|| scene.px.par_iter().enumerate().map(|(i, &p)| tone(i, p)).collect());
        drop(p_tone);
        let _p_def = crate::prof::span("  Definition (Texture/Clarity/Dehaze)");
        let def = def_part();
        if crate::tools::definition_active(&settings.tools) {
            let def_measured = crate::tools::apply_definition(&mut toned, w, h, scene.source_long, &settings.tools, &tuning.tools, &oklab, def);
            if let Some(slot) = &settings.measure_into {
                if let Some(m) = slot.lock().unwrap().as_mut() {
                    m.definition = def_measured;
                }
            }
        }
        // The gradients' Texture, Clarity and Dehaze: each tool run over the
        // picture at the strongest amount the gradients reach (each way), and
        // faded in pixel by pixel by the gradients' amount there. The same
        // tools and numbers as the global sliders (not refitted to
        // Lightroom's local ones yet).
        // Each measures on the whole photo once, as the sliders' tools do: a slider drag or a
        // zoomed-in part reuses that (`DefinitionMeasured::masks`).
        if let Some(g) = grads.as_ref().filter(|g| g.has_definition()) {
            let amounts: Vec<[f32; 3]> = (0..toned.len()).into_par_iter().map(|i| { let (fx, fy) = frac(i); g.at_frac(fx, fy).definition }).collect();
            let mut mask_measured = Vec::new();
            for tool in 0..3 {
                for (way, sign) in [1.0f32, -1.0].into_iter().enumerate() {
                    let peak = amounts.iter().map(|a| a[tool] * sign).fold(0.0f32, f32::max);
                    if peak < 0.5 {
                        continue;
                    }
                    let amount = (peak * sign).clamp(-100.0, 100.0);
                    let one = crate::tools::ToolSettings {
                        texture: if tool == 0 { amount } else { 0.0 },
                        clarity: if tool == 1 { amount } else { 0.0 },
                        dehaze: if tool == 2 { amount } else { 0.0 },
                        ..Default::default()
                    };
                    let mut done = toned.clone();
                    let key = tool * 2 + way;
                    let reuse = def.measured.and_then(|m| m.mask(key, &one, &tuning.tools));
                    let part = crate::tools::DefinitionPart { frame: frame.as_ref(), measured: reuse };
                    if let Some(m) = crate::tools::apply_definition(&mut done, w, h, scene.source_long, &one, &tuning.tools, &oklab, part) {
                        mask_measured.push((key, m));
                    }
                    toned.par_iter_mut().zip(done.par_iter()).zip(amounts.par_iter()).for_each(|((c, d), a)| {
                        let k = (a[tool] * sign / peak).clamp(0.0, 1.0);
                        if k > 0.0 {
                            *c = std::array::from_fn(|j| c[j] + (d[j] - c[j]) * k);
                        }
                    });
                }
            }
            if let Some(slot) = settings.measure_into.as_ref().filter(|_| !mask_measured.is_empty()) {
                if let Some(m) = slot.lock().unwrap().as_mut() {
                    m.definition.get_or_insert_with(Default::default).masks = mask_measured;
                }
            }
        }
        drop(_p_def);
        if let Some(px) = gpu_job.as_ref().and_then(|j| crate::gpu::render(j, crate::gpu::Pass::Finish, Some(&toned))).and_then(gpu_pixels) {
            return px;
        }
        let _p = crate::prof::span("  per-pixel finish (display, sRGB)");
        toned.into_par_iter().enumerate().flat_map_iter(|(i, c)| finish(i, c)).collect()
    } else {
        if let Some(px) = gpu_job.as_ref().and_then(|j| crate::gpu::render(j, crate::gpu::Pass::Full, None)).and_then(gpu_pixels) {
            return px;
        }
        let _p = crate::prof::span("  per-pixel tone + finish");
        scene.px.par_iter().enumerate().flat_map_iter(|(i, &p)| finish(i, tone(i, p))).collect()
    };
    px
}

/// A GPU render's pixels as `render_core_q`'s output type.
fn gpu_pixels<T: 'static>(out: crate::gpu::Output) -> Option<Vec<T>> {
    let any: Box<dyn std::any::Any> = match out {
        crate::gpu::Output::Bytes(v) => Box::new(v),
        crate::gpu::Output::Words(v) => Box::new(v),
        crate::gpu::Output::Toned(_) => return None,
    };
    any.downcast::<Vec<T>>().ok().map(|v| *v)
}

pub fn render_dynamic(scene: &SceneImage, settings: &RenderSettings, baseline: f32, wb: Option<&WhiteBalance>) -> DynamicImage {
    DynamicImage::ImageRgb8(render(scene, settings, baseline, wb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_grey_lands_on_target() {
        let curve = ToneCurve::standard();
        assert!((curve.apply(0.18) - MID_GREY_OUT).abs() < 1e-3);
    }

    #[test]
    fn curve_is_monotonic_and_bounded() {
        for profile in [Profile::ShinobiStandard, Profile::ShinobiColour] {
            let curve = ToneCurve::with(&profile.tuning().curve);
            let mut prev = -1.0;
            let mut white_at = f32::MAX;
            for i in 0..2000 {
                let x = i as f32 * 0.01; // up to 20, far above white
                let y = curve.apply(x);
                assert!(y >= prev && (0.0..=1.0).contains(&y), "{profile:?}: x {x} y {y}");
                if y >= 1.0 && white_at == f32::MAX {
                    white_at = x;
                }
                prev = y;
            }
            // Like Lightroom's default rendering, white is reached a few stops
            // over mid-grey - but not so soon that bright skin and skies clip.
            let stops = (white_at / 0.18).log2();
            println!("{profile:?}: white at {stops:.2} stops over mid-grey");
            assert!(stops > 2.0, "{profile:?}: white only {stops:.2} stops over mid-grey");
        }
    }

    #[test]
    fn tone_keeps_hue_ratio() {
        let curve = ToneCurve::standard();
        let out = tone_rgb(&curve, [0.4, 0.2, 0.1]);
        // Middle channel sits at the same proportion between min and max.
        let before = (0.2 - 0.1) / (0.4 - 0.1);
        let after = (out[1] - out[2]) / (out[0] - out[2]);
        assert!((before - after).abs() < 1e-5, "{out:?}");
        // Greys stay grey.
        let g = tone_rgb(&curve, [0.3, 0.3, 0.3]);
        assert!(g[0] == g[1] && g[1] == g[2]);
    }

    #[test]
    fn a_boost_past_the_edge_is_eased_to_fit_and_a_fitting_one_is_left_alone() {
        // Boosting chroma on a line from a grey: red goes negative at strength 3.
        let line = |k: f32| [0.3 + 0.2 * k, 0.3 - 0.1 * k, 0.3 - 0.1 * k];
        assert_eq!(limit_boost(line, 1.5), line(1.5));
        let eased = limit_boost(line, 6.0);
        assert!(eased.iter().all(|&v| v >= 0.0));
        assert!((eased[1] - 0.0).abs() < 1e-3, "stops at the edge: {eased:?}");
        // Already outside before the boost: left as it was (clipped), not eased.
        let outside = |k: f32| [0.3, -0.05 * k, 0.1];
        assert_eq!(limit_boost(outside, 2.0), [0.3, 0.0, 0.1]);
    }

    #[test]
    fn clipped_highlights_go_neutral() {
        // A blown pixel: every raw channel at its ceiling after white balance.
        let clip = [2.0, 1.0, 1.5];
        let scene = scene_from_camera(1, 1, &[clip], clip, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let p = scene.px[0];
        assert!((p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6, "{p:?}");
        assert!(p[0] >= 2.0, "keeps its brightness: {p:?}");
    }

    #[test]
    fn contrast_pivots_near_grey_and_stays_a_photo() {
        // The pivot (a little above mid-grey, as in Lightroom) never moves.
        let t = Tuning::default();
        let p = 0.18 * 2f32.powf(t.contrast_pivot);
        assert!((contrast([p; 3], 100.0, &t)[1] - p).abs() < 1e-5);
        assert!((contrast([p; 3], -100.0, &t)[1] - p).abs() < 1e-5);
        // One stop over grey: further at +100, closer (but not grey) at -100.
        let bright = [0.36, 0.36, 0.36];
        let up = contrast(bright, 100.0, &Tuning::default())[1];
        let down = contrast(bright, -100.0, &Tuning::default())[1];
        assert!(up > 0.36 && down < 0.36 && down > 0.22, "{up} {down}");
        // Colour ratios survive.
        let c = contrast([0.4, 0.2, 0.1], 60.0, &Tuning::default());
        assert!((c[0] / c[1] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn tone_bands_never_invert_tones() {
        // Every combination of the four sliders at -100, 0 or +100.
        for h in [-100.0, 0.0, 100.0] {
            for sh in [-100.0, 0.0, 100.0] {
                for w in [-100.0, 0.0, 100.0] {
                    for b in [-100.0, 0.0, 100.0] {
                        let s = RenderSettings { highlights: h, shadows: sh, whites: w, blacks: b, ..Default::default() };
                        let mut prev = 0.0;
                        for i in 0..1200 {
                            let ev = -9.0 + i as f32 * 0.01; // -9 to +3 stops around grey
                            let v = 0.18 * 2f32.powf(ev);
                            let out = tone_bands([v, v, v], &s, &Tuning::default())[1];
                            assert!(out > prev, "inversion at {ev} EV with {h} {sh} {w} {b}");
                            prev = out;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn tone_bands_hit_their_ranges() {
        let s = |f: fn(&mut RenderSettings)| {
            let mut r = RenderSettings::default();
            f(&mut r);
            r
        };
        let at = |ev: f32, set: &RenderSettings| (tone_bands([0.18 * 2f32.powf(ev); 3], set, &Tuning::default())[1] / (0.18 * 2f32.powf(ev))).log2();
        // Each slider works mostly in its own range, as measured in Lightroom.
        let hl = s(|r| r.highlights = -100.0);
        // About -0.9 stops at the top under Adobe Color (more under Adobe Standard).
        assert!(at(3.5, &hl) < -0.8 && at(-3.0, &hl).abs() < 0.1, "highlights");
        let sh = s(|r| r.shadows = 100.0);
        assert!(at(-2.8, &sh) > 1.5 && at(3.0, &sh).abs() < 0.1, "shadows");
        let wh = s(|r| r.whites = 100.0);
        assert!(at(5.0, &wh) > at(1.0, &wh) && at(-3.0, &wh) < 0.2, "whites");
        let bl = s(|r| r.blacks = -100.0);
        assert!(at(-3.0, &bl) < -2.0 && at(3.0, &bl).abs() < 0.2, "blacks");
    }

    #[test]
    fn orientation_6_rotates_clockwise() {
        // 2x1 image: left red, right blue. Rotated 90 deg clockwise it becomes
        // 1x2 with red on top.
        let s = SceneImage { width: 2, height: 1, px: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], source_long: 2 };
        let r = s.oriented(6);
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.px[0], [1.0, 0.0, 0.0]);
    }
}

/// The Adobe path's measured correction (`Tuning::adobe_fix_l` / `_c`) on a
/// display-linear Rec.2020 colour: lightness moved by the table's offset at
/// its CIELAB L*, colour strength (distance from the gray of the same
/// luminance) scaled by its factor.
fn adobe_fix(c: [f32; 3], t: &Tuning) -> [f32; 3] {
    if t.adobe_fix_l == [0.0; 11] && t.adobe_fix_c == [0.0; 11] {
        return c;
    }
    let y = (0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2]).max(0.0);
    if y <= 1e-6 {
        return c;
    }
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let l = 116.0 * f(y) - 16.0;
    let at = |tab: &[f32; 11]| {
        let p = (l / 10.0).clamp(0.0, 10.0);
        let i = (p.floor() as usize).min(9);
        tab[i] + (tab[i + 1] - tab[i]) * (p - i as f32)
    };
    let l2 = (l + at(&t.adobe_fix_l)).max(0.0);
    let fy = (l2 + 16.0) / 116.0;
    let y2 = if fy > 0.206893 { fy * fy * fy } else { (fy - 16.0 / 116.0) / 7.787 };
    let k = y2 / y;
    let g = 1.0 + at(&t.adobe_fix_c);
    c.map(|v| y2 + (v * k - y2) * g)
}
