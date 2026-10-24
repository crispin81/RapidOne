//! Rendering with a DNG camera profile (and optionally a look profile), as
//! the public DNG specification describes:
//!
//!   white-balanced camera RGB
//!     -> ForwardMatrix (blended for the white balance's colour temperature)
//!     -> XYZ (D50) -> linear ProPhoto RGB
//!     -> HueSatMap (the profile's hue/saturation/value corrections)
//!     -> exposure
//!     -> LookTable (the profile's, or a look profile's)
//!     -> tone curve, applied so hues hold (curve on the largest and smallest
//!        channel, the middle one placed proportionally)
//!     -> display (sRGB)
//!
//! This is how "Adobe Standard" and "Adobe Color" can render exactly as in
//! Lightroom, from profile files already on the user's machine.

use crate::dcp::{HsvTable, Look, Mat3, Profile};
use crate::pipeline::{mul3, SceneImage, REC2020_TO_XYZ};
use crate::whitebalance::{invert3, WhiteBalance};
use image::RgbImage;
use rayon::prelude::*;

/// Linear ProPhoto RGB (ROMM, D50) -> XYZ (D50).
pub const PROPHOTO_TO_XYZ_D50: Mat3 = [
    [0.797_674_9, 0.135_191_7, 0.031_353_4],
    [0.288_040_2, 0.711_874_1, 0.000_085_7],
    [0.0, 0.0, 0.825_210_0],
];

/// XYZ (D50) -> linear sRGB (D65), with Bradford adaptation.
const XYZ_D50_TO_SRGB: Mat3 = [
    [3.133_856_1, -1.616_866_7, -0.490_614_6],
    [-0.978_768_4, 1.916_141_5, 0.033_454_0],
    [0.071_945_3, -0.228_991_1, 1.405_242_7],
];

/// XYZ (D65) -> XYZ (D50), Bradford.
pub const XYZ_D65_TO_D50: Mat3 = [
    [1.047_811_2, 0.022_886_6, -0.050_127_0],
    [0.029_542_4, 0.990_484_4, -0.017_049_1],
    [-0.009_234_5, 0.015_043_6, 0.752_131_6],
];

