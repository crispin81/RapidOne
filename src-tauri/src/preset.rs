//! Lightroom develop settings (`crs:`, Camera Raw Settings) read from XMP: a
//! Lightroom preset (`.xmp`) or a photo's own sidecar / embedded packet. The
//! result is a patch in RapidOne's terms - only what the XMP sets - since our
//! sliders take Lightroom's numbers as they are. Settings we can't apply yet
//! are listed in `skipped`, so the user knows what's missing.
//!
//! The XMP shapes Lightroom writes are few and fixed (attributes on
//! `rdf:Description`, a handful of child elements for curves, the profile's
//! Look and the preset's name), so this reads them directly.

use serde::Serialize;
use std::collections::BTreeMap;

/// Lightroom's develop settings in RapidOne's terms.
#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LightroomSettings {
    /// The preset's name, if it is a preset.
    pub name: Option<String>,
    /// Slider id (as the app names them) -> value, in Lightroom's numbers.
    pub sliders: BTreeMap<String, f32>,
    /// "asShot", or "custom" with `temperature` / `tint` (Lightroom's Kelvin
    /// and tint scale, which ours now matches).
    pub white_balance: Option<String>,
    pub temperature: Option<f32>,
    pub tint: Option<f32>,
    /// The profile as the app names it ("adobeColor", "shinobiStandard"); the
    /// app falls back to Shinobi Colour where Adobe Color isn't installed.
    pub profile: Option<String>,
    /// Mixer band ("red" .. "magenta") -> "hue" / "sat" / "lum" -> value.
    pub mixer: BTreeMap<String, BTreeMap<String, f32>>,
    /// Point curves in Lightroom's 0..255 units: "rgb", "r", "g", "b".
    pub curves: BTreeMap<String, Vec<[f32; 2]>>,
    /// Colour grading: "shadows" / "mids" / "highs" / "global" -> "hue" (0..360),
    /// "sat" (0..100), "lum" (-100..100); plus "blending" / "balance" under "".
    pub grade: BTreeMap<String, BTreeMap<String, f32>>,
    /// Lightroom settings present here that RapidOne doesn't apply yet.
    pub skipped: Vec<String>,
    /// The photo's crop in RapidOne's terms (crop.ts), when it has one and the
    /// photo is known (`for_photo`).
    pub crop: Option<ImportedCrop>,
    /// Lightroom's crop as written, until `for_photo` converts it.
    #[serde(skip)]
    pub lightroom_crop: Option<LightroomCrop>,
    /// The photo's Linear and Radial gradients in RapidOne's terms (`for_photo`).
    pub gradients: Vec<crate::gradient::Gradient>,
    /// Lightroom's gradients as written (sensor frame), until `for_photo` turns them.
    #[serde(skip)]
    pub lightroom_gradients: Vec<crate::gradient::Gradient>,
    /// The XMP's own orientation (tiff:Orientation), if it sets one.
    #[serde(skip)]
    pub orientation: Option<u16>,
}

/// Lightroom's masked corrections that are one Linear or one Radial gradient,
/// as RapidOne gradients still in the unturned sensor frame; and whether any
/// other kind of mask (brush, AI, combinations) was there.
///
/// Read from the gradient kit (17 Oct): positions are fractions of the
/// unturned sensor picture, like the crop; Radial acts OUTSIDE the oval
/// unless Flipped; local Exposure is stored as stops / 4, the other sliders as
/// slider / 100. An oval's Angle is kept as Lightroom's (negated) here and
/// converted in `turn_gradients` (`gradient::lightroom_oval`), which needs the
/// photo's shape.
fn lightroom_gradients(rest: &str) -> (Vec<crate::gradient::Gradient>, bool) {
    use crate::gradient::{Gradient, Kind};
    let mut out = Vec::new();
    let mut other = false;
    for container in ["crs:MaskGroupBasedCorrections", "crs:GradientBasedCorrections", "crs:CircularGradientBasedCorrections"] {
        let Some(block) = take_block(rest, container).0 else { continue };
        // Each correction: its settings (an rdf:Description whose What is
        // "Correction") and, after it, its masks.
        let starts: Vec<usize> = block.match_indices("crs:What=\"Correction\"").map(|(i, _)| i).collect();
        for (n, &i) in starts.iter().enumerate() {
            let head_start = block[..i].rfind('<').unwrap_or(0);
            let head_end = block[i..].find('>').map(|e| i + e).unwrap_or(block.len());
            let head = &block[head_start..head_end];
            let end = starts.get(n + 1).copied().unwrap_or(block.len());
            let body = &block[head_end..end];
            let a = |k: &str| attr(head, k).and_then(|v| v.trim().parse::<f32>().ok());
            if attr(head, "crs:CorrectionActive").is_some_and(|v| v.eq_ignore_ascii_case("false")) {
                continue;
            }
            let masks: Vec<&str> = body
                .match_indices("crs:What=\"Mask/")
                .map(|(m, _)| {
                    let s = body[..m].rfind('<').unwrap_or(0);
                    let e = body[m..].find('>').map(|x| m + x).unwrap_or(body.len());
                    &body[s..e]
                })
                .collect();
            let mask = match masks.as_slice() {
                [m] => *m,
                _ => {
                    other = true;
                    continue;
                }
            };
            let what = attr(mask, "crs:What").unwrap_or_default();
            let m = |k: &str| attr(mask, k).and_then(|v| v.trim().parse::<f32>().ok());
            let inverted = attr(mask, "crs:MaskInverted").is_some_and(|v| v.eq_ignore_ascii_case("true"));
            if m("crs:MaskValue").is_some_and(|v| v < 0.999) || m("crs:MaskBlendMode").is_some_and(|v| v != 0.0) {
                other = true;
                continue;
            }
            let amount = a("crs:CorrectionAmount").unwrap_or(1.0);
            let mut g = Gradient {
                exposure: 4.0 * amount * a("crs:LocalExposure2012").unwrap_or(0.0),
                temperature: 100.0 * amount * a("crs:LocalTemperature").unwrap_or(0.0),
                tint: 100.0 * amount * a("crs:LocalTint").unwrap_or(0.0),
                contrast: 100.0 * amount * a("crs:LocalContrast2012").unwrap_or(0.0),
                highlights: 100.0 * amount * a("crs:LocalHighlights2012").unwrap_or(0.0),
                shadows: 100.0 * amount * a("crs:LocalShadows2012").unwrap_or(0.0),
                saturation: 100.0 * amount * a("crs:LocalSaturation").unwrap_or(0.0),
                whites: 100.0 * amount * a("crs:LocalWhites2012").unwrap_or(0.0),
                blacks: 100.0 * amount * a("crs:LocalBlacks2012").unwrap_or(0.0),
                texture: 100.0 * amount * a("crs:LocalTexture").unwrap_or(0.0),
                clarity: 100.0 * amount * a("crs:LocalClarity2012").unwrap_or(0.0),
                dehaze: 100.0 * amount * a("crs:LocalDehaze").unwrap_or(0.0),
                ..Default::default()
            };
            match what.as_str() {
                "Mask/Gradient" => {
                    g.kind = Kind::Linear;
                    g.full = [m("crs:FullX").unwrap_or(0.5), m("crs:FullY").unwrap_or(0.0)];
                    g.zero = [m("crs:ZeroX").unwrap_or(0.5), m("crs:ZeroY").unwrap_or(0.5)];
                    if inverted {
                        std::mem::swap(&mut g.full, &mut g.zero);
                    }
                }
                "Mask/CircularGradient" => {
                    let (t, l, b, r) = (m("crs:Top").unwrap_or(0.25), m("crs:Left").unwrap_or(0.25), m("crs:Bottom").unwrap_or(0.75), m("crs:Right").unwrap_or(0.75));
                    g.kind = Kind::Radial;
                    g.centre = [(l + r) / 2.0, (t + b) / 2.0];
                    g.rx = ((r - l) / 2.0).abs().max(1e-3);
                    g.ry = ((b - t) / 2.0).abs().max(1e-3);
                    g.angle = -m("crs:Angle").unwrap_or(0.0);
                    g.feather = m("crs:Feather").unwrap_or(50.0);
                    let flipped = attr(mask, "crs:Flipped").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                    // Lightroom: outside unless Flipped; RapidOne: inside unless invert.
                    g.invert = !flipped ^ inverted;
                }
                _ => {
                    other = true;
                    continue;
                }
            }
            out.push(g);
        }
    }
    (out, other)
}

