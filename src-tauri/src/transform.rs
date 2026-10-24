//! Auto level / auto keystone: finds the photo's near-vertical straight lines
//! (building edges, door frames, window sides) and measures how they lean.
//!
//! Each near-vertical line is described as `x = x0 + t * (y - cy)`, where `t`
//! is its lean (dx per dy, y pointing down) and `x0` where it crosses the
//! middle row. In a photo that is merely rotated, every vertical leans the
//! same way: `t` is constant. When the camera was tilted up or down, the
//! verticals converge on a vanishing point and the lean grows with distance
//! from the centre: `t = r + k * (x0 - cx)`. Fitting that line through all
//! detected lines gives both answers at once - `r` is the rotation, `k` the
//! keystone - which is why auto keystone also levels.
//!
//! When there are too few verticals (landscapes, seascapes), auto level falls
//! back to near-horizontal lines such as the horizon.
//!
//! As Lightroom's Upright (Adobe's "Automatic Upright Adjustment of Photographs",
//! Lee et al. 2012), only genuinely straight segments count: each candidate is
//! traced along its length and must be backed by a continuous run of edge pixels
//! leaning its way, so a person's outline (curved: body, hair, a dress) drops out
//! and walls, frames and horizons stay.

use image::{imageops::FilterType, DynamicImage, GrayImage};

/// Long edge the photo is shrunk to for line finding - lines a building's
/// height long survive easily, and it keeps the search to milliseconds.
const ANALYSIS_DIM: u32 = 1000;
/// Steepest lean considered: about 22 degrees either side.
const MAX_LEAN: f32 = 0.4;
const LEAN_STEP: f32 = 0.005;
const POSITION_BIN_PX: f32 = 2.0;
/// Edge pixels weaker than this (gradient magnitude, 0..~1440) are ignored.
const EDGE_THRESHOLD: f32 = 60.0;
/// A line must be at least this fraction of the image height (or width for
/// horizontals) to count - short edges are mostly texture, not structure.
const MIN_LINE_FRACTION: f32 = 0.12;
const MAX_LINES: usize = 30;
/// A line's straight run: edge pixels within a pixel of the exact line, leaning within this of it
/// (about 1.4 degrees, room for a little lens bend; a photo's soft edges give each pixel the line's
/// true lean, which a curve's drift from it fails)...
const STRAIGHT_LEAN: f32 = 0.025;
/// ...with gaps no longer than this (analysis pixels)...
const STRAIGHT_GAP: usize = 4;
/// ...unbroken for at least this fraction of the image height (width for horizontals).
const STRAIGHT_RUN_FRACTION: f32 = 0.12;


#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoTransform {
    /// Clockwise rotation (degrees) that levels the photo, from its verticals
    /// (what keystone uses, with the tilt).
    pub rotate_deg: f32,
    /// The rotation for Level alone (today the same as `rotate_deg`: the verticals,
    /// else the horizontals); kept apart so Level can weigh things its own way.
    pub rotate_level_deg: f32,
    /// Camera tilt to undo (degrees): positive means it was tilted up, so the
    /// verticals converge towards the top. None when there weren't enough
    /// vertical lines to measure it.
    pub vertical_tilt_deg: Option<f32>,
    /// How many straight lines the measurement is based on.
    pub line_count: usize,
}

struct Line {
    /// Lean: dx per dy for near-verticals, dy per dx for near-horizontals.
    lean: f32,
    /// Offset of the line from the image centre where it crosses the middle,
    /// in analysis pixels.
    offset: f32,
    votes: f32,
}

/// Sobel gradients. Returns (gx, gy) per pixel; borders stay zero.
fn gradients(gray: &GrayImage) -> (Vec<f32>, Vec<f32>) {
    let (w, h) = gray.dimensions();
    let (w, h) = (w as usize, h as usize);
    let px = |x: usize, y: usize| gray.as_raw()[y * w + x] as f32;
    let mut gx = vec![0.0; w * h];
    let mut gy = vec![0.0; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            gx[y * w + x] = (px(x + 1, y - 1) + 2.0 * px(x + 1, y) + px(x + 1, y + 1))
                - (px(x - 1, y - 1) + 2.0 * px(x - 1, y) + px(x - 1, y + 1));
            gy[y * w + x] = (px(x - 1, y + 1) + 2.0 * px(x, y + 1) + px(x + 1, y + 1))
                - (px(x - 1, y - 1) + 2.0 * px(x, y - 1) + px(x + 1, y - 1));
        }
    }
    (gx, gy)
}