fn apply3(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// Colour temperature of an EXIF LightSource code, for blending a
/// profile's two calibrations.
fn illuminant_kelvin(code: u16) -> f32 {
    match code {
        17 | 3 => 2856.0, // Standard light A, tungsten
        24 => 3200.0,     // ISO studio tungsten
        15 => 3450.0,     // white fluorescent
        2 | 14 => 4150.0, // (cool white) fluorescent
        18 => 4874.0,     // Standard light B
        23 | 13 => 5003.0,
        1 | 4 | 9 => 5500.0,
        20 => 5503.0,
        12 => 6430.0,
        10 | 21 => 6504.0,
        19 => 6774.0,
        11 | 22 => 7504.0,
        _ => 5000.0,
    }
}

/// Weight of the profile's first calibration at `kelvin` (DNG: linear in
/// inverse temperature, clamped to the two calibrations).
fn weight1(p: &Profile, kelvin: f32) -> f32 {
    let (k1, k2) = (illuminant_kelvin(p.illuminant1), illuminant_kelvin(p.illuminant2));
    if (k1 - k2).abs() < 1.0 {
        return 1.0;
    }
    ((1.0 / kelvin - 1.0 / k2) / (1.0 / k1 - 1.0 / k2)).clamp(0.0, 1.0)
}

/// The light (kelvin, duv) whose white reads as `neutral` on the sensor,
/// through the profile's own colour matrices. Needs the usual tungsten (A)
/// and D65 pair.
pub fn profile_white(p: &Profile, neutral: [f32; 3], calibration: [f32; 3]) -> Option<(f32, f32)> {
    let (a, d65) = match (p.illuminant1, p.illuminant2) {
        (17, 21) => (p.color_matrix1?, p.color_matrix2?),
        (21, 17) => (p.color_matrix2?, p.color_matrix1?),
        _ => return None,
    };
    // DNG: the CameraCalibration scales the matrices' camera rows.
    let cal = |m: Mat3| -> Mat3 { [0, 1, 2].map(|i| m[i].map(|v| v * calibration[i])) };
    let (a, d65) = (cal(a), cal(d65));
    Some(crate::whitebalance::CameraColour { a, d65 }.white_for_neutral(neutral))
}

fn blend_mat(a: Option<Mat3>, b: Option<Mat3>, w: f32) -> Option<Mat3> {
    match (a, b) {
        (Some(a), Some(b)) => Some(std::array::from_fn(|i| std::array::from_fn(|j| w * a[i][j] + (1.0 - w) * b[i][j]))),
        (a, b) => a.or(b),
    }
}

fn blend_table(a: &Option<HsvTable>, b: &Option<HsvTable>, w: f32) -> Option<HsvTable> {
    match (a, b) {
        (Some(a), Some(b)) if a.data.len() == b.data.len() => Some(HsvTable {
            data: a.data.iter().zip(&b.data).map(|(x, y)| std::array::from_fn(|i| w * x[i] + (1.0 - w) * y[i])).collect(),
            ..a.clone()
        }),
        (a, b) => a.clone().or_else(|| b.clone()),
    }
}

fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// RGB -> (hue 0..6, saturation 0..1, value), as the DNG specification's
/// hue/saturation map uses it.
fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if max <= 0.0 || d <= 0.0 {
        return [0.0, 0.0, max];
    }
    let h = if max == r {
        (g - b) / d
    } else if max == g {
        2.0 + (b - r) / d
    } else {
        4.0 + (r - g) / d
    };
    [h.rem_euclid(6.0), d / max, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    if s <= 0.0 {
        return [v, v, v];
    }
    let h = h.rem_euclid(6.0);
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Applies a hue/saturation/value table: looks up (hue shift, sat scale,
/// val scale) by trilinear interpolation (hue wraps round), then adjusts.
fn apply_table(t: &HsvTable, rgb: [f32; 3]) -> [f32; 3] {
    let [h, s, v] = rgb_to_hsv(rgb);
    let hf = h * t.hue_divs as f32 / 6.0;
    let sf = s * (t.sat_divs - 1) as f32;
    let vin = if t.srgb_gamma { srgb_encode(v) } else { v.clamp(0.0, 1.0) };
    let vf = vin * (t.val_divs.max(1) - 1) as f32;
    let (h0, s0, v0) = (hf.floor(), sf.floor().min((t.sat_divs - 1) as f32), vf.floor().min((t.val_divs.max(1) - 1) as f32));
    let (ht, st, vt) = (hf - h0, sf - s0, vf - v0);
    let (h0, s0, v0) = (h0 as usize % t.hue_divs, s0 as usize, v0 as usize);
    let h1 = (h0 + 1) % t.hue_divs;
    let s1 = (s0 + 1).min(t.sat_divs - 1);
    let v1 = (v0 + 1).min(t.val_divs.max(1) - 1);
    let at = |vi: usize, hi: usize, si: usize| t.data[(vi * t.hue_divs + hi) * t.sat_divs + si];
    let lerp = |a: [f32; 3], b: [f32; 3], w: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * w) };
    let plane = |vi: usize| {
        let a = lerp(at(vi, h0, s0), at(vi, h0, s1), st);
        let b = lerp(at(vi, h1, s0), at(vi, h1, s1), st);
        lerp(a, b, ht)
    };
    let [dh, ds, dv] = lerp(plane(v0), plane(v1), vt);
    hsv_to_rgb([h + dh * 6.0 / 360.0, (s * ds).min(1.0), v * dv])
}

/// A tone curve through (x, y) points on 0..1, as a lookup table.
pub struct Curve {
    lut: Vec<f32>,
}

impl Curve {
    const SIZE: usize = 4096;

    /// Piecewise-linear through dense points (a .dcp's 64-point curve).
    pub fn linear(points: &[(f32, f32)]) -> Curve {
        Curve::from_fn(|x| {
            let i = points.partition_point(|p| p.0 < x);
            if i == 0 {
                return points.first().map_or(x, |p| p.1);
            }
            if i >= points.len() {
                return points.last().map_or(x, |p| p.1);
            }
            let (a, b) = (points[i - 1], points[i]);
            a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0).max(1e-9)
        })
    }

    /// Smooth (monotone cubic) through sparse control points, like a
    /// Lightroom point curve.
    pub fn smooth(points: &[(f32, f32)]) -> Curve {
        let n = points.len();
        if n < 2 {
            return Curve::from_fn(|x| x);
        }
        let d: Vec<f32> = points.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0).max(1e-9)).collect();
        // Fritsch-Carlson tangents.
        let mut m = vec![0.0f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
        }
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
        Curve::from_fn(|x| {
            let i = points.partition_point(|p| p.0 <= x).clamp(1, n - 1) - 1;
            let (p0, p1) = (points[i], points[i + 1]);
            let h = (p1.0 - p0.0).max(1e-9);
            let t = ((x - p0.0) / h).clamp(0.0, 1.0);
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * p0.1 + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * p1.1 + (t3 - t2) * h * m[i + 1]
        })
    }

    /// A natural cubic spline through the points (zero curvature at the
    /// ends), blended with the monotone one by `amount` (0..1): bends more
    /// strongly through each point. Kept within 0..1.
    pub fn natural(points: &[(f32, f32)], amount: f32) -> Curve {
        let mono = Curve::smooth(points);
        let n = points.len();
        if n < 3 || amount <= 0.0 {
            return mono;
        }
        // Solve for the second derivatives (tridiagonal system).
        let h: Vec<f32> = points.windows(2).map(|w| (w[1].0 - w[0].0).max(1e-9)).collect();
        let mut a = vec![0.0f32; n];
        let mut b = vec![1.0f32; n];
        let mut c = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        for i in 1..n - 1 {
            a[i] = h[i - 1];
            b[i] = 2.0 * (h[i - 1] + h[i]);
            c[i] = h[i];
            r[i] = 6.0 * ((points[i + 1].1 - points[i].1) / h[i] - (points[i].1 - points[i - 1].1) / h[i - 1]);
        }
        for i in 1..n {
            let m = a[i] / b[i - 1];
            b[i] -= m * c[i - 1];
            r[i] -= m * r[i - 1];
        }
        let mut z = vec![0.0f32; n];
        z[n - 1] = r[n - 1] / b[n - 1];
        for i in (0..n - 1).rev() {
            z[i] = (r[i] - c[i] * z[i + 1]) / b[i];
        }
        Curve::from_fn(|x| {
            let i = points.partition_point(|p| p.0 <= x).clamp(1, n - 1) - 1;
            let (p0, p1, hi) = (points[i], points[i + 1], h[i]);
            let t = (x - p0.0).clamp(0.0, hi);
            let u = hi - t;
            let s = z[i] * u * u * u / (6.0 * hi) + z[i + 1] * t * t * t / (6.0 * hi)
                + (p0.1 / hi - z[i] * hi / 6.0) * u
                + (p1.1 / hi - z[i + 1] * hi / 6.0) * t;
            let m = mono.eval(x);
            (m + (s - m) * amount).clamp(0.0, 1.0)
        })
    }

    pub fn from_fn(f: impl Fn(f32) -> f32) -> Curve {
        Curve { lut: (0..Self::SIZE).map(|i| f(i as f32 / (Self::SIZE - 1) as f32)).collect() }
    }

    pub fn eval(&self, x: f32) -> f32 {
        let f = x.clamp(0.0, 1.0) * (Self::SIZE - 1) as f32;
        let i = (f as usize).min(Self::SIZE - 2);
        let t = f - i as f32;
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t
    }

    /// Applies the curve so hue holds: to the largest and smallest channel,
    /// with the middle one kept at the same proportion between them.
    pub fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        let c = c.map(|v| v.clamp(0.0, 1.0));
        let (mut hi, mut lo) = (0, 0);
        for i in 1..3 {
            if c[i] > c[hi] {
                hi = i;
            }
            if c[i] < c[lo] {
                lo = i;
            }
        }
        if hi == lo {
            return c.map(|v| self.eval(v));
        }
        let mid = 3 - hi - lo;
        let (nh, nl) = (self.eval(c[hi]), self.eval(c[lo]));
        let mut out = [0.0; 3];
        out[hi] = nh;
        out[lo] = nl;
        out[mid] = nl + (nh - nl) * (c[mid] - c[lo]) / (c[hi] - c[lo]);
        out
    }
}