/// Gradients from the sensor frame into the photo as shown: the XMP's
/// orientation (or the camera's) turns them, as it does the crop.
fn turn_gradients(list: &[crate::gradient::Gradient], orientation: Option<u16>, exif: u16, aspect: f32) -> Option<Vec<crate::gradient::Gradient>> {
    let turns = quarter_turns(orientation.unwrap_or(exif))?;
    Some(
        list.iter()
            .map(|g| {
                let mut g = g.clone();
                // An angled oval: Lightroom's turn (0..1 coordinates) as ours,
                // on the sensor picture (`aspect` = its width / height).
                if g.kind == crate::gradient::Kind::Radial {
                    let (c, rx, ry, a) = crate::gradient::lightroom_oval(g.centre, g.rx, g.ry, -g.angle, aspect);
                    (g.centre, g.rx, g.ry, g.angle) = (c, rx, ry, a);
                }
                for _ in 0..turns {
                    // A clockwise quarter turn: (x, y) -> (1 - y, x); radii swap.
                    let t = |p: [f32; 2]| [1.0 - p[1], p[0]];
                    g.full = t(g.full);
                    g.zero = t(g.zero);
                    g.centre = t(g.centre);
                    std::mem::swap(&mut g.rx, &mut g.ry);
                }
                g
            })
            .collect(),
    )
}

/// RapidOne's crop (crop.ts / crop.rs): quarter turns, straighten (degrees,
/// clockwise) and the kept rectangle as fractions of the turned canvas.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct ImportedCrop {
    pub quarter: u8,
    pub angle: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Lightroom's crop as its XMP has it, and the orientation it sets (if any).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightroomCrop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub angle: f32,
    pub orientation: Option<u16>,
}

/// Clockwise quarter turns that show a photo of this EXIF orientation upright
/// (mirrored orientations: None).
fn quarter_turns(orientation: u16) -> Option<u8> {
    match orientation {
        1 => Some(0),
        6 => Some(1),
        3 => Some(2),
        8 => Some(3),
        _ => None,
    }
}

impl LightroomCrop {
    /// In RapidOne's terms, for a photo whose sensor picture is `w` x `h` and
    /// whose camera orientation is `exif` (how RapidOne shows it upright).
    ///
    /// Lightroom's reading (crop kit, 17 Oct, every variant within resampling):
    /// (Left, Top) and (Right, Bottom) are OPPOSITE CORNERS of the crop, as
    /// fractions of the unturned sensor picture, and the crop's sides run at
    /// CropAngle - so its width and height come from that diagonal. A crop
    /// reaching outside the photo shrinks about its centre until it fits. The
    /// photo's orientation (the XMP's own, else the camera's) turns the result.
    pub fn to_rapidone(&self, w: f32, h: f32, exif: u16) -> Option<ImportedCrop> {
        let turns_lr = quarter_turns(self.orientation.unwrap_or(exif))?;
        let turns_exif = quarter_turns(exif)?;
        let th = self.angle.to_radians();
        let (ux, uy) = (th.cos(), th.sin());
        let (dx, dy) = ((self.right - self.left) * w, (self.bottom - self.top) * h);
        let (mut ow, mut oh) = (dx * ux + dy * uy, -dx * uy + dy * ux);
        if ow <= 1e-3 || oh <= 1e-3 {
            return None;
        }
        let (mut cx, mut cy) = ((self.left + self.right) / 2.0 * w, (self.top + self.bottom) / 2.0 * h);
        let mut k = 1.0f32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let (px, py) = (su * ow / 2.0 * ux - sv * oh / 2.0 * uy, su * ow / 2.0 * uy + sv * oh / 2.0 * ux);
            for (d, lim) in [(px, if px < 0.0 { cx } else { w - cx }), (py, if py < 0.0 { cy } else { h - cy })] {
                if d.abs() > 1e-6 {
                    k = k.min(lim.max(0.0) / d.abs());
                }
            }
        }
        ow *= k;
        oh *= k;
        // Into Lightroom's upright frame: each clockwise quarter turn takes
        // (x, y) on a w x h picture to (h - y, x); the crop's angle stays.
        let (mut fw, mut fh) = (w, h);
        for _ in 0..turns_lr {
            (cx, cy) = (fh - cy, cx);
            (fw, fh) = (fh, fw);
            (ow, oh) = (oh, ow);
        }
        // RapidOne: the canvas point p shows the photo at R(-angle) p (crop.rs),
        // so angle = -CropAngle and the crop's centre on the canvas is R(angle) C.
        let angle = -self.angle;
        let (s, c) = angle.to_radians().sin_cos();
        let (qx, qy) = (cx - fw / 2.0, cy - fh / 2.0);
        let (px, py) = (qx * c - qy * s, qx * s + qy * c);
        let x0 = (px + fw / 2.0 - ow / 2.0) / fw;
        let y0 = (py + fh / 2.0 - oh / 2.0) / fh;
        Some(ImportedCrop { quarter: (turns_lr + 4 - turns_exif) % 4, angle, x0, y0, x1: x0 + ow / fw, y1: y0 + oh / fh })
    }
}