/// Hough-style search for near-vertical lines (or near-horizontal ones, by
/// swapping axes). Each strong edge pixel votes only for leans close to its
/// own edge direction, which keeps texture from forming phantom lines.
fn find_lines(gx: &[f32], gy: &[f32], w: usize, h: usize, vertical: bool) -> Vec<Line> {
    // Work in a frame where the lines run along "v" and are positioned by "u".
    let (u_len, v_len) = if vertical { (w, h) } else { (h, w) };
    let cu = u_len as f32 / 2.0;
    let cv = v_len as f32 / 2.0;
    let lean_bins = (2.0 * MAX_LEAN / LEAN_STEP) as usize + 1;
    let pos_bins = (u_len as f32 / POSITION_BIN_PX) as usize + 1;
    let mut acc = vec![0.0f32; lean_bins * pos_bins];

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            // Across-the-line gradient (gu) must dominate the along-the-line one.
            let (gu, gv) = if vertical { (gx[i], gy[i]) } else { (gy[i], gx[i]) };
            let mag = (gu * gu + gv * gv).sqrt();
            if mag < EDGE_THRESHOLD || gu.abs() < gv.abs() {
                continue;
            }
            // The edge runs perpendicular to the gradient: its lean is -gv/gu.
            let local_lean = -gv / gu;
            if local_lean.abs() > MAX_LEAN + 0.05 {
                continue;
            }
            let (u, v) = if vertical { (x as f32, y as f32) } else { (y as f32, x as f32) };
            let lo = ((local_lean - 0.05 + MAX_LEAN) / LEAN_STEP).floor().max(0.0) as usize;
            let hi = (((local_lean + 0.05 + MAX_LEAN) / LEAN_STEP).ceil() as usize).min(lean_bins - 1);
            for li in lo..=hi {
                let lean = li as f32 * LEAN_STEP - MAX_LEAN;
                let u0 = u - lean * (v - cv);
                if u0 < 0.0 || u0 >= u_len as f32 {
                    continue;
                }
                acc[li * pos_bins + (u0 / POSITION_BIN_PX) as usize] += 1.0;
            }
        }
    }

    // Strongest peaks first, suppressing near-duplicates of each accepted line.
    // Absolute floor (a real structural line), and relative to the strongest
    // line so the weak "ghost" fits beside each real edge don't get counted.
    let strongest = acc.iter().cloned().fold(0.0f32, f32::max);
    let min_votes = (MIN_LINE_FRACTION * v_len as f32).max(0.3 * strongest);
    let mut order: Vec<usize> = (0..acc.len()).filter(|&i| acc[i] >= min_votes).collect();
    order.sort_by(|&a, &b| acc[b].total_cmp(&acc[a]));
    let mut lines: Vec<Line> = Vec::new();
    for idx in order {
        let lean = (idx / pos_bins) as f32 * LEAN_STEP - MAX_LEAN;
        let u0 = (idx % pos_bins) as f32 * POSITION_BIN_PX + POSITION_BIN_PX / 2.0;
        let duplicate = lines
            .iter()
            .any(|l| (l.offset + cu - u0).abs() < 20.0 && (l.lean - lean).abs() < 0.08);
        if duplicate {
            continue;
        }
        // Straight along its length (a segment, not a curve's tangent): its longest unbroken run
        // of edge pixels leaning its way; that run is its weight.
        let run = straight_run(gx, gy, w, h, vertical, lean, u0, cv);

        if run < STRAIGHT_RUN_FRACTION * v_len as f32 {
            continue;
        }
        lines.push(Line { lean, offset: u0 - cu, votes: run });
        if lines.len() >= MAX_LINES {
            break;
        }
    }
    lines
}

/// The longest run (analysis pixels) along the line `u = u0 + lean * (v - cv)` of edge pixels
/// within a pixel of it, strong enough and facing its way (`STRAIGHT_LEAN`), gaps of up to
/// `STRAIGHT_GAP` allowed.
fn straight_run(gx: &[f32], gy: &[f32], w: usize, h: usize, vertical: bool, lean: f32, u0: f32, cv: f32) -> f32 {
    let (u_len, v_len) = if vertical { (w, h) } else { (h, w) };
    let (mut best, mut run, mut gap) = (0usize, 0usize, 0usize);
    for v in 1..v_len.saturating_sub(1) {
        let u = u0 + lean * (v as f32 - cv);
        let on = (-1i32..=1).any(|du| {
            let uu = u.round() as i32 + du;
            if uu < 1 || uu >= u_len as i32 - 1 {
                return false;
            }
            let (x, y) = if vertical { (uu as usize, v) } else { (v, uu as usize) };
            let i = y * w + x;
            let (gu, gv) = if vertical { (gx[i], gy[i]) } else { (gy[i], gx[i]) };
            let mag = (gu * gu + gv * gv).sqrt();
            mag >= EDGE_THRESHOLD && gu.abs() >= gv.abs() && (-gv / gu - lean).abs() <= STRAIGHT_LEAN
        });
        if on {
            run += 1 + gap;
            gap = 0;
            best = best.max(run);
        } else if run > 0 {
            gap += 1;
            if gap > STRAIGHT_GAP {
                run = 0;
                gap = 0;
            }
        }
    }
    best as f32
}