/// How to combine a look with its base profile, while we establish which
/// way Lightroom does it.
#[derive(Clone, Copy, Debug)]
pub struct LookOptions {
    /// Use the base profile's own LookTable as well as the look's.
    pub profile_look_table: bool,
    /// Apply the look's point curve (in sRGB-gamma space) after the profile's curve.
    pub look_curve: bool,
}

/// Everything needed to render a photo with a profile, prepared once.
pub struct ProfileRender {
    /// Scene (linear Rec.2020, as-shot balanced) -> linear ProPhoto.
    to_prophoto: Mat3,
    hue_sat_map: Option<HsvTable>,
    look_tables: Vec<HsvTable>,
    tone: Curve,
    look_curve: Option<Curve>,
}

impl ProfileRender {
    /// For a photo with white balance `wb`, rendered at `kelvin`/`tint`.
    /// `default_curve` is used when the profile has no tone curve of its own:
    /// Adobe's newer profiles leave it out and rely on the converter's default
    /// curve, which older installed profiles carry in full.
    pub fn new(
        profile: &Profile,
        look: Option<&Look>,
        wb: &WhiteBalance,
        kelvin: f32,
        tint: f32,
        opts: LookOptions,
        default_curve: &[(f32, f32)],
    ) -> ProfileRender {
        // Back from the scene to white-balanced camera RGB: undo the scene's
        // as-shot conversion (it was built from balanced camera RGB).
        let to_camera = invert3(&wb.camera_to_rec2020(wb.shot_kelvin, wb.shot_tint));
        // Re-balance for the chosen light: raw = balanced x neutral_shot,
        // then balance by the neutral for the new light.
        let neutral_new = wb.camera.neutral_for(kelvin, tint / crate::whitebalance::TINT_PER_DUV);
        // The profile's calibrations are blended by the light's temperature
        // as the profile's own colour matrices see it (the decoder's matrices
        // can disagree: D810 sunshine read as 3000 K through them).
        let light_kelvin = profile_white(profile, neutral_new, wb.calibration).map_or(kelvin, |(k, _)| k);
        let w = weight1(profile, light_kelvin);
        let rebalance: Mat3 = std::array::from_fn(|i| {
            std::array::from_fn(|j| if i == j { wb.neutral_shot[i] / neutral_new[i] } else { 0.0 })
        });
        let to_xyz_d50 = match blend_mat(profile.forward_matrix1, profile.forward_matrix2, w) {
            Some(forward) => mul3(&forward, &mul3(&rebalance, &to_camera)),
            // No ForwardMatrix: use our own white-balanced scene, adapted to D50.
            None => mul3(&XYZ_D65_TO_D50, &mul3(&REC2020_TO_XYZ, &wb.rebalance(kelvin, tint))),
        };
        let to_prophoto = mul3(&invert3(&PROPHOTO_TO_XYZ_D50), &to_xyz_d50);

        let mut look_tables = Vec::new();
        if look.is_none() || opts.profile_look_table {
            look_tables.extend(profile.look_table.clone());
        }
        if let Some(t) = look.and_then(|l| l.look_table.clone()) {
            look_tables.push(t);
        }
        let tone = if profile.tone_curve.len() >= 2 {
            Curve::linear(&profile.tone_curve)
        } else if default_curve.len() >= 2 {
            Curve::linear(default_curve)
        } else {
            Curve::from_fn(|x| x)
        };
        let look_curve = look
            .filter(|l| opts.look_curve && l.tone_curve.len() >= 2)
            .map(|l| Curve::smooth(&l.tone_curve.iter().map(|&(x, y)| (x / 255.0, y / 255.0)).collect::<Vec<_>>()));
        ProfileRender {
            to_prophoto,
            hue_sat_map: blend_table(&profile.hue_sat_map1, &profile.hue_sat_map2, w),
            look_tables,
            tone,
            look_curve,
        }
    }

