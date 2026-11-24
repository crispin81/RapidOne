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
/// Level trusts lines only when most of them agree: within this many degrees of their weighted
/// median...
const LEVEL_AGREE_DEG: f32 = 2.0;
/// ...this share of their combined length at least...
const LEVEL_AGREE_SHARE: f32 = 0.6;
/// ...and never turns further than this. On 11 Nov "+ straighten" turned hand-held portraits by up
/// to 34.6 deg: the keystone fit's rotation, extrapolated from three short lines 10 deg and more
/// apart (a sleeve, a door edge), far outside the leans it was fitted to.
const MAX_LEVEL_DEG: f32 = 10.0;
/// Horizontals (Level's fallback when there are too few verticals) asking for a turn past
/// `LEVEL_GENTLE_DEG` must agree more closely than verticals: indoors they are mostly tables, banners and shelves seen at an angle, whose
/// perspective spreads them by a few degrees, where a real horizon is one straight line. On 14 Nov
/// a bar's "Happy Birthday" banner and table edges (-4.6, -3.2, -1.2 deg) turned a straight group
/// shot (P1336868) by +4.1 deg.
const LEVEL_AGREE_HORIZONTAL_DEG: f32 = 1.2;
/// Turns up to this are left to the plain rules (`LEVEL_AGREE_DEG`, lines on one side allowed):
/// the stricter checks are only for the large turns that went wrong, so the many decent small
/// ones stay as they were (on the 326-photo Deborah Slater set, 9 results change, 145 -> 139 turned).
const LEVEL_GENTLE_DEG: f32 = 3.0;
/// Verticals away from the middle lean by perspective as well as by rotation (a camera tilted down
/// at a table makes the left side's verticals lean one way and the right side's the other), so
/// agreeing verticals count only when they sit on both sides of the photo, or near its middle
/// (within this fraction of the width either side), where perspective barely leans them. On 15 Nov
/// three bottle and bucket edges, all on the left, turned a straight table shot (P1346998) by
/// +7.4 deg, where the keystone fit of the same lines gave +0.7.
const LEVEL_CENTRE_FRACTION: f32 = 0.1;
/// ...but only for turns larger than this: perspective on one side shows as a big lean, and smaller
/// one-sided turns were mostly fine, so they are kept as before (6 deg: P1346998 asked 7.4, a
/// threshold of 3 also dropped good 3.5 deg turns on a pillar, P1347067/P1347070).
const LEVEL_ONE_SIDED_MAX_DEG: f32 = 6.0;


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
    /// How many agreeing lines Level's turn is based on: 0 when they disagreed, or asked for more
    /// than `MAX_LEVEL_DEG` (Level then leaves the photo as it is).
    pub level_line_count: usize,
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

/// Level's lean (degrees, the lines' own: dx per dy, or dy per dx for horizontals) when the lines
/// agree (`LEVEL_AGREE_DEG`, `LEVEL_AGREE_SHARE`), and how many lines agreed: their length-weighted
/// mean. Two agreeing lines at least, or one long one (`min_single`, analysis pixels).
fn consensus(lines: &[Line], min_single: f32, agree_deg: f32) -> Option<(f32, usize)> {
    if lines.is_empty() {
        return None;
    }
    let median = weighted_median_lean(lines).atan().to_degrees();
    let agree: Vec<&Line> = lines.iter().filter(|l| (l.lean.atan().to_degrees() - median).abs() <= agree_deg).collect();
    let total: f32 = lines.iter().map(|l| l.votes).sum();
    let agreed: f32 = agree.iter().map(|l| l.votes).sum();
    let enough = agree.len() >= 2 || agree.first().is_some_and(|l| l.votes >= min_single);
    if !enough || agreed < LEVEL_AGREE_SHARE * total {
        return None;
    }
    let mean = agree.iter().map(|l| l.votes * l.lean.atan().to_degrees()).sum::<f32>() / agreed;
    Some((mean, agree.len()))
}

