//! Halo-free depth edges (phase 1c §4): two surfaces per pixel where the photo's depth jumps.
//!
//! A depth map made at a model's own resolution has soft edges: a subject's silhouette, hair and a
//! tree's leaves are blended into the background over a pixel or two. A depth *mask* built from a
//! soft edge fades there, so a band that holds the subject but not the background still tints the
//! background's rim (a halo), and the subject's own edge goes half applied.
//!
//! So at a depth edge the pixel gets **two** depths, a near and a far one, and a **coverage**
//! alpha: `band = alpha·band(near) + (1 − alpha)·band(far)`. Never a blended in-between depth: a
//! band between the two still resolves to exactly which surface it is on the pixel's own share of
//! the two.
//!
//! The edges come from the refined control map itself: a small two-means on log distance (the
//! control scale *is* log distance) in a window, kept only where the window is clearly **bimodal**
//! — the largest gap between neighbouring sorted values is most of the window's spread, and both
//! sides hold a share of it. A receding ground plane (a ramp, no gap) is not an edge. Then the
//! coverage is guided-filtered by the photo's own luminance (so a hair's matte follows the strands,
//! not the depth map's own staircase) and clamped to the edge's own coverage in the window, so the
//! filter can't invent a near side where there is none.
//!
//! Where BiRefNet's subject matte exists it wins at the subject's outline: alpha is the matte
//! itself, near is the subject's own depth (the median of its interior near the edge) and far is
//! the background's, which is what keeps hair clean.
//!
//! The mask reads `band`; Bokeh (phase 2) and Mist (phase 3) read alpha/near/far directly, so a
//! blur or a fog can also resolve the two surfaces separately instead of blurring the edge.

use crate::gradient::{band_weight, Grid};
use crate::tools::{guided_filter, min_filter};

/// The two surfaces and their coverage, at the refined control map's size (2048 on the long side).
#[derive(Clone, Debug)]
pub struct EdgeMaps {
    pub w: usize,
    pub h: usize,
    /// 1: all of the near surface (the usual, away from an edge); 0: all the far one.
    pub alpha: Vec<f32>,
    /// The nearer surface's control depth (0..1, 1 = nearest).
    pub near: Vec<f32>,
    /// The farther surface's control depth.
    pub far: Vec<f32>,
}

impl EdgeMaps {
    /// A map with no edges: every pixel wholly its own control depth (the formula still holds).
    pub fn flat(control: &Grid) -> EdgeMaps {
        EdgeMaps { w: control.w, h: control.h, alpha: vec![1.0; control.data.len()], near: control.data.clone(), far: control.data.clone() }
    }

    /// The depth mask's band here: `alpha·band(near) + (1 − alpha)·band(far)`.
    pub fn band(&self, near: f32, far: f32, fade: f32) -> Grid {
        let data = (0..self.alpha.len())
            .map(|i| {
                let a = self.alpha[i].clamp(0.0, 1.0);
                let n = band_weight(self.near[i], near, far, fade);
                let f = band_weight(self.far[i], near, far, fade);
                a * n + (1.0 - a) * f
            })
            .collect();
        Grid { w: self.w, h: self.h, data }
    }

    /// The share of the frame whose pixels are part way between two surfaces (for the notes).
    pub fn edge_share(&self) -> f32 {
        if self.alpha.is_empty() {
            return 0.0;
        }
        self.alpha.iter().filter(|a| **a > 0.01 && **a < 0.99).count() as f32 / self.alpha.len() as f32
    }