impl LightroomSettings {
    /// True if the XMP carried any develop settings at all.
    pub fn is_empty(&self) -> bool {
        self.sliders.is_empty()
            && self.white_balance.is_none()
            && self.profile.is_none()
            && self.mixer.is_empty()
            && self.curves.is_empty()
            && self.grade.is_empty()
            && self.skipped.is_empty()
    }
}

impl LightroomSettings {
    /// These settings applied on top of `s` (what the engine renders), as the
    /// app applies them to a photo: only what they set changes. `adobe_available`:
    /// whether Adobe Color can be used; if not it falls back to Shinobi Colour.
    /// (Sliders the engine doesn't render yet - sharpening, noise, grain,
    /// transform - are left to the app.)
    pub fn apply_to(&self, s: &mut crate::pipeline::RenderSettings, adobe_available: bool) {
        use crate::pipeline::Profile;
        for (id, &v) in &self.sliders {
            let t = &mut s.tools;
            let slot: Option<&mut f32> = match id.as_str() {
                "exposure" => Some(&mut s.exposure),
                "contrast" => Some(&mut s.contrast),
                "highlights" => Some(&mut s.highlights),
                "shadows" => Some(&mut s.shadows),
                "whites" => Some(&mut s.whites),
                "blacks" => Some(&mut s.blacks),
                "vibrance" => Some(&mut s.vibrance),
                "saturation" => Some(&mut s.saturation),
                "texture" => Some(&mut t.texture),
                "clarity" => Some(&mut t.clarity),
                "dehaze" => Some(&mut t.dehaze),
                "vignette" => Some(&mut t.vignette),
                "vignetteMidpoint" => Some(&mut t.vignette_midpoint),
                "vignetteFeather" => Some(&mut t.vignette_feather),
                "vignetteRoundness" => Some(&mut t.vignette_roundness),
                "lensVignette" => Some(&mut s.lens_vignette),
                "calShadowTint" => Some(&mut t.cal_shadow_tint),
                "calRedHue" => Some(&mut t.cal_red_hue),
                "calRedSat" => Some(&mut t.cal_red_sat),
                "calGreenHue" => Some(&mut t.cal_green_hue),
                "calGreenSat" => Some(&mut t.cal_green_sat),
                "calBlueHue" => Some(&mut t.cal_blue_hue),
                "calBlueSat" => Some(&mut t.cal_blue_sat),
                "paramShadows" => Some(&mut t.param_shadows),
                "paramDarks" => Some(&mut t.param_darks),
                "paramLights" => Some(&mut t.param_lights),
                "paramHighlights" => Some(&mut t.param_highlights),
                "paramShadowSplit" => Some(&mut t.param_shadow_split),
                "paramMidSplit" => Some(&mut t.param_mid_split),
                "paramHighSplit" => Some(&mut t.param_high_split),
                _ => None,
            };
            if let Some(slot) = slot {
                *slot = v;
            }
        }
        match self.white_balance.as_deref() {
            Some("asShot") => (s.temperature, s.tint) = (None, None),
            Some("custom") => (s.temperature, s.tint) = (self.temperature, self.tint),
            _ => {}
        }
        match self.profile.as_deref() {
            Some("adobeColor") => s.profile = if adobe_available { Profile::AdobeColor } else { Profile::ShinobiColour },
            Some("adobeStandard") => s.profile = if adobe_available { Profile::AdobeStandard } else { Profile::ShinobiStandard },
            Some("shinobiStandard") => s.profile = Profile::ShinobiStandard,
            Some("shinobiColour") => s.profile = Profile::ShinobiColour,
            _ => {}
        }
        for (b, (_, band)) in BANDS.iter().enumerate() {
            if let Some(m) = self.mixer.get(*band) {
                for (k, key) in ["hue", "sat", "lum"].iter().enumerate() {
                    if let Some(&v) = m.get(*key) {
                        s.tools.mixer[b][k] = v;
                    }
                }
            }
        }
        for (key, curve) in [("rgb", &mut s.tools.curve_rgb), ("r", &mut s.tools.curve_red), ("g", &mut s.tools.curve_green), ("b", &mut s.tools.curve_blue)] {
            if let Some(points) = self.curves.get(key) {
                *curve = points.clone();
            }
        }
        let g = &mut s.tools.grade;
        for (name, range) in [("shadows", &mut g.shadows), ("mids", &mut g.mids), ("highs", &mut g.highs), ("global", &mut g.global)] {
            if let Some(m) = self.grade.get(name) {
                range.hue = m.get("hue").copied().unwrap_or(range.hue);
                range.sat = m.get("sat").copied().unwrap_or(range.sat);
                range.lum = m.get("lum").copied().unwrap_or(range.lum);
            }
        }
        if let Some(m) = self.grade.get("") {
            g.blending = m.get("blending").copied().unwrap_or(g.blending);
            g.balance = m.get("balance").copied().unwrap_or(g.balance);
        }
    }
}

