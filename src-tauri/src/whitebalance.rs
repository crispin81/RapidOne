//! White balance: Temperature (Kelvin) and Tint, with the same numbers
//! Lightroom shows for RAWs.
//!
//! The camera's as-shot setting is turned into Kelvin and Tint the way the DNG
//! specification describes: find the light whose white reads as the
//! camera's white-balance neutral, using the camera's two colour matrices
//! (tungsten and daylight) blended for that light. Any other Kelvin/Tint is
//! rendered by re-running that chain for the new light, and Bradford-adapting
//! its white to D65 - one 3x3 matrix applied to the cached scene image.

use crate::pipeline::{mul3, REC2020_TO_XYZ};

const BRADFORD: [[f32; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

pub fn invert3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let inv = 1.0 / det;
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ]
}

/// Chromaticity (x, y) of a black body at `kelvin` (Kim et al. 2002 cubic
/// fit, 1667-25000 K).
fn planckian_xy(kelvin: f32) -> (f32, f32) {
    let t = kelvin.clamp(1667.0, 25000.0) as f64;
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    (x as f32, y as f32)
}

fn xy_to_uv((x, y): (f32, f32)) -> (f32, f32) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy((u, v): (f32, f32)) -> (f32, f32) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

/// A light's chromaticity: `kelvin` on the locus, moved `duv` off it
/// (positive = above the locus = greener).
fn light_xy(kelvin: f32, duv: f32) -> (f32, f32) {
    let (u, v) = xy_to_uv(planckian_xy(kelvin));
    if duv == 0.0 {
        return uv_to_xy((u, v));
    }
    // Direction along the locus, from a small step in temperature, then the
    // perpendicular that points towards larger v (green).
    let (u2, v2) = xy_to_uv(planckian_xy(kelvin * 1.01));
    let (du, dv) = (u2 - u, v2 - v);
    let len = (du * du + dv * dv).sqrt();
    let (mut nu, mut nv) = (-dv / len, du / len);
    if nv < 0.0 {
        nu = -nu;
        nv = -nv;
    }
    uv_to_xy((u + nu * duv, v + nv * duv))
}

fn xy_to_xyz((x, y): (f32, f32)) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Bradford adaptation matrix (XYZ -> XYZ) from one white to another.
fn bradford(src: [f32; 3], dst: [f32; 3]) -> [[f32; 3]; 3] {
    let lms = |v: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| BRADFORD[i][0] * v[0] + BRADFORD[i][1] * v[1] + BRADFORD[i][2] * v[2])
    };
    let (s, d) = (lms(src), lms(dst));
    let scale = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul3(&invert3(&BRADFORD), &mul3(&scale, &BRADFORD))
}

// ---- Absolute white balance (Kelvin + Tint), the DNG-specification way ----

/// Lightroom's tint units, per Duv (CIE 1960 uv distance of the *light* from the
/// Planckian locus, positive = greener light). Lightroom's tint describes the
/// correction: a greenish light needs magenta added, so it reads positive.
/// Checked against Lightroom's As Shot values (Panasonic S5II: 4400 K +23,
/// ours 4420 K +23).
pub const TINT_PER_DUV: f32 = 3000.0;

/// Kelvin of the DNG standard illuminants the two camera matrices are measured under.
const KELVIN_A: f32 = 2856.0;
const KELVIN_D65: f32 = 6504.0;