    /// One pixel's facts, bilinear (for phase 2's Bokeh and phase 3's Mist).
    pub fn at(&self, x: f32, y: f32) -> (f32, f32, f32) {
        let read = |data: &[f32]| {
            let fx = (x * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
            let fy = (y * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let top = data[y0 * self.w + x0] * (1.0 - tx) + data[y0 * self.w + x1] * tx;
            let bottom = data[y1 * self.w + x0] * (1.0 - tx) + data[y1 * self.w + x1] * tx;
            top * (1.0 - ty) + bottom * ty
        };
        (read(&self.near), read(&self.far), read(&self.alpha))
    }
}

/// The smallest jump in control depth that counts as a step: 4% of the log range between the
/// scene's anchors (about a 10% change in distance).
pub const MIN_STEP: f32 = 0.04;
/// How much of the window's spread the largest gap must hold to be bimodal: a step leaves one big
/// gap, a ramp leaves many small ones.
pub const BIMODAL_GAP: f32 = 0.5;
/// Both surfaces must hold at least this share of the window (a lone speck is not an edge).
pub const MIN_SHARE: f32 = 0.15;

/// Whether a window's values (sorted) are clearly two surfaces, and their means and near share.
/// `near` is the higher control value (the nearer surface).
pub fn bimodal(sorted: &[f32]) -> Option<(f32, f32, f32)> {
    if sorted.len() < 9 {
        return None;
    }
    let (lo, hi) = (sorted[0], sorted[sorted.len() - 1]);
    let spread = hi - lo;
    if !(spread >= MIN_STEP) {
        return None;
    }
    let mut gap = 0.0f32;
    let mut at = 0usize;
    for i in 1..sorted.len() {
        let g = sorted[i] - sorted[i - 1];
        if g > gap {
            gap = g;
            at = i;
        }
    }
    if gap < BIMODAL_GAP * spread {
        return None;
    }
    let n = sorted.len() as f32;
    let (low, high) = (&sorted[..at], &sorted[at..]);
    if (low.len() as f32) < MIN_SHARE * n || (high.len() as f32) < MIN_SHARE * n {
        return None;
    }
    let mean = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
    Some((mean(high), mean(low), high.len() as f32 / n))
}

/// The raw per-pixel edges of a control map: for each pixel, its window's two-means. Pixels that
/// aren't on an edge keep their own control depth on both sides and alpha 1.
pub fn detect(control: &Grid, radius: usize) -> EdgeMaps {
    use rayon::prelude::*;
    let (w, h, r) = (control.w, control.h, radius.max(1));
    let mut alpha = vec![1.0f32; w * h];
    let mut near = control.data.clone();
    let mut far = control.data.clone();
    let rows: Vec<(Vec<f32>, Vec<f32>, Vec<f32>)> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut a = vec![1.0f32; w];
            let mut n = control.data[y * w..(y + 1) * w].to_vec();
            let mut f = n.clone();
            let mut window = Vec::with_capacity((2 * r + 1) * (2 * r + 1));
            for x in 0..w {
                window.clear();
                for dy in -(r as isize)..=(r as isize) {
                    let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    for dx in -(r as isize)..=(r as isize) {
                        let xx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                        window.push(control.data[yy * w + xx]);
                    }
                }
                window.sort_by(f32::total_cmp);
                if let Some((near_c, far_c, share)) = bimodal(&window) {
                    n[x] = near_c;
                    f[x] = far_c;
                    a[x] = share;
                }
            }
            (a, n, f)
        })
        .collect();
    for (y, (a, n, f)) in rows.into_iter().enumerate() {
        alpha[y * w..(y + 1) * w].copy_from_slice(&a);
        near[y * w..(y + 1) * w].copy_from_slice(&n);
        far[y * w..(y + 1) * w].copy_from_slice(&f);
    }
    EdgeMaps { w, h, alpha, near, far }
}

/// The local mean of `control` over the window, counting only pixels whose `mask` is set (its own
/// values as weights), used for the subject matte's two sides.
fn masked_mean(control: &[f32], mask: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    use rayon::prelude::*;
    // A box sum of both, then their ratio: O(1) a pixel with a summed-area table per row band.
    let sums = |src: &[f32]| -> Vec<f64> {
        // Prefix sums over rows, then per column over the window's rows (cheap enough at 2048).
        let mut row = vec![0.0f64; (w + 1) * h];
        for y in 0..h {
            for x in 0..w {
                row[y * (w + 1) + x + 1] = row[y * (w + 1) + x] + src[y * w + x] as f64;
            }
        }
        let mut out = vec![0.0f64; w * h];
        let w1 = w as isize + 1;
        let rows: Vec<Vec<f64>> = (0..h)
            .into_par_iter()
            .map(|y| {
                let y0 = (y as isize - r as isize).clamp(0, h as isize - 1) as usize;
                let y1 = (y as isize + r as isize).clamp(0, h as isize - 1) as usize;
                (0..w)
                    .map(|x| {
                        let x0 = (x as isize - r as isize).clamp(0, w as isize - 1) as usize;
                        let x1 = (x as isize + r as isize).clamp(0, w as isize - 1) as usize;
                        let mut total = 0.0;
                        for yy in y0..=y1 {
                            let base = (yy as isize * w1) as usize;
                            total += row[base + x1 + 1] - row[base + x0];
                        }
                        total
                    })
                    .collect()
            })
            .collect();
        for (y, line) in rows.into_iter().enumerate() {
            out[y * w..(y + 1) * w].copy_from_slice(&line);
        }
        out
    };
    let (num, den) = (sums(&(0..w * h).map(|i| control[i] * mask[i]).collect::<Vec<f32>>()), sums(mask));
    (0..w * h).map(|i| if den[i] > 1e-6 { (num[i] / den[i]) as f32 } else { f32::NAN }).collect()
}