/// Lightroom name -> the app's slider id.
const SLIDERS: [(&str, &str); 36] = [
    ("ParametricShadows", "paramShadows"),
    ("ParametricDarks", "paramDarks"),
    ("ParametricLights", "paramLights"),
    ("ParametricHighlights", "paramHighlights"),
    ("ParametricShadowSplit", "paramShadowSplit"),
    ("ParametricMidtoneSplit", "paramMidSplit"),
    ("ParametricHighlightSplit", "paramHighSplit"),
    ("Exposure2012", "exposure"),
    ("Contrast2012", "contrast"),
    ("Highlights2012", "highlights"),
    ("Shadows2012", "shadows"),
    ("Whites2012", "whites"),
    ("Blacks2012", "blacks"),
    ("Texture", "texture"),
    ("Clarity2012", "clarity"),
    ("Dehaze", "dehaze"),
    ("Vibrance", "vibrance"),
    ("Saturation", "saturation"),
    ("Sharpness", "sharpen"),
    ("LuminanceSmoothing", "noise"),
    ("PostCropVignetteAmount", "vignette"),
    ("PostCropVignetteMidpoint", "vignetteMidpoint"),
    ("PostCropVignetteFeather", "vignetteFeather"),
    ("PostCropVignetteRoundness", "vignetteRoundness"),
    ("PostCropVignetteHighlightContrast", "vignetteHighlights"),
    ("GrainAmount", "grain"),
    ("ShadowTint", "calShadowTint"),
    ("RedHue", "calRedHue"),
    ("RedSaturation", "calRedSat"),
    ("GreenHue", "calGreenHue"),
    ("GreenSaturation", "calGreenSat"),
    ("BlueHue", "calBlueHue"),
    ("BlueSaturation", "calBlueSat"),
    ("PerspectiveRotate", "rotate"),
    ("PerspectiveVertical", "vertical"),
    ("PerspectiveHorizontal", "horizontal"),
];

const BANDS: [(&str, &str); 8] = [
    ("Red", "red"),
    ("Orange", "orange"),
    ("Yellow", "yellow"),
    ("Green", "green"),
    ("Aqua", "aqua"),
    ("Blue", "blue"),
    ("Purple", "purple"),
    ("Magenta", "magenta"),
];

/// Settings we read but can't apply yet: (Lightroom name, its "does
/// nothing" value, what to call it, the setting it depends on). Listed as
/// skipped only when set to something else - and, for a setting that only
/// shapes another (vignette shape, grain size, sharpening detail), only when
/// that one is in use.
/// Colour noise reduction isn't listed: it changes only noise, not tone or
/// colour (Lightroom applies 25 to every RAW by default).
const NOT_YET: [(&str, &str, &str, Option<&str>); 13] = [
    ("SharpenRadius", "+1.0", "Sharpening radius/detail/masking", Some("Sharpness")),
    ("SharpenDetail", "25", "Sharpening radius/detail/masking", Some("Sharpness")),
    ("SharpenEdgeMasking", "0", "Sharpening radius/detail/masking", Some("Sharpness")),
    ("LuminanceNoiseReductionDetail", "50", "Noise reduction detail/contrast", Some("LuminanceSmoothing")),
    ("LensProfileEnable", "0", "Lens profile corrections", None),
    ("AutoLateralCA", "0", "Chromatic aberration removal", None),
    ("LensManualDistortionAmount", "0", "Manual lens distortion", None),
    ("VignetteAmount", "0", "Lens vignetting", None),
    ("DefringePurpleAmount", "0", "Defringe", None),
    ("DefringeGreenAmount", "0", "Defringe", None),
    ("GrainSize", "25", "Grain size/roughness", Some("GrainAmount")),
    ("PostCropVignetteStyle", "1", "Vignette style", Some("PostCropVignetteAmount")),
    ("ConvertToGrayscale", "False", "Black & white", None),
];