/// Weighted least-squares fit of `lean = r + k * offset`. Returns (r, k).
fn fit_lean(lines: &[&Line]) -> (f32, f32) {
    let sw: f32 = lines.iter().map(|l| l.votes).sum();
    let mx = lines.iter().map(|l| l.votes * l.offset).sum::<f32>() / sw;
    let my = lines.iter().map(|l| l.votes * l.lean).sum::<f32>() / sw;
    let sxx: f32 = lines.iter().map(|l| l.votes * (l.offset - mx).powi(2)).sum();
    let sxy: f32 = lines.iter().map(|l| l.votes * (l.offset - mx) * (l.lean - my)).sum();
    let k = if sxx > 1e-6 { sxy / sxx } else { 0.0 };
    (my - k * mx, k)
}

/// Fit, then refit without lines that disagree badly (a diagonal roof edge,
/// a leaning lamp post) so a few strays can't skew the answer.
fn robust_fit(lines: &[Line]) -> (f32, f32, usize) {
    let all: Vec<&Line> = lines.iter().collect();
    let (r, k) = fit_lean(&all);
    let mut residuals: Vec<f32> = lines.iter().map(|l| (l.lean - r - k * l.offset).abs()).collect();
    residuals.sort_by(f32::total_cmp);
    let cutoff = (residuals[residuals.len() / 2] * 2.5).max(0.01);
    let kept: Vec<&Line> = lines.iter().filter(|l| (l.lean - r - k * l.offset).abs() <= cutoff).collect();
    if kept.len() < 3 {
        return (r, k, lines.len());
    }
    let (r, k) = fit_lean(&kept);
    (r, k, kept.len())
}

fn weighted_median_lean(lines: &[Line]) -> f32 {
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by(|a, b| a.lean.total_cmp(&b.lean));
    let half = sorted.iter().map(|l| l.votes).sum::<f32>() / 2.0;
    let mut running = 0.0;
    for l in &sorted {
        running += l.votes;
        if running >= half {
            return l.lean;
        }
    }
    0.0
}