/// The edge maps of a refined control map, guided by the photo's lightness where the subject
/// matte has nothing to say.
///
/// `guide` is the photo at the same size (perceptual lightness, 0..1-ish), `subject` its matte at
/// the same size when one exists. Where the matte is part way (its outline), alpha is the matte
/// and the two surfaces are the subject's own depth and the background's, measured a little either
/// side of the outline; elsewhere the depth map's own bimodal windows decide.
pub fn edges(control: &Grid, guide: Option<&Grid>, subject: Option<&Grid>, radius: usize) -> EdgeMaps {
    let r = radius.max(1);
    let raw = detect(control, r);
    let mut out = raw.clone();
    // The coverage, following the photo's own edge: the guide is the picture's lightness, the
    // input the raw share; then clamped to the coverage the window's own edges hold.
    if let Some(guide) = guide.filter(|g| g.w == control.w && g.h == control.h) {
        let filtered = guided_filter(&guide.data, &raw.alpha, control.w, control.h, (r * 2).max(2), 1e-3);
        let edge_lo: Vec<f32> = raw.alpha.iter().map(|a| if *a > 0.01 && *a < 0.99 { *a } else { 1.0 }).collect();
        let edge_hi: Vec<f32> = raw.alpha.iter().map(|a| if *a > 0.01 && *a < 0.99 { *a } else { 0.0 }).collect();
        let lo = min_filter(&edge_lo, control.w, control.h, r);
        let negated: Vec<f32> = edge_hi.iter().map(|v| -v).collect();
        let hi: Vec<f32> = min_filter(&negated, control.w, control.h, r).into_iter().map(|v| -v).collect();
        out.alpha = filtered
            .iter()
            .zip(lo.iter().zip(hi.iter()))
            .map(|(&v, (&lo, &hi))| if lo <= hi { v.clamp(lo, hi) } else { v })
            .collect();
    }
    // The subject's outline, where there is one: the matte is the coverage, so hair keeps.
    if let Some(matte) = subject.filter(|m| m.w == control.w && m.h == control.h) {
        let inside: Vec<f32> = matte.data.iter().map(|m| if *m > 0.9 { 1.0 } else { 0.0 }).collect();
        let outside: Vec<f32> = matte.data.iter().map(|m| if *m < 0.1 { 1.0 } else { 0.0 }).collect();
        let subject_z = masked_mean(&control.data, &inside, control.w, control.h, r.max(4) * 2);
        let back_z = masked_mean(&control.data, &outside, control.w, control.h, r.max(4) * 2);
        for i in 0..out.alpha.len() {
            let m = matte.data[i].clamp(0.0, 1.0);
            if !(0.02..=0.98).contains(&m) {
                continue;
            }
            let (a, b) = (subject_z[i], back_z[i]);
            if !(a.is_finite() && b.is_finite()) {
                continue;
            }
            // The nearer of the two is the near surface, whichever side of the matte it is on.
            if a >= b {
                out.near[i] = a;
                out.far[i] = b;
                out.alpha[i] = m;
            } else {
                out.near[i] = b;
                out.far[i] = a;
                out.alpha[i] = 1.0 - m;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A control map with a ramp across it (left half) and a step down the middle (right half).
    fn ramp_and_step(w: usize, h: usize) -> Grid {
        let mut data = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = if x < w / 2 {
                    // A ramp: 0.3 to 0.7 across the left half, the same in every row.
                    0.3 + 0.4 * x as f32 / (w / 2) as f32
                } else if y < h / 2 {
                    0.3
                } else {
                    0.75
                };
            }
        }
        Grid { w, h, data }
    }

    #[test]
    fn a_ramp_is_not_an_edge_but_a_step_is() {
        let (w, h) = (80usize, 60usize);
        let g = ramp_and_step(w, h);
        let e = detect(&g, 2);
        // On the step (right half, just below the middle): both surfaces, and the values either side.
        let i = (h / 2 + 1) * w + w / 2 + 10;
        assert!(e.alpha[i] > 0.01 && e.alpha[i] < 0.99, "alpha {}", e.alpha[i]);
        assert!((e.near[i] - 0.75).abs() < 1e-3, "near {}", e.near[i]);
        assert!((e.far[i] - 0.3).abs() < 1e-3, "far {}", e.far[i]);
        // In the ramp there is no edge: both surfaces are the pixel's own depth.
        let j = 10 * w + 15;
        assert!((e.alpha[j] - 1.0).abs() < 1e-6);
        assert!((e.near[j] - g.data[j]).abs() < 1e-6 && (e.far[j] - g.data[j]).abs() < 1e-6);
        // The flat parts away from the step are flat too (only the step's own band is an edge).
        let k = (h - 2) * w + w / 2 + 30;
        assert!((e.near[k] - 0.75).abs() < 1e-3 && (e.far[k] - 0.75).abs() < 1e-3);
    }

    #[test]
    fn a_single_lone_value_is_not_an_edge() {
        // One odd pixel in a flat field: the gap is big but its share is far below MIN_SHARE.
        let (w, h) = (40usize, 40usize);
        let mut data = vec![0.4f32; w * h];
        data[20 * w + 20] = 0.9;
        assert!(bimodal(&{
            let mut v = vec![0.4f32; 49];
            v[48] = 0.9;
            v
        })
        .is_none());
        let e = detect(&Grid { w, h, data }, 3);
        assert!(e.edge_share() < 0.05, "{}", e.edge_share());
    }

    #[test]
    fn the_band_blends_two_surfaces_by_their_coverage() {
        // A pixel wholly near, one halfway, one wholly far, each with the same two surfaces.
        let e = EdgeMaps { w: 3, h: 1, alpha: vec![1.0, 0.5, 0.0], near: vec![0.8; 3], far: vec![0.2; 3] };
        // A band that holds the near surface only (0.6..0.9): 1, half, 0.
        let band = e.band(0.9, 0.6, 0.0);
        assert!((band.data[0] - 1.0).abs() < 1e-6);
        assert!((band.data[1] - 0.5).abs() < 1e-6);
        assert!(band.data[2].abs() < 1e-6);
        // A band that holds the far surface only (0.0..0.3): 0, half, 1.
        let band = e.band(0.3, 0.0, 0.0);
        assert!(band.data[0].abs() < 1e-6 && (band.data[1] - 0.5).abs() < 1e-6 && (band.data[2] - 1.0).abs() < 1e-6);
        // The whole range holds both: everything is in the mask, whatever the coverage.
        let band = e.band(1.0, 0.0, 0.0);
        assert!(band.data.iter().all(|v| (v - 1.0).abs() < 1e-6));
        // A flat map is its own band: the formula changes nothing away from edges.
        let control = Grid { w: 3, h: 1, data: vec![0.1, 0.5, 0.9] };
        let flat = EdgeMaps::flat(&control);
        let band = flat.band(0.7, 0.3, 0.0);
        assert!((band.data[0] - 0.0).abs() < 1e-6 && (band.data[1] - 1.0).abs() < 1e-6 && (band.data[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn the_subject_matte_is_the_coverage_at_its_outline() {
        // A subject block (depth 0.8) on a background (0.2), its matte soft over two columns.
        let (w, h) = (60usize, 40usize);
        let mut control = vec![0.2f32; w * h];
        let mut matte = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                if (10..30).contains(&x) {
                    control[y * w + x] = 0.8;
                }
                matte[y * w + x] = match x {
                    28 => 0.75,
                    29 => 0.25,
                    10..=27 => 1.0,
                    _ => 0.0,
                };
            }
        }
        let e = edges(&Grid { w, h, data: control }, None, Some(&Grid { w, h, data: matte }), 2);
        // At the soft outline the matte is the coverage, and the surfaces are the two depths.
        let at = 20 * w + 28;
        assert!((e.alpha[at] - 0.75).abs() < 1e-3, "alpha {}", e.alpha[at]);
        assert!((e.near[at] - 0.8).abs() < 1e-2 && (e.far[at] - 0.2).abs() < 1e-2);
        let at = 20 * w + 29;
        assert!((e.alpha[at] - 0.25).abs() < 1e-3, "alpha {}", e.alpha[at]);
        // A band holding the subject only reads 1 / 0.75 / 0.25 / 0 across the outline.
        let band = e.band(0.9, 0.5, 0.0);
        assert!((band.data[20 * w + 27] - 1.0).abs() < 1e-6);
        assert!((band.data[20 * w + 28] - 0.75).abs() < 1e-3);
        assert!((band.data[20 * w + 29] - 0.25).abs() < 1e-3);
        // Well outside, away from the outline's band, the band holds nothing.
        assert!((band.data[20 * w + 40]).abs() < 1e-6);
        // Inside, the matte is 1 and the pixel is wholly its own depth.
        assert!((e.alpha[20 * w + 15] - 1.0).abs() < 1e-6);
    }
}
