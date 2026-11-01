//! The pickers that read a colour off the photo where it is clicked, as
//! Lightroom's eyedroppers do: White balance (click something that should be
//! neutral: Kelvin and Tint that make it grey) and Defringe (click a fringe:
//! the hue range round its colour).
//!
//! The click is a point on the photo as shown (crop, straighten, keystone and
//! quarter turns applied), traced back to the developed photo as the zoomed
//! view does (`zoom::source_of`), then through the lens correction, so the
//! colour read is the one under the pointer.

use crate::develop::Scene;
use crate::pipeline::RenderSettings;

/// What a click read.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    /// Kelvin and Tint (Lightroom's numbers) that make the clicked colour neutral;
    /// None where it can't be (black, or a channel at zero).
    pub temperature: Option<f32>,
    pub tint: Option<f32>,
    /// The fringe's kind ("purple" or "green") and the hue range round it (0..100),
    /// None for a colour too grey to be a fringe.
    pub fringe: Option<(String, f32, f32)>,
}

/// The scene light round a point of the photo as shown (`fx`, `fy`: fractions), `r` pixels
/// each way: None outside the photo.
fn samples(scene: &Scene, settings: &RenderSettings, fx: f32, fy: f32, r: i32) -> Option<Vec<[f32; 3]>> {
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    let img = &scene.image;
    let (w, h) = (img.width, img.height);
    let (sw, sh) = crate::zoom::shown_size(&settings.crop, w, h);
    let (x, y) = crate::zoom::source_of(settings, w, h)(fx * sw, fy * sh)?;
    let lens = crate::commands::lens_correction(scene, settings);
    let mut out = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (x + dx as f32, y + dy as f32);
            if px < 0.0 || py < 0.0 || px > (w - 1) as f32 || py > (h - 1) as f32 {
                continue;
            }
            out.push(match &lens {
                Some(l) => {
                    let (src, gain) = l.source_at(px, py, w, h);
                    std::array::from_fn(|ch| crate::commands::bilinear(&img.px, w, h, src[ch].0, src[ch].1)[ch] * gain)
                }
                None => crate::commands::bilinear(&img.px, w, h, px, py),
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Kelvin and Tint that make `rgb` (scene light as shot, linear Rec.2020) neutral.
pub fn white_balance_for(wb: Option<&crate::whitebalance::WhiteBalance>, rgb: [f32; 3]) -> Option<(f32, f32)> {
    if rgb.iter().any(|v| !v.is_finite() || *v <= 1e-6) {
        return None;
    }
    let (kelvin, tint) = match wb {
        // A RAW: back to the sensor's values for that colour (the as-shot chain undone), which
        // are the "neutral" of the light that would make it grey; that light in Kelvin and Tint.
        Some(wb) => {
            let shot = wb.camera_to_rec2020(wb.shot_kelvin, wb.shot_tint);
            let inv = crate::whitebalance::invert3(&shot);
            let bal: [f32; 3] = std::array::from_fn(|i| inv[i][0] * rgb[0] + inv[i][1] * rgb[1] + inv[i][2] * rgb[2]);
            let raw: [f32; 3] = std::array::from_fn(|i| bal[i] * wb.neutral_shot[i]);
            if raw.iter().any(|v| *v <= 1e-9) {
                return None;
            }
            let (k, duv) = wb.camera.white_for_neutral([raw[0] / raw[1], 1.0, raw[2] / raw[1]]);
            (k, duv * crate::whitebalance::TINT_PER_DUV)
        }
        // A TIFF or JPEG: `pipeline::simple_rebalance` undone (red and blue gains for Kelvin,
        // green for Tint) for the gains that make it grey.
        None => {
            let warm = (rgb[2] / rgb[0]).sqrt();
            let green = (rgb[0] * rgb[2]).sqrt() / rgb[1];
            let mireds = 1.0e6 / crate::pipeline::NEUTRAL_KELVIN - warm.log2() / 0.0035;
            if mireds <= 20.0 {
                return None;
            }
            (1.0e6 / mireds, -green.log2() / 0.003)
        }
    };
    (kelvin.is_finite() && tint.is_finite()).then(|| (kelvin.clamp(2000.0, 50000.0), tint.clamp(-150.0, 150.0)))
}

/// Reads the photo where it was clicked.
pub fn pick(scene: &Scene, settings: &RenderSettings, fx: f32, fy: f32) -> Result<Picked, String> {
    // White balance: the average of a small square (one pixel is noisy).
    let around = samples(scene, settings, fx, fy, 2).ok_or("That's outside the photo")?;
    let n = around.len() as f32;
    let mean: [f32; 3] = std::array::from_fn(|c| around.iter().map(|p| p[c]).sum::<f32>() / n);
    let wb = white_balance_for(scene.wb.as_ref(), mean);
    // A fringe is thin: its most coloured pixel close by.
    let fringe = samples(scene, settings, fx, fy, 3)
        .and_then(|s| s.into_iter().max_by(|a, b| crate::defringe::hue_sat(*a).1.total_cmp(&crate::defringe::hue_sat(*b).1)))
        .and_then(crate::defringe::range_for)
        .map(|(kind, lo, hi)| (kind.to_string(), lo, hi));
    Ok(Picked { temperature: wb.map(|w| w.0), tint: wb.map(|w| w.1), fringe })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whitebalance::{CameraColour, WhiteBalance};

    fn panasonic() -> WhiteBalance {
        let camera = CameraColour {
            a: [[1.3022, -0.817, 0.1348], [-0.3478, 1.0901, 0.2974], [0.0031, 0.0471, 0.7057]],
            d65: [[1.0308, -0.4206, -0.0783], [-0.4088, 1.2102, 0.2229], [-0.0125, 0.1051, 0.5912]],
        };
        WhiteBalance::new(camera, [1.0 / 1.8671875, 1.0, 1.0 / 1.9726563])
    }

    #[test]
    fn grey_under_another_light_gives_that_light() {
        let wb = panasonic();
        // A grey card lit by 3200 K, tint +10, photographed with the camera's as-shot setting:
        // the sensor reads that light's neutral, which the as-shot chain turns into this colour.
        let (k, tint) = (3200.0f32, 10.0f32);
        let n = wb.camera.neutral_for(k, tint / crate::whitebalance::TINT_PER_DUV);
        let bal: [f32; 3] = std::array::from_fn(|i| n[i] / wb.neutral_shot[i] * 0.18);
        let m = wb.camera_to_rec2020(wb.shot_kelvin, wb.shot_tint);
        let rgb: [f32; 3] = std::array::from_fn(|i| m[i][0] * bal[0] + m[i][1] * bal[1] + m[i][2] * bal[2]);
        let (pk, pt) = white_balance_for(Some(&wb), rgb).unwrap();
        assert!((pk - k).abs() < 30.0, "{pk}");
        assert!((pt - tint).abs() < 1.0, "{pt}");
        // A grey that is already grey: the as-shot white balance.
        let grey = std::array::from_fn(|i| m[i][0] * 0.18 + m[i][1] * 0.18 + m[i][2] * 0.18);
        let (gk, gt) = white_balance_for(Some(&wb), grey).unwrap();
        assert!((gk - wb.shot_kelvin).abs() < 30.0 && (gt - wb.shot_tint).abs() < 1.0, "{gk} {gt}");
    }

    #[test]
    fn simple_white_balance_round_trips() {
        // A JPEG with a warm, green cast: the picked Kelvin/Tint's gains make it grey.
        let rgb = [0.5f32, 0.45, 0.3];
        let (k, t) = white_balance_for(None, rgb).unwrap();
        let m = crate::pipeline::simple_rebalance(Some(k), Some(t));
        let out: [f32; 3] = std::array::from_fn(|i| m[i][i] * rgb[i]);
        assert!((out[0] - out[1]).abs() < 1e-3 && (out[1] - out[2]).abs() < 1e-3, "{out:?}");
    }
}