/// Reads Lightroom's develop settings from an XMP packet's text.
pub fn parse(xmp: &str) -> LightroomSettings {
    let mut out = LightroomSettings::default();
    // The profile's Look block has its own crs:Name (the profile's name), and
    // the preset's name sits in an rdf:Alt; take both out before reading the
    // plain settings.
    let (look_name, rest) = take_block(xmp, "crs:Look");
    let look_name = look_name.and_then(|b| attr(&b, "crs:Name").or_else(|| element(&b, "crs:Name")));
    let (name_block, rest) = take_block(&rest, "crs:Name");
    out.name = name_block.and_then(|b| element(&b, "rdf:li"));
    let a = attrs(&rest);
    let num = |k: &str| a.get(k).and_then(|v| v.trim().parse::<f32>().ok());

    for (lr, id) in SLIDERS {
        if let Some(v) = num(lr) {
            out.sliders.insert(id.to_string(), v);
        }
    }
    // Noise: RapidOne's is AI noise reduction (a blend towards the cleaned
    // photo, 0-100), not Lightroom's Luminance smoothing strength, so the
    // numbers don't match; any Luminance noise reduction in the edit means
    // "clean it", which is Noise 50 here (Chris, 16 Oct).
    if out.sliders.get("noise").is_some_and(|&v| v > 0.0) {
        out.sliders.insert("noise".into(), 50.0);
    }
    // Lightroom's Scale is a percentage (100 = unchanged), as ours is.
    if let Some(v) = num("PerspectiveScale") {
        out.sliders.insert("scale".into(), v);
    }
    // Lens profile corrections stay off for now: `lensprofile` reads Adobe's
    // vignetting, but on the S5II + Sigma 24mm real edits Lightroom's change is
    // nearly all distortion (the RW2's own corrections), and dividing out the
    // profile's vignetting made them worse.

    // White balance: as shot, or a custom Kelvin/Tint (a named preset such
    // as "Daylight" also stores its Kelvin/Tint, so it applies as custom).
    match a.get("WhiteBalance").map(|s| s.as_str()) {
        Some("As Shot") => out.white_balance = Some("asShot".into()),
        Some("Auto") => out.skipped.push("Auto white balance".into()),
        Some(_) | None => {
            if let (Some(k), Some(t)) = (num("Temperature"), num("Tint")) {
                out.white_balance = Some("custom".into());
                out.temperature = Some(k);
                out.tint = Some(t);
            } else if num("IncrementalTemperature").is_some() || num("IncrementalTint").is_some() {
                out.skipped.push("White balance for JPEG/TIFF (relative)".into());
            }
        }
    }

    // Profile: Adobe Color is a Look on Adobe Standard.
    let camera_profile = a.get("CameraProfile").cloned();
    match (look_name.as_deref(), camera_profile.as_deref()) {
        (Some(look), _) if look.eq_ignore_ascii_case("Adobe Color") => out.profile = Some("adobeColor".into()),
        (Some(look), _) => out.skipped.push(format!("Profile \"{look}\"")),
        // "Adobe Standard" and its newer "Adobe Standard v2".
        (None, Some(p)) if p.to_ascii_lowercase().starts_with("adobe standard") => out.profile = Some("adobeStandard".into()),
        (None, Some(p)) => out.skipped.push(format!("Profile \"{p}\"")),
        (None, None) => {}
    }

    for (lr, band) in BANDS {
        for (prefix, key) in [("HueAdjustment", "hue"), ("SaturationAdjustment", "sat"), ("LuminanceAdjustment", "lum")] {
            if let Some(v) = num(&format!("{prefix}{lr}")) {
                out.mixer.entry(band.to_string()).or_default().insert(key.to_string(), v);
            }
        }
    }

    for (lr, key) in [("ToneCurvePV2012", "rgb"), ("ToneCurvePV2012Red", "r"), ("ToneCurvePV2012Green", "g"), ("ToneCurvePV2012Blue", "b")] {
        if let Some(points) = curve(&rest, lr) {
            out.curves.insert(key.to_string(), points);
        }
    }

    let ranges = [
        ("shadows", "SplitToningShadowHue", "SplitToningShadowSaturation", "ColorGradeShadowLum"),
        ("mids", "ColorGradeMidtoneHue", "ColorGradeMidtoneSat", "ColorGradeMidtoneLum"),
        ("highs", "SplitToningHighlightHue", "SplitToningHighlightSaturation", "ColorGradeHighlightLum"),
        ("global", "ColorGradeGlobalHue", "ColorGradeGlobalSat", "ColorGradeGlobalLum"),
    ];
    for (range, hue, sat, lum) in ranges {
        for (k, lr) in [("hue", hue), ("sat", sat), ("lum", lum)] {
            if let Some(v) = num(lr) {
                out.grade.entry(range.to_string()).or_default().insert(k.to_string(), v);
            }
        }
    }
    for (k, lr) in [("blending", "ColorGradeBlending"), ("balance", "SplitToningBalance")] {
        if let Some(v) = num(lr) {
            out.grade.entry(String::new()).or_default().insert(k.to_string(), v);
        }
    }

    for (lr, neutral, label, needs) in NOT_YET {
        if needs.is_some_and(|k| num(k).is_none_or(|v| v == 0.0)) {
            continue;
        }
        let set = a.get(lr).is_some_and(|v| match (v.trim().parse::<f32>(), neutral.parse::<f32>()) {
            (Ok(x), Ok(n)) => (x - n).abs() > 1e-6,
            _ => !v.eq_ignore_ascii_case(neutral),
        });
        if set && !out.skipped.iter().any(|s| s == label) {
            out.skipped.push(label.to_string());
        }
    }
    // Crop: read here, converted once the photo is known (`for_photo`); a
    // preset's crop isn't applied.
    if a.get("HasCrop").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        let orientation = rest
            .split("tiff:Orientation=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|v| v.trim().parse::<u16>().ok());
        out.lightroom_crop = Some(LightroomCrop {
            left: num("CropLeft").unwrap_or(0.0),
            top: num("CropTop").unwrap_or(0.0),
            right: num("CropRight").unwrap_or(1.0),
            bottom: num("CropBottom").unwrap_or(1.0),
            angle: num("CropAngle").unwrap_or(0.0),
            orientation,
        });
        out.skipped.push("Crop".into());
    }
    // Masks and healing count only with entries in them (Lightroom often
    // writes the empty lists). Linear and Radial gradients are read (and
    // turned once the photo is known: `for_photo`); other masks are skipped.
    let used = |tag: &str| take_block(&rest, tag).0.is_some_and(|b| b.contains("<rdf:li"));
    let (gradients, other_masks) = lightroom_gradients(&rest);
    out.lightroom_gradients = gradients;
    out.orientation = rest
        .split("tiff:Orientation=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .and_then(|v| v.trim().parse::<u16>().ok());
    if other_masks || used("crs:PaintBasedCorrections") || (out.lightroom_gradients.is_empty() && ["crs:MaskGroupBasedCorrections", "crs:GradientBasedCorrections", "crs:CircularGradientBasedCorrections"].iter().any(|t| used(t))) {
        out.skipped.push("Masks and local adjustments".into());
    }
    if !out.lightroom_gradients.is_empty() {
        out.skipped.push("Gradients".into());
    }
    if used("crs:RetouchAreas") || used("crs:RedEyeInfo") {
        out.skipped.push("Healing / red eye".into());
    }
    out
}

/// Lightroom's develop settings for a photo: its sidecar (`photo.xmp`, or
/// `photo.ext.xmp` as some tools write it). None if there is no sidecar or it
/// carries no develop settings (e.g. only a rating).
pub fn for_photo(photo: &std::path::Path) -> Option<LightroomSettings> {
    let mut candidates = vec![photo.with_extension("xmp")];
    if let Some(name) = photo.file_name() {
        candidates.push(photo.with_file_name(format!("{}.xmp", name.to_string_lossy())));
    }
    candidates.into_iter().find_map(|p| {
        let xmp = std::fs::read_to_string(p).ok()?;
        let mut s = parse(&xmp);
        if s.lightroom_crop.is_some() || !s.lightroom_gradients.is_empty() {
            // The sensor picture's shape and the camera's orientation, from
            // the embedded preview (only the shape matters).
            if let Ok(e) = crate::preview::extract_source_image(photo, None) {
                if let Some(crop) = s.lightroom_crop.and_then(|c| c.to_rapidone(e.image.width() as f32, e.image.height() as f32, e.orientation)) {
                    s.crop = Some(crop);
                    s.skipped.retain(|k| k != "Crop");
                }
                let aspect = e.image.width() as f32 / e.image.height().max(1) as f32;
                if let Some(g) = turn_gradients(&s.lightroom_gradients, s.orientation, e.orientation, aspect) {
                    s.gradients = g;
                    s.skipped.retain(|k| k != "Gradients");
                }
            }
        }
        (!s.is_empty()).then_some(s)
    })
}

/// Every `crs:Key="value"` attribute (Lightroom's usual form) and simple
/// `<crs:Key>value</crs:Key>` element, keyed without the prefix. The first
/// occurrence wins: the photo's own settings come first, and Lightroom
/// repeats many keys later inside masks and other nested blocks.
fn attrs(xmp: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in xmp.split("crs:").skip(1) {
        let key: String = part.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let rest = &part[key.len()..];
        if let Some(r) = rest.strip_prefix("=\"") {
            if let Some((v, _)) = r.split_once('"') {
                out.entry(key).or_insert_with(|| unescape(v));
            }
        } else if let Some(r) = rest.strip_prefix('>') {
            // Element form: only plain text (not a nested rdf:Seq).
            if let Some((v, _)) = r.split_once('<') {
                if !v.trim().is_empty() {
                    out.entry(key).or_insert_with(|| unescape(v.trim()));
                }
            }
        }
    }
    out
}

fn unescape(v: &str) -> String {
    v.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&apos;", "'").replace("&amp;", "&")
}

/// The first `<tag ...>...</tag>` (or self-closing `<tag .../>`) block, and
/// the text without it.
fn take_block(xmp: &str, tag: &str) -> (Option<String>, String) {
    let open = format!("<{tag}");
    let Some(start) = xmp.match_indices(&open).map(|(i, _)| i).find(|&i| {
        // Not a longer name that merely starts the same (crs:LookTable...).
        xmp[i + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')
    }) else {
        return (None, xmp.to_string());
    };
    let Some(head_end) = xmp[start..].find('>').map(|i| start + i) else {
        return (None, xmp.to_string());
    };
    let end = if xmp[..head_end].ends_with('/') {
        head_end + 1
    } else {
        let close = format!("</{tag}>");
        match xmp[head_end..].find(&close) {
            Some(i) => head_end + i + close.len(),
            None => return (None, xmp.to_string()),
        }
    };
    (Some(xmp[start..end].to_string()), format!("{}{}", &xmp[..start], &xmp[end..]))
}

fn attr(block: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let i = block.find(&needle)? + needle.len();
    Some(unescape(&block[i..i + block[i..].find('"')?]))
}

/// The text of the first `<name ...>text</name>`.
fn element(block: &str, name: &str) -> Option<String> {
    let start = block.find(&format!("<{name}"))?;
    let body = start + block[start..].find('>')? + 1;
    let end = body + block[body..].find(&format!("</{name}>"))?;
    let text = block[body..end].trim();
    (!text.is_empty() && !text.contains('<')).then(|| unescape(text))
}

/// A point curve (`<crs:Name><rdf:Seq><rdf:li>x, y</rdf:li>...`), 0..255.
fn curve(xmp: &str, name: &str) -> Option<Vec<[f32; 2]>> {
    let open = format!("<crs:{name}>");
    let start = xmp.find(&open)?;
    let end = start + xmp[start..].find(&format!("</crs:{name}>"))?;
    let points: Vec<[f32; 2]> = xmp[start..end]
        .split("<rdf:li>")
        .skip(1)
        .filter_map(|li| {
            let (x, y) = li.split("</rdf:li>").next()?.split_once(',')?;
            Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
        })
        .collect();
    (points.len() >= 2).then_some(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a preset exported from Lightroom Classic 13.
    const PRESET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Cluster=""
   crs:UUID="5A1E6F2B3C4D4E5F8A9B0C1D2E3F4A5B"
   crs:SupportsAmount="True"
   crs:SupportsColor="True"
   crs:SupportsMonochrome="True"
   crs:SupportsHighDynamicRange="True"
   crs:SupportsNormalDynamicRange="True"
   crs:SupportsSceneReferred="True"
   crs:SupportsOutputReferred="True"
   crs:RequiresRGBTables="False"
   crs:CameraModelRestriction=""
   crs:Copyright=""
   crs:ContactInfo=""
   crs:Version="16.0"
   crs:ProcessVersion="15.4"
   crs:WhiteBalance="Custom"
   crs:Temperature="5600"
   crs:Tint="+8"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="-12"
   crs:Highlights2012="-40"
   crs:Shadows2012="+25"
   crs:Whites2012="+5"
   crs:Blacks2012="-8"
   crs:Vibrance="+10"
   crs:Saturation="-5"
   crs:HueAdjustmentOrange="-6"
   crs:SaturationAdjustmentOrange="-10"
   crs:LuminanceAdjustmentOrange="+8"
   crs:HueAdjustmentGreen="+20"
   crs:SplitToningShadowHue="210"
   crs:SplitToningShadowSaturation="12"
   crs:SplitToningHighlightHue="45"
   crs:SplitToningHighlightSaturation="9"
   crs:SplitToningBalance="+10"
   crs:ColorGradeMidtoneHue="30"
   crs:ColorGradeMidtoneSat="4"
   crs:ColorGradeShadowLum="0"
   crs:ColorGradeBlending="60"
   crs:ShadowTint="+3"
   crs:BlueSaturation="+15"
   crs:Sharpness="40"
   crs:LensProfileEnable="1"
   crs:ToneCurveName2012="Custom"
   crs:CameraProfile="Adobe Standard"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Wedding Warm</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">User Presets</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 18</rdf:li>
     <rdf:li>64, 60</rdf:li>
     <rdf:li>192, 198</rdf:li>
     <rdf:li>255, 250</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012Red>
   <crs:Look>
    <rdf:Description
     crs:Name="Adobe Color"
     crs:Amount="1"
     crs:UUID="B952C231111CD8E0ECCF14B86BAA7077"
     crs:SupportsAmount="false"
     crs:SupportsMonochrome="false"
     crs:SupportsOutputReferred="false">
     <crs:Group>
      <rdf:Alt>
       <rdf:li xml:lang="x-default">Profiles</rdf:li>
      </rdf:Alt>
     </crs:Group>
     <crs:Parameters>
      <rdf:Description
       crs:Version="16.0"
       crs:ProcessVersion="15.4"
       crs:ConvertToGrayscale="False"
       crs:LookTable="E1095149FDB39D7A057BAB208837E2E1"/>
     </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn reads_a_preset() {
        let s = parse(PRESET);
        assert_eq!(s.name.as_deref(), Some("Wedding Warm"));
        assert_eq!(s.sliders["exposure"], 0.35);
        assert_eq!(s.sliders["contrast"], -12.0);
        assert_eq!(s.sliders["highlights"], -40.0);
        assert_eq!(s.sliders["calShadowTint"], 3.0);
        assert_eq!(s.sliders["calBlueSat"], 15.0);
        assert_eq!(s.sliders["sharpen"], 40.0);
        assert_eq!(s.white_balance.as_deref(), Some("custom"));
        assert_eq!((s.temperature, s.tint), (Some(5600.0), Some(8.0)));
        assert_eq!(s.profile.as_deref(), Some("adobeColor"));
        assert_eq!(s.mixer["orange"]["hue"], -6.0);
        assert_eq!(s.mixer["orange"]["lum"], 8.0);
        assert_eq!(s.mixer["green"]["hue"], 20.0);
        assert!(!s.mixer.contains_key("red"));
        assert_eq!(s.curves["rgb"], vec![[0.0, 18.0], [64.0, 60.0], [192.0, 198.0], [255.0, 250.0]]);
        assert_eq!(s.curves["r"].len(), 2);
        assert_eq!(s.grade["shadows"]["hue"], 210.0);
        assert_eq!(s.grade["highs"]["sat"], 9.0);
        assert_eq!(s.grade["mids"]["hue"], 30.0);
        assert_eq!(s.grade[""]["blending"], 60.0);
        assert_eq!(s.grade[""]["balance"], 10.0);
        // The look's own crs:Name / ConvertToGrayscale don't leak into the settings.
        assert_eq!(s.skipped, vec!["Lens profile corrections".to_string()]);
    }

    #[test]
    fn any_luminance_noise_reduction_means_noise_50() {
        let s = parse(r#"<rdf:Description crs:LuminanceSmoothing="30" crs:Exposure2012="0"/>"#);
        assert_eq!(s.sliders["noise"], 50.0);
        let s = parse(r#"<rdf:Description crs:LuminanceSmoothing="0" crs:Exposure2012="0"/>"#);
        assert_eq!(s.sliders["noise"], 0.0);
    }

    #[test]
    fn as_shot_and_plain_adobe_standard() {
        let s = parse(r#"<rdf:Description crs:WhiteBalance="As Shot" crs:Temperature="5000" crs:Tint="0" crs:CameraProfile="Adobe Standard" crs:Exposure2012="0"/>"#);
        assert_eq!(s.white_balance.as_deref(), Some("asShot"));
        assert_eq!(s.temperature, None);
        assert_eq!(s.profile.as_deref(), Some("adobeStandard"));
        assert_eq!(s.sliders["exposure"], 0.0);
    }

    #[test]
    fn unsupported_profile_and_masks_are_reported() {
        let s = parse(r#"<rdf:Description crs:CameraProfile="Camera Classic Chrome" crs:ConvertToGrayscale="True"><crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:LocalExposure2012="0.3"/></rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections></rdf:Description>"#);
        assert_eq!(s.profile, None);
        assert!(s.skipped.contains(&"Profile \"Camera Classic Chrome\"".to_string()));
        assert!(s.skipped.contains(&"Black & white".to_string()));
        assert!(s.skipped.contains(&"Masks and local adjustments".to_string()));
    }

    #[test]
    fn the_photos_own_values_win_over_nested_copies() {
        // As in real sidecars: the photo's settings first, then masks that repeat keys.
        let s = parse(
            r#"<rdf:Description crs:Exposure2012="+1.05" crs:ParametricDarks="-19" crs:CameraProfile="Adobe Standard v2" crs:PostCropVignetteAmount="0" crs:PostCropVignetteMidpoint="20">
               <crs:MaskGroupBasedCorrections><rdf:Seq/></crs:MaskGroupBasedCorrections>
               <rdf:Description crs:Exposure2012="0" crs:ParametricDarks="0"/></rdf:Description>"#,
        );
        assert_eq!(s.sliders["exposure"], 1.05);
        assert_eq!(s.profile.as_deref(), Some("adobeStandard"));
        // The parametric curve is read; the empty mask list and the unused vignette shape aren't reported.
        assert_eq!(s.sliders["paramDarks"], -19.0);
        assert!(s.skipped.is_empty());
    }

    #[test]
    fn a_rating_only_sidecar_has_no_settings() {
        assert!(parse(r#"<rdf:Description xmp:Rating="3" rapidculling:Pick="true"/>"#).is_empty());
    }
}


#[cfg(test)]
mod apply_tests {
    use super::*;
    use crate::pipeline::{Profile, RenderSettings};

    #[test]
    fn applies_only_what_the_settings_set() {
        let lr = parse(
            r#"<rdf:Description crs:WhiteBalance="Custom" crs:Temperature="5600" crs:Tint="+8" crs:Exposure2012="+0.35"
               crs:HueAdjustmentOrange="-6" crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="12" crs:ColorGradeBlending="60"
               crs:CameraProfile="Adobe Standard"><crs:Look><rdf:Description crs:Name="Adobe Color"/></crs:Look>
               <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 18</rdf:li><rdf:li>255, 250</rdf:li></rdf:Seq></crs:ToneCurvePV2012></rdf:Description>"#,
        );
        let mut s = RenderSettings { contrast: 20.0, ..Default::default() };
        lr.apply_to(&mut s, false);
        assert_eq!(s.exposure, 0.35);
        assert_eq!(s.contrast, 20.0, "not in the preset, so untouched");
        assert_eq!((s.temperature, s.tint), (Some(5600.0), Some(8.0)));
        assert_eq!(s.profile, Profile::ShinobiColour, "Adobe Color falls back without Adobe's profiles");
        assert_eq!(s.tools.mixer[1][0], -6.0);
        assert_eq!(s.tools.mixer[0], [0.0; 3]);
        assert_eq!((s.tools.grade.shadows.hue, s.tools.grade.shadows.sat, s.tools.grade.blending), (210.0, 12.0, 60.0));
        assert_eq!(s.tools.curve_rgb, vec![[0.0, 18.0], [255.0, 250.0]]);
        lr.apply_to(&mut s, true);
        assert_eq!(s.profile, Profile::AdobeColor);
    }
}

#[cfg(test)]
mod crop_import_tests {
    use super::*;

    fn lr(l: f32, t: f32, r: f32, b: f32, angle: f32, o: Option<u16>) -> LightroomCrop {
        LightroomCrop { left: l, top: t, right: r, bottom: b, angle, orientation: o }
    }

    /// Where RapidOne's crop shows output corner (fx, fy) (fractions of the
    /// crop) on the canvas-turned photo, in pixels (crop.rs).
    fn source(c: &ImportedCrop, w: f32, h: f32, fx: f32, fy: f32) -> (f32, f32) {
        let x = (c.x0 + (c.x1 - c.x0) * fx) * w - w / 2.0;
        let y = (c.y0 + (c.y1 - c.y0) * fy) * h - h / 2.0;
        let (s, co) = (-c.angle).to_radians().sin_cos();
        (x * co - y * s + w / 2.0, x * s + y * co + h / 2.0)
    }

    #[test]
    fn a_plain_crop_is_the_same_rectangle() {
        let c = lr(0.1, 0.2, 0.8, 0.9, 0.0, None).to_rapidone(6000.0, 4000.0, 1).unwrap();
        assert_eq!(c.quarter, 0);
        assert!((c.x0 - 0.1).abs() < 1e-5 && (c.y0 - 0.2).abs() < 1e-5 && (c.x1 - 0.8).abs() < 1e-5 && (c.y1 - 0.9).abs() < 1e-5);
    }

    #[test]
    fn an_angled_crop_has_lightrooms_corners() {
        // Small enough to fit, so (Left, Top) and (Right, Bottom) are the corners.
        let (w, h) = (6000.0, 4000.0);
        let c = lr(0.3, 0.3, 0.7, 0.7, 5.0, None).to_rapidone(w, h, 1).unwrap();
        let (ax, ay) = source(&c, w, h, 0.0, 0.0);
        let (bx, by) = source(&c, w, h, 1.0, 1.0);
        assert!((ax - 0.3 * w).abs() < 0.5 && (ay - 0.3 * h).abs() < 0.5, "{ax} {ay}");
        assert!((bx - 0.7 * w).abs() < 0.5 && (by - 0.7 * h).abs() < 0.5, "{bx} {by}");
    }

    #[test]
    fn a_turned_photo_is_cropped_in_the_sensor_frame() {
        // XMP turns a landscape sensor picture clockwise (6); RapidOne shows it unturned.
        let c = lr(0.1, 0.2, 0.8, 0.9, 0.0, Some(6)).to_rapidone(6000.0, 4000.0, 1).unwrap();
        assert_eq!(c.quarter, 1);
        // Turned clockwise, the sensor's bottom edge becomes the left.
        assert!((c.x0 - 0.1).abs() < 1e-5 && (c.x1 - 0.8).abs() < 1e-5, "{c:?}");
        assert!((c.y0 - 0.1).abs() < 1e-5 && (c.y1 - 0.8).abs() < 1e-5, "{c:?}");
    }
}

#[cfg(test)]
mod gradient_import_tests {
    use super::*;
    use crate::gradient::Kind;

    const KIT: &str = r#"<rdf:Description rdf:about="" crs:HasSettings="True">
   <crs:CircularGradientBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1" crs:CorrectionActive="true" crs:LocalExposure2012="0.25" crs:LocalSaturation="-0.5">
      <crs:CorrectionMasks><rdf:Seq>
        <rdf:li crs:What="Mask/CircularGradient" crs:MaskValue="1" crs:Top="0.2" crs:Left="0.35" crs:Bottom="0.8" crs:Right="0.65" crs:Angle="30" crs:Feather="50" crs:Flipped="false" crs:Version="2"/>
      </rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li></rdf:Seq></crs:CircularGradientBasedCorrections>
   <crs:GradientBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1" crs:CorrectionActive="true" crs:LocalExposure2012="-0.25">
      <crs:CorrectionMasks><rdf:Seq>
        <rdf:li crs:What="Mask/Gradient" crs:MaskValue="1" crs:FullX="0.5" crs:FullY="0" crs:ZeroX="0.5" crs:ZeroY="0.5"/>
      </rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li></rdf:Seq></crs:GradientBasedCorrections>
  </rdf:Description>"#;

    #[test]
    fn linear_and_radial_are_read() {
        let s = parse(KIT);
        assert!(!s.skipped.iter().any(|k| k.starts_with("Masks")), "{:?}", s.skipped);
        let g = turn_gradients(&s.lightroom_gradients, s.orientation, 1, 1.5).unwrap();
        assert_eq!(g.len(), 2);
        let lin = g.iter().find(|g| g.kind == Kind::Linear).unwrap();
        assert_eq!((lin.full, lin.zero), ([0.5, 0.0], [0.5, 0.5]));
        assert!((lin.exposure + 1.0).abs() < 1e-6);
        let rad = g.iter().find(|g| g.kind == Kind::Radial).unwrap();
        assert!((rad.exposure - 1.0).abs() < 1e-6 && (rad.saturation + 50.0).abs() < 1e-4);
        assert!(rad.invert, "Lightroom's radial acts outside unless Flipped");
        // The 30 degree oval, turned as Lightroom turns it (0..1 coordinates).
        assert!((rad.angle - 31.0).abs() < 2.0 || (rad.angle + 59.0).abs() < 2.0, "{:?}", (rad.rx, rad.ry, rad.angle));
    }

    #[test]
    fn a_portrait_camera_turns_them_like_the_crop() {
        // Camera orientation 8 (turned anticlockwise to show): the sensor's top
        // edge is the left edge of the photo as shown (gradient kit, 17 Oct).
        let s = parse(KIT);
        let g = turn_gradients(&s.lightroom_gradients, s.orientation, 8, 1.5).unwrap();
        let lin = g.iter().find(|g| g.kind == Kind::Linear).unwrap();
        assert!((lin.full[0] - 0.0).abs() < 1e-6 && (lin.zero[0] - 0.5).abs() < 1e-6, "{:?}", (lin.full, lin.zero));
    }
}