    /// The profile's first step, before exposure: a scene pixel (linear
    /// Rec.2020, as-shot balanced) to linear ProPhoto through the profile's
    /// matrices and HueSatMap.
    pub fn working(&self, p: [f32; 3]) -> [f32; 3] {
        let c = apply3(&self.to_prophoto, p).map(|v| v.max(0.0));
        match &self.hue_sat_map {
            Some(t) => apply_table(t, c),
            None => c,
        }
    }

    /// The profile's last step, after exposure (and any tone sliders): the
    /// LookTable(s) and tone curve, to display-linear ProPhoto.
    pub fn finish(&self, c: [f32; 3]) -> [f32; 3] {
        self.finish_curves(self.finish_look(c))
    }

    /// The look tables alone (the first part of `finish`).
    pub fn finish_look(&self, c: [f32; 3]) -> [f32; 3] {
        let mut c = c.map(|v| v.max(0.0));
        for t in &self.look_tables {
            c = apply_table(t, c);
        }
        c
    }

    /// The profile and look tone curves (the rest of `finish`).
    pub fn finish_curves(&self, c: [f32; 3]) -> [f32; 3] {
        // (Tried: the look's curve replacing the profile's, or either curve
        // applied to brightness only - all further from Lightroom.)
        let mut c = self.tone.apply_rgb(c.map(|v| v.max(0.0)));
        if let Some(lc) = &self.look_curve {
            c = lc.apply_rgb(c.map(srgb_encode)).map(srgb_decode);
        }
        c
    }