/// `focal_ratio` is the lens's focal length as a fraction of the image's long
/// edge (35mm-equivalent focal length / 36). It turns the measured
/// convergence into a camera tilt angle.
pub fn analyse(img: &DynamicImage, focal_ratio: f32) -> AutoTransform {
    let small = img.resize(ANALYSIS_DIM, ANALYSIS_DIM, FilterType::Triangle).to_luma8();
    let (w, h) = small.dimensions();
    let (gx, gy) = gradients(&small);
    let verticals = find_lines(&gx, &gy, w as usize, h as usize, true);
    // A horizontal with dy/dx = s > 0 runs downhill to the right, i.e. the photo
    // is turned clockwise, so turning back is negative: -atan(s).
    let horizontals = find_lines(&gx, &gy, w as usize, h as usize, false);
    let from_horizontals = (!horizontals.is_empty()).then(|| -weighted_median_lean(&horizontals).atan().to_degrees());

    if verticals.len() >= 3 {
        let (r, k, used) = robust_fit(&verticals);
        // Lines leaning right at the top have r < 0: the photo is turned
        // clockwise, so it needs turning back anticlockwise (negative degrees).
        let rotate_deg = r.atan().to_degrees();
        // k > 0: lines converge upward, i.e. the camera was tilted up. Distance
        // to the vanishing point is 1/k; tilt = atan(focal / distance).
        let focal_px = focal_ratio * w.max(h) as f32;
        let tilt = (focal_px * k).atan().to_degrees();
        // Level on the verticals too: weighing the horizontals in (as Lightroom's Level does) tilted
        // indoor photos, whose "horizontals" are mostly tables and floors running into the picture
        // (tried 23 Oct on a wedding: two of three big changes were worse).
        return AutoTransform { rotate_deg, rotate_level_deg: rotate_deg, vertical_tilt_deg: Some(tilt), line_count: used };
    }

    // Not enough verticals: level on the horizontals instead, if there are two or more, or one that
    // runs across half the photo (a real horizon). One short one alone is too easily a table edge or
    // a step, and the photo is better left as it is.
    let trusted = horizontals.len() >= 2 || horizontals.first().is_some_and(|l| l.votes >= 0.5 * w as f32);
    if let Some(h_deg) = from_horizontals.filter(|_| trusted) {
        return AutoTransform { rotate_deg: h_deg, rotate_level_deg: h_deg, vertical_tilt_deg: None, line_count: horizontals.len() };
    }
    AutoTransform { rotate_deg: 0.0, rotate_level_deg: 0.0, vertical_tilt_deg: None, line_count: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, RgbImage};

    /// Draws dark vertical-ish bars on white, with lean `t = r + k * offset`, their edges
    /// anti-aliased as a photo's are (each pixel shaded by how much of it the bar covers): hard
    /// 1-pixel steps, as no camera makes, give every edge pixel an upright lean.
    fn synthetic(r: f32, k: f32) -> DynamicImage {
        let (w, h) = (800u32, 600u32);
        let mut img = GrayImage::from_pixel(w, h, Luma([230]));
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        for &x0 in &[-300.0f32, -180.0, -60.0, 60.0, 180.0, 300.0] {
            let t = r + k * x0;
            for y in 0..h {
                let xc = cx + x0 + t * (y as f32 - cy);
                for x in (xc as i32 - 6)..=(xc as i32 + 6) {
                    if x < 0 || x as u32 >= w {
                        continue;
                    }
                    // The share of pixel [x, x + 1) inside the bar [xc - 4.5, xc + 4.5).
                    let cover = ((x as f32 + 1.0).min(xc + 4.5) - (x as f32).max(xc - 4.5)).clamp(0.0, 1.0);
                    let old = img.get_pixel(x as u32, y)[0] as f32;
                    img.put_pixel(x as u32, y, Luma([(old + (30.0 - old) * cover).round() as u8]));
                }
            }
        }
        DynamicImage::ImageRgb8(DynamicImage::ImageLuma8(img).to_rgb8())
    }

    #[test]
    fn measures_pure_rotation() {
        // Lines leaning right at the top by 3 degrees: needs -3 degrees.
        let result = analyse(&synthetic(-(3f32.to_radians().tan()), 0.0), 1.0);
        assert!((result.rotate_deg + 3.0).abs() < 0.5, "{:?}", result);
        assert!(result.vertical_tilt_deg.unwrap().abs() < 2.0, "{:?}", result);
    }

    #[test]
    fn measures_upward_tilt() {
        // Converging upward (k > 0) means the camera was tilted up.
        let result = analyse(&synthetic(0.0, 0.0004), 1.0);
        assert!(result.rotate_deg.abs() < 0.5, "{:?}", result);
        assert!(result.vertical_tilt_deg.unwrap() > 5.0, "{:?}", result);
    }

    #[test]
    fn blank_image_finds_nothing() {
        let blank = DynamicImage::ImageRgb8(RgbImage::from_pixel(400, 300, image::Rgb([128, 128, 128])));
        let result = analyse(&blank, 1.0);
        assert_eq!(result.line_count, 0);
        assert_eq!(result.rotate_deg, 0.0);
    }

    #[test]
    fn measures_rotation_and_tilt_together() {
        let result = analyse(&synthetic(-(2f32.to_radians().tan()), 0.0003), 1.0);
        assert!((result.rotate_deg + 2.0).abs() < 0.6, "{:?}", result);
        assert!(result.vertical_tilt_deg.unwrap() > 3.0, "{:?}", result);
    }

    #[test]
    fn levels_on_horizon_when_no_verticals() {
        // One long dark-over-light boundary running downhill to the right by
        // 2 degrees (dy/dx > 0, y down): the photo is turned clockwise, so it
        // needs about -2 degrees.
        let (w, h) = (900u32, 500u32);
        let slope = 2f32.to_radians().tan();
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let horizon = h as f32 / 2.0 + slope * (x as f32 - w as f32 / 2.0);
                // Anti-aliased, as a photo's horizon is: the share of the pixel above the line.
                let above = (horizon - y as f32).clamp(0.0, 1.0);
                img.put_pixel(x, y, Luma([(60.0 + 140.0 * above).round() as u8]));
            }
        }
        let result = analyse(&DynamicImage::ImageLuma8(img), 1.0);
        assert!(result.vertical_tilt_deg.is_none(), "{:?}", result);
        assert!((result.rotate_deg + 2.0).abs() < 0.5, "{:?}", result);
    }
}