/// Nearest point on the Planckian locus to a chromaticity: (kelvin, duv),
/// duv positive above the locus (greener).
pub fn xy_to_kelvin_duv(xy: (f32, f32)) -> (f32, f32) {
    let (u, v) = xy_to_uv(xy);
    let dist = |mired: f32| {
        let (lu, lv) = xy_to_uv(planckian_xy(1e6 / mired));
        (lu - u).powi(2) + (lv - v).powi(2)
    };
    // Coarse scan in mireds (perceptually even), then golden-section refine.
    let mut best = 40.0f32;
    let mut m = 40.0f32;
    while m <= 600.0 {
        if dist(m) < dist(best) {
            best = m;
        }
        m += 2.0;
    }
    let (mut lo, mut hi) = ((best - 2.0).max(40.0), (best + 2.0).min(600.0));
    for _ in 0..40 {
        let a = lo + (hi - lo) * 0.382;
        let b = lo + (hi - lo) * 0.618;
        if dist(a) < dist(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let kelvin = 1e6 / ((lo + hi) / 2.0);
    let (lu, lv) = xy_to_uv(planckian_xy(kelvin));
    let d = ((u - lu).powi(2) + (v - lv).powi(2)).sqrt();
    (kelvin, if v >= lv { d } else { -d })
}

/// A camera's two colour matrices (XYZ -> camera), as in the DNG spec.
#[derive(Clone, Copy, Debug)]
pub struct CameraColour {
    pub a: [[f32; 3]; 3],
    pub d65: [[f32; 3]; 3],
}

impl CameraColour {
    /// The XYZ -> camera matrix for light of the given colour temperature:
    /// blended between the two calibrations by inverse temperature, as the
    /// DNG specification describes.
    pub fn matrix_at(&self, kelvin: f32) -> [[f32; 3]; 3] {
        let g = ((1.0 / kelvin - 1.0 / KELVIN_D65) / (1.0 / KELVIN_A - 1.0 / KELVIN_D65)).clamp(0.0, 1.0);
        std::array::from_fn(|i| std::array::from_fn(|j| g * self.a[i][j] + (1.0 - g) * self.d65[i][j]))
    }

    /// How a white object under this light reads on the sensor (camera RGB,
    /// green = 1). White balance multipliers are its reciprocals.
    pub fn neutral_for(&self, kelvin: f32, duv: f32) -> [f32; 3] {
        let xyz = xy_to_xyz(light_xy(kelvin, duv));
        let m = self.matrix_at(kelvin);
        let n: [f32; 3] = std::array::from_fn(|i| m[i][0] * xyz[0] + m[i][1] * xyz[1] + m[i][2] * xyz[2]);
        [n[0] / n[1], 1.0, n[2] / n[1]]
    }

    /// The light (kelvin, duv) whose white reads as `neutral` on the sensor -
    /// i.e. what the camera's white balance setting means in Kelvin. The
    /// matrix depends on the answer, so it's found by iteration (DNG spec).
    pub fn white_for_neutral(&self, neutral: [f32; 3]) -> (f32, f32) {
        let mut xy = (0.3457, 0.3585); // start at D50
        let mut kd = xy_to_kelvin_duv(xy);
        for _ in 0..50 {
            let inv = invert3(&self.matrix_at(kd.0));
            let xyz: [f32; 3] = std::array::from_fn(|i| inv[i][0] * neutral[0] + inv[i][1] * neutral[1] + inv[i][2] * neutral[2]);
            let sum = xyz[0] + xyz[1] + xyz[2];
            let next = (xyz[0] / sum, xyz[1] / sum);
            let done = (next.0 - xy.0).abs() < 1e-7 && (next.1 - xy.1).abs() < 1e-7;
            xy = next;
            kd = xy_to_kelvin_duv(xy);
            if done {
                break;
            }
        }
        kd
    }
}

/// A photo's white balance facts: its camera's colour calibration and what
/// the as-shot setting means, so any Kelvin/Tint can be rendered from the
/// cached scene image.
#[derive(Clone, Copy, Debug)]
pub struct WhiteBalance {
    pub camera: CameraColour,
    /// Camera RGB of a white object as shot (green = 1); the scene image was
    /// white balanced by its reciprocals.
    pub neutral_shot: [f32; 3],
    pub shot_kelvin: f32,
    /// In Lightroom tint units.
    pub shot_tint: f32,
    /// Adobe CameraCalibration (red, green, blue) already applied to
    /// `camera`; profiles' own matrices need it too.
    pub calibration: [f32; 3],
}

impl WhiteBalance {
    pub fn new(camera: CameraColour, neutral_shot: [f32; 3]) -> Self {
        let (shot_kelvin, duv) = camera.white_for_neutral(neutral_shot);
        Self { camera, neutral_shot, shot_kelvin, shot_tint: duv * TINT_PER_DUV, calibration: [1.0; 3] }
    }

    /// Camera RGB (balanced with the as-shot multipliers) -> linear Rec.2020,
    /// assuming the light was `kelvin`/`tint`: back to raw sensor values, into
    /// XYZ with the matrix for that light, then Bradford-adapted from that
    /// light's white to D65 (Rec.2020's white). Scaled so a white object
    /// under the assumed light comes out with luminance 1, so changing white
    /// balance doesn't change brightness.
    pub fn camera_to_rec2020(&self, kelvin: f32, tint: f32) -> [[f32; 3]; 3] {
        let duv = tint / TINT_PER_DUV;
        let white = xy_to_xyz(light_xy(kelvin, duv));
        let d65 = xy_to_xyz((0.3127, 0.3290));
        let to_xyz = invert3(&self.camera.matrix_at(kelvin));
        let unbalance = [[self.neutral_shot[0], 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, self.neutral_shot[2]]];
        let m = mul3(
            &invert3(&REC2020_TO_XYZ),
            &mul3(&bradford(white, d65), &mul3(&to_xyz, &unbalance)),
        );
        // Luminance of the assumed light's white, in this pipeline.
        let n_new = self.camera.neutral_for(kelvin, duv);
        let probe: [f32; 3] = std::array::from_fn(|i| n_new[i] / self.neutral_shot[i]);
        let rgb: [f32; 3] = std::array::from_fn(|i| m[i][0] * probe[0] + m[i][1] * probe[1] + m[i][2] * probe[2]);
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        m.map(|row| row.map(|v| v / y))
    }

    /// Scene (as-shot Rec.2020) -> scene re-balanced for `kelvin`/`tint`.
    /// Identity at the as-shot values.
    pub fn rebalance(&self, kelvin: f32, tint: f32) -> [[f32; 3]; 3] {
        let shot = self.camera_to_rec2020(self.shot_kelvin, self.shot_tint);
        mul3(&self.camera_to_rec2020(kelvin, tint), &invert3(&shot))
    }
}

#[cfg(test)]
mod absolute_tests {
    use super::*;

    fn panasonic() -> CameraColour {
        // Panasonic DC-S5M2, as rawler reports it.
        CameraColour {
            a: [[1.3022, -0.817, 0.1348], [-0.3478, 1.0901, 0.2974], [0.0031, 0.0471, 0.7057]],
            d65: [[1.0308, -0.4206, -0.0783], [-0.4088, 1.2102, 0.2229], [-0.0125, 0.1051, 0.5912]],
        }
    }

    #[test]
    fn as_shot_matches_lightroom() {
        // P1024033.RW2: rawler WB [1.8671875, 1, 1.9726563]; Lightroom says 4400 K, +23.
        let wb = WhiteBalance::new(panasonic(), [1.0 / 1.8671875, 1.0, 1.0 / 1.9726563]);
        assert!((wb.shot_kelvin - 4400.0).abs() < 100.0, "{}", wb.shot_kelvin);
        assert!((wb.shot_tint - 23.0).abs() < 4.0, "{}", wb.shot_tint);
    }

    #[test]
    fn rebalance_is_identity_as_shot_and_warms_when_raised() {
        let wb = WhiteBalance::new(panasonic(), [1.0 / 1.8671875, 1.0, 1.0 / 1.9726563]);
        let id = wb.rebalance(wb.shot_kelvin, wb.shot_tint);
        for i in 0..3 {
            for j in 0..3 {
                assert!((id[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-3, "{id:?}");
            }
        }
        // Telling it the light was bluer (higher K) makes the picture warmer.
        let warm = wb.rebalance(wb.shot_kelvin + 2000.0, wb.shot_tint);
        let g = [0.18f32, 0.18, 0.18];
        let out: [f32; 3] = std::array::from_fn(|i| warm[i][0] * g[0] + warm[i][1] * g[1] + warm[i][2] * g[2]);
        assert!(out[0] > out[2] * 1.05, "{out:?}");
    }
}