    /// Renders a scene at `exposure` stops (total, including any baseline).
    /// `black` (scene-linear, after exposure) is subtracted with the range
    /// rescaled to keep white at white: a black point.
    pub fn render(&self, scene: &SceneImage, exposure: f32, black: f32) -> RgbImage {
        let gain = 2f32.powf(exposure);
        let black_scale = 1.0 / (1.0 - black).max(1e-3);
        let to_srgb = mul3(&XYZ_D50_TO_SRGB, &PROPHOTO_TO_XYZ_D50);
        let bytes: Vec<u8> = scene
            .px
            .par_iter()
            .flat_map_iter(|&p| {
                let c = self.working(p).map(|v| ((v * gain - black) * black_scale).max(0.0));
                apply3(&to_srgb, self.finish(c)).map(|v| (srgb_encode(v) * 255.0 + 0.5) as u8)
            })
            .collect();
        RgbImage::from_raw(scene.width as u32, scene.height as u32, bytes).expect("buffer size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        for c in [[0.2f32, 0.5, 0.9], [0.9, 0.1, 0.3], [0.4, 0.4, 0.4], [0.0, 0.7, 0.2]] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn identity_table_changes_nothing() {
        let t = HsvTable { hue_divs: 6, sat_divs: 3, val_divs: 2, data: vec![[0.0, 1.0, 1.0]; 36], srgb_gamma: false };
        let c = [0.3f32, 0.6, 0.2];
        let out = apply_table(&t, c);
        for i in 0..3 {
            assert!((out[i] - c[i]).abs() < 1e-5, "{out:?}");
        }
    }

    #[test]
    fn rgb_tone_keeps_hue_order_and_greys() {
        let curve = Curve::smooth(&[(0.0, 0.0), (0.5, 0.7), (1.0, 1.0)]);
        let grey = curve.apply_rgb([0.5, 0.5, 0.5]);
        assert!((grey[0] - 0.7).abs() < 1e-3 && grey[0] == grey[1] && grey[1] == grey[2], "{grey:?}");
        let c = curve.apply_rgb([0.6, 0.3, 0.1]);
        assert!(c[0] > c[1] && c[1] > c[2], "{c:?}");
    }
}