/// The verticals' consensus; a turn past `LEVEL_ONE_SIDED_MAX_DEG` only when perspective can't be
/// what they agree on: the agreeing lines sit on both sides of the middle, or mostly near it
/// (`LEVEL_CENTRE_FRACTION`).
fn vertical_consensus(verticals: &[Line], w: u32, h: u32) -> Option<(f32, usize)> {
    let (deg, n) = consensus(verticals, 0.6 * h as f32, LEVEL_AGREE_DEG)?;
    if deg.abs() <= LEVEL_ONE_SIDED_MAX_DEG {
        return Some((deg, n));
    }
    let band = LEVEL_CENTRE_FRACTION * w as f32;
    // The same lines `consensus` agreed on (within `LEVEL_AGREE_DEG` of the weighted median).
    let median = weighted_median_lean(verticals).atan().to_degrees();
    let agree = verticals.iter().filter(|l| (l.lean.atan().to_degrees() - median).abs() <= LEVEL_AGREE_DEG);
    let (mut left, mut right, mut centre) = (0.0f32, 0.0f32, 0.0f32);
    for l in agree {
        if l.offset < -band {
            left += l.votes;
        } else if l.offset > band {
            right += l.votes;
        } else {
            centre += l.votes;
        }
    }
    let both_sides = left > 0.0 && right > 0.0;
    let mostly_centre = centre >= 0.5 * (left + right + centre);
    (both_sides || mostly_centre).then_some((deg, n))
}

/// The horizontals' consensus as a clockwise turn (past `LEVEL_GENTLE_DEG`, closer agreement than
/// verticals: `LEVEL_AGREE_HORIZONTAL_DEG`). A horizontal with dy/dx = s > 0 runs downhill to the right, i.e.
/// the photo is turned clockwise, so turning back is negative. One alone counts if it runs across
/// half the photo's width.
fn horizon_level(horizontals: &[Line], w: u32) -> Option<(f32, usize)> {
    let min_single = 0.5 * w as f32;
    let (d, n) = consensus(horizontals, min_single, LEVEL_AGREE_DEG)?;
    if d.abs() <= LEVEL_GENTLE_DEG {
        return Some((-d, n));
    }
    consensus(horizontals, min_single, LEVEL_AGREE_HORIZONTAL_DEG).map(|(d, n)| (-d, n))
}

/// Level's turn (clockwise degrees) and the lines it's based on: the verticals' when they agree;
/// the horizontals' (a horizon) only when there are fewer than three verticals, as before 11 Nov
/// (indoors, "horizontals" are mostly tables and floors running into the picture: 23 Oct); (0, 0)
/// when the lines it uses disagree or the turn would pass `MAX_LEVEL_DEG`. A rotation only, so not the keystone fit's `r`, which leans on its convergence
/// term and can land far from every line measured.
fn level_of(verticals: &[Line], horizontals: &[Line], w: u32, h: u32) -> (f32, usize) {
    let within = |t: &(f32, usize)| t.0.abs() <= MAX_LEVEL_DEG;
    // One vertical alone counts if it runs most of the photo's height; one horizontal, half its width.
    let from_v = vertical_consensus(verticals, w, h).filter(within);
    let from_h = horizon_level(horizontals, w).filter(within);
    if verticals.len() >= 3 {
        return from_v.unwrap_or((0.0, 0));
    }
    from_v.or(from_h).unwrap_or((0.0, 0))
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
    let (level_deg, level_lines) = level_of(&verticals, &horizontals, w, h);
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
        // Level weighs only the lines that agree (`level_of`).
        return AutoTransform { rotate_deg, rotate_level_deg: level_deg, vertical_tilt_deg: Some(tilt), line_count: used, level_line_count: level_lines };
    }

    // Not enough verticals: level on the horizontals instead, if there are two or more, or one that
    // runs across half the photo (a real horizon). One short one alone is too easily a table edge or
    // a step, and the photo is better left as it is. (Keystone's own turn; Level's is `level_of`.)
    let trusted = horizontals.len() >= 2 || horizontals.first().is_some_and(|l| l.votes >= 0.5 * w as f32);
    if let Some(h_deg) = from_horizontals.filter(|_| trusted) {
        return AutoTransform { rotate_deg: h_deg, rotate_level_deg: level_deg, vertical_tilt_deg: None, line_count: horizontals.len(), level_line_count: level_lines };
    }
    AutoTransform { rotate_deg: 0.0, rotate_level_deg: level_deg, vertical_tilt_deg: None, line_count: 0, level_line_count: level_lines }
}

/// What `analyse` saw, line by line, for checking a photo it levelled badly (examples/level_check).
pub fn describe(img: &DynamicImage) -> String {
    let small = img.resize(ANALYSIS_DIM, ANALYSIS_DIM, FilterType::Triangle).to_luma8();
    let (w, h) = small.dimensions();
    let (gx, gy) = gradients(&small);
    let mut out = format!("analysed at {w}x{h}\n");
    for (name, vertical) in [("verticals", true), ("horizontals", false)] {
        let lines = find_lines(&gx, &gy, w as usize, h as usize, vertical);
        out += &format!("{name}: {}\n", lines.len());
        for l in &lines {
            out += &format!("  lean {:+6.2} deg  offset {:+6.0} px  run {:4.0} px\n", l.lean.atan().to_degrees(), l.offset, l.votes);
        }
        if vertical && lines.len() >= 3 {
            let (r, k, used) = robust_fit(&lines);
            out += &format!("  fit: r {:+.2} deg, k {:+.6}, {used} kept; weighted median lean {:+.2} deg\n", r.atan().to_degrees(), k, weighted_median_lean(&lines).atan().to_degrees());
        }
    }
    let horizontals = find_lines(&gx, &gy, w as usize, h as usize, false);
    let verticals = find_lines(&gx, &gy, w as usize, h as usize, true);
    let (deg, n) = level_of(&verticals, &horizontals, w, h);
    out += &format!("level: {deg:+.2} deg from {n} agreeing lines\n");
    out
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

    /// Lines as level_check measured them on the 11 Nov portraits: (lean degrees, run px).
    fn lines(leans: &[(f32, f32)]) -> Vec<Line> {
        leans.iter().map(|&(d, run)| Line { lean: d.to_radians().tan(), offset: 0.0, votes: run }).collect()
    }

    #[test]
    fn level_ignores_lines_that_disagree() {
        let (w, h) = (1000, 667);
        // P1346896 (was turned +34.6): two agree near 0, one is 10 degrees off.
        let (d, n) = level_of(&lines(&[(-0.0, 103.0), (0.29, 119.0), (-9.93, 90.0)]), &[], w, h);
        assert!(d.abs() < 0.3 && n == 2, "{d} {n}");
        // P1336791 (was +33.2): all three disagree, no horizontals: left alone.
        assert_eq!(level_of(&lines(&[(-14.31, 113.0), (5.14, 81.0), (-9.09, 93.0)]), &[], w, h), (0.0, 0));
        // P1336888 (was +14.3): one of three near 0, one short horizontal: left alone.
        assert_eq!(level_of(&lines(&[(0.57, 88.0), (15.38, 92.0), (-14.04, 86.0)]), &lines(&[(2.86, 239.0)]), w, h), (0.0, 0));
        // P1336762 (was -11.9): five verticals, scattered: left alone (its horizontals aren't used
        // with three verticals or more).
        assert_eq!(level_of(&lines(&[(-8.25, 101.0), (-3.15, 100.0), (-12.95, 92.0), (-11.59, 91.0), (-1.72, 104.0)]), &lines(&[(0.29, 181.0), (0.0, 135.0), (1.15, 141.0)]), w, h), (0.0, 0));
        // P1336720 (was -0.9, fine): one vertical, four horizontals that agree.
        let (d, n) = level_of(&lines(&[(-1.72, 181.0)]), &lines(&[(1.15, 231.0), (0.86, 413.0), (-0.29, 120.0), (0.86, 152.0)]), w, h);
        assert!((d + 0.9).abs() < 0.4 && n >= 3, "{d} {n}");
        // P1336790 (was -15.1): verticals agree, but on 17 degrees, past the limit.
        assert_eq!(level_of(&lines(&[(16.96, 99.0), (17.74, 90.0), (11.03, 83.0)]), &lines(&[(1.43, 182.0)]), w, h), (0.0, 0));
        // P1336733 (was -0.5, fine): still levelled about the same.
        let (d, _) = level_of(&lines(&[(-0.86, 211.0), (-1.15, 102.0), (0.0, 158.0), (-1.72, 102.0)]), &lines(&[(12.13, 132.0)]), w, h);
        assert!((d + 0.7).abs() < 0.3, "{d}");
    }

    /// Lines with where they sit: (lean degrees, offset px from the middle, run px).
    fn placed(lines: &[(f32, f32, f32)]) -> Vec<Line> {
        lines.iter().map(|&(d, offset, run)| Line { lean: d.to_radians().tan(), offset, votes: run }).collect()
    }

    #[test]
    fn level_leaves_one_sided_and_spread_lines_alone() {
        let (w, h) = (1000, 667);
        // P1346998 (15 Nov, was +7.4): three agreeing verticals, all on the left (bottles and an ice
        // bucket leaning by perspective), no horizontals: left alone.
        let v = placed(&[(7.97, -337.0, 110.0), (6.28, -357.0, 101.0), (-4.86, 297.0, 91.0), (7.97, -415.0, 89.0), (-13.50, -149.0, 84.0)]);
        assert_eq!(level_of(&v, &[], w, h), (0.0, 0));
        // P1336868 (14 Nov, was +4.1): no verticals, three horizontals spread over 3.4 deg (a
        // banner and table edges): left alone by Level.
        let hz = placed(&[(-1.15, -206.0, 173.0), (-4.57, -310.0, 504.0), (-3.15, -270.0, 236.0)]);
        assert_eq!(level_of(&[], &hz, w, h), (0.0, 0));
        assert_eq!(horizon_level(&hz, w), None);
        // A turned room: verticals on both sides agree on -7 deg: levelled.
        let (d, n) = level_of(&placed(&[(-7.1, -350.0, 150.0), (-6.9, 320.0, 140.0), (-7.0, -200.0, 100.0)]), &[], w, h);
        assert!((d + 7.0).abs() < 0.2 && n == 3, "{d} {n}");
        // The same lean on one side only: left alone.
        assert_eq!(level_of(&placed(&[(-7.1, -350.0, 150.0), (-6.9, -320.0, 140.0), (-7.0, -200.0, 100.0)]), &[], w, h), (0.0, 0));
        // A smaller one-sided turn (under 6 deg) is kept, as before.
        let (d, n) = level_of(&placed(&[(-2.0, -380.0, 150.0), (-1.8, -300.0, 120.0), (9.0, 350.0, 90.0)]), &[], w, h);
        assert!((d + 1.9).abs() < 0.2 && n == 2, "{d} {n}");
        // Small horizontal turns keep the 2 deg agreement: two lines 1.6 deg apart still level by ~-1.
        let (d, n) = level_of(&[], &placed(&[(0.2, -100.0, 300.0), (1.8, 100.0, 250.0)]), w, h);
        assert!((d + 0.93).abs() < 0.2 && n == 2, "{d} {n}");
        // Agreeing verticals near the middle (a door frame behind the subject): levelled.
        let (d, n) = level_of(&placed(&[(7.5, -60.0, 200.0), (7.3, 40.0, 150.0), (-6.0, 380.0, 90.0)]), &[], w, h);
        assert!((d - 7.4).abs() < 0.2 && n == 2, "{d} {n}");
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
