//! Fitting a relative depth model to the scene's metric distances, and blending higher-resolution
//! tiles into the global map (phase 1c): the optional High quality model (Depth Anything 3
//! Mono-Large, `hq.rs`) and the tiled pass both end up here.
//!
//! A relative model gives depth up to an affine transform. Its output is fitted to MoGe's metric
//! distances `z_moge ≈ a·z_model + b` over the pixels both call real geometry and not sky, with a
//! robust fit (iteratively reweighted least squares, Huber with k = 1.345 and a MAD scale), so a
//! wrong pixel or a speck of sky can't tilt it. Depth models are affine in inverse depth rather
//! than in metres (a disparity map is linear in 1/z), so both spaces are fitted and the one with
//! the lower median relative error in metres is kept: `z ≈ a·z_model + b` in log space, or
//! `1/z ≈ a/z_model + b` in inverse depth. If neither fits, quantile matching (the model's 10% and
//! 90% quantiles mapped onto MoGe's) is the fallback.
//!
//! Tiles: the photo is covered with overlapping square windows at the model's own input size (so
//! each window is seen at full resolution, not squeezed into the global pass's grid). Each window's
//! output is least-squares fitted to the global map over that window, then blended in with
//! raised-cosine weights: detail from the tiles, layout from the global pass, and no seam where one
//! ends. PatchFusion (MIT) is the reference for the idea; this is our own implementation.
//!
//! Everything here is plain maths over a `Grid`, so the tests below run without any model.

use crate::gradient::Grid;

/// The space a fit is made in: log distance, or inverse distance (what a disparity map is linear in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Log,
    Inverse,
}

impl Space {
    /// A distance as the fit's own value.
    fn forward(self, z: f64) -> f64 {
        match self {
            Space::Log => z.ln(),
            Space::Inverse => 1.0 / z,
        }
    }

    /// The fit's value back as a distance.
    fn back(self, t: f64) -> f64 {
        match self {
            Space::Log => t.exp(),
            Space::Inverse => 1.0 / t,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Space::Log => "log",
            Space::Inverse => "inverse depth",
        }
    }

    /// The distance a model value stands for under a fitted `a` and `b` (`t = a·x + b`).
    pub fn metres_of(self, a: f64, b: f64, model: f32) -> f64 {
        let x = model as f64;
        if !(x > 0.0) || !x.is_finite() {
            return f64::INFINITY;
        }
        let z = self.back(a * self.forward(x) + b);
        if z.is_finite() && z > 0.0 { z } else { f64::INFINITY }
    }
}

/// `z_moge ≈ a·z_model + b`, in `space`: how a relative model's values become metres.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub space: Space,
    pub a: f64,
    pub b: f64,
    /// The median relative error of the fitted distances, metres (0.05 = 5%).
    pub error: f64,
    /// The median |ln(fitted / true)|: a factor, so it says whether a fit explains the scene at all
    /// (0.5 is a factor of 1.65 either way). Comparable across the two spaces, unlike `error` when
    /// the distances span orders of magnitude.
    pub log_error: f64,
    /// How many pixels were fitted.
    pub samples: usize,
}

impl Fit {
    /// The distance this fit gives for a model value (which must be > 0).
    pub fn metres(&self, model: f32) -> f32 {
        let x = model as f64;
        if !(x > 0.0) || !x.is_finite() {
            return f32::INFINITY;
        }
        let t = self.a * self.space.forward(x) + self.b;
        let z = self.space.back(t);
        if z.is_finite() && z > 0.0 {
            z as f32
        } else {
            f32::INFINITY
        }
    }

    /// A model value's own fitting-space coordinate (for a per-tile least squares).
    pub fn x_of(&self, model: f32) -> f64 {
        self.space.forward(model.max(1e-9) as f64)
    }
}

/// Pairs of (MoGe's distance in metres, the model's own value) that take part in a fit: both
/// finite, positive and not sky.
pub fn pairs_from(distance: &[f32], model: &[f32], sky: &[f32]) -> Vec<(f32, f32)> {
    let mut pairs = Vec::new();
    let step = (distance.len() / 40_000).max(1);
    for i in (0..distance.len().min(model.len()).min(sky.len())).step_by(step) {
        let (z, v) = (distance[i], model[i]);
        if z.is_finite() && z > 0.0 && v.is_finite() && v > 0.0 && sky[i] < 0.5 {
            pairs.push((z, v));
        }
    }
    pairs
}

/// The values sorted (total order, finite only).
fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = values.filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    v
}

/// A quantile of a sorted slice (k-th of a thousand).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[(((sorted.len() - 1) as f64) * q).round() as usize]
}

/// The median of an unsorted set of residuals.
fn median(mut values: Vec<f64>) -> f64 {
    values.retain(|v| v.is_finite());
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// A weighted least squares line `y = a·x + b` (None: no spread in x).
fn line(x: &[f64], y: &[f64], w: &[f64]) -> Option<(f64, f64)> {
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..x.len() {
        sw += w[i];
        sx += w[i] * x[i];
        sy += w[i] * y[i];
        sxx += w[i] * x[i] * x[i];
        sxy += w[i] * x[i] * y[i];
    }
    if sw <= 0.0 {
        return None;
    }
    let mx = sx / sw;
    let my = sy / sw;
    let var = sxx / sw - mx * mx;
    if var.abs() < 1e-12 {
        return None;
    }
    let a = (sxy / sw - mx * my) / var;
    Some((a, my - a * mx))
}

/// The median relative error of `a`, `b` in `space` over the pairs, metres, and the median log
/// ratio (a factor, the same in either space).
fn relative_error(space: Space, a: f64, b: f64, pairs: &[(f32, f32)]) -> (f64, f64) {
    let mut linear: Vec<f64> = Vec::with_capacity(pairs.len());
    let mut log: Vec<f64> = Vec::with_capacity(pairs.len());
    for &(z, v) in pairs {
        let fitted = space.back(a * space.forward(v as f64) + b);
        if fitted.is_finite() && fitted > 0.0 {
            linear.push(((fitted - z as f64) / z as f64).abs());
            log.push((fitted / z as f64).ln().abs());
        }
    }
    let median = |mut v: Vec<f64>| {
        v.retain(|e| e.is_finite());
        v.sort_by(f64::total_cmp);
        if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
    };
    (median(linear), median(log))
}

/// A robust fit in one space: Huber IRLS (k = 1.345, MAD scale) on the pairs' transformed values.
pub fn fit_space(pairs: &[(f32, f32)], space: Space) -> Option<Fit> {
    if pairs.len() < 8 {
        return None;
    }
    let x: Vec<f64> = pairs.iter().map(|&(_, v)| space.forward(v as f64)).collect();
    let y: Vec<f64> = pairs.iter().map(|&(z, _)| space.forward(z as f64)).collect();
    if !x.iter().chain(y.iter()).all(|v| v.is_finite()) {
        return None;
    }
    let mut w = vec![1.0f64; x.len()];
    let (mut a, mut b) = line(&x, &y, &w)?;
    if !(a.is_finite() && b.is_finite()) {
        return None;
    }
    const K: f64 = 1.345;
    for _ in 0..24 {
        let residuals: Vec<f64> = (0..x.len()).map(|i| y[i] - (a * x[i] + b)).collect();
        let scale = 1.4826 * median(residuals.iter().map(|r| r.abs()).collect());
        if !(scale > 0.0) || !scale.is_finite() {
            break;
        }
        let mut moved = 0.0f64;
        for i in 0..x.len() {
            let r = residuals[i].abs();
            let wi = if r <= K * scale { 1.0 } else { K * scale / r };
            moved = moved.max((wi - w[i]).abs());
            w[i] = wi;
        }
        match line(&x, &y, &w) {
            Some((na, nb)) if na.is_finite() && nb.is_finite() => {
                a = na;
                b = nb;
            }
            _ => break,
        }
        if moved < 1e-6 {
            break;
        }
    }
    // Only a rising fit makes sense (a nearer model value must mean a nearer distance).
    if !(a > 0.0) {
        return None;
    }
    let (error, log_error) = relative_error(space, a, b, pairs);
    Some(Fit { space, a, b, error, log_error, samples: pairs.len() })
}

/// Quantile matching, the fallback when a fit fails: the model's 10% and 90% quantiles mapped
/// straight onto MoGe's (in inverse depth, so the ends are distances rather than logs).
pub fn quantile_fit(pairs: &[(f32, f32)]) -> Option<Fit> {
    if pairs.len() < 8 {
        return None;
    }
    let space = Space::Inverse;
    let xs = sorted(pairs.iter().map(|&(_, v)| space.forward(v as f64)));
    let ys = sorted(pairs.iter().map(|&(z, _)| space.forward(z as f64)));
    let (x0, x1, y0, y1) = (quantile(&xs, 0.1), quantile(&xs, 0.9), quantile(&ys, 0.1), quantile(&ys, 0.9));
    if !(x1 - x0).abs().gt(&1e-12) {
        return None;
    }
    let a = (y1 - y0) / (x1 - x0);
    let b = y0 - a * x0;
    if !(a > 0.0) || !a.is_finite() || !b.is_finite() {
        return None;
    }
    let (error, log_error) = relative_error(space, a, b, pairs);
    Some(Fit { space, a, b, error, log_error, samples: pairs.len() })
}

/// The better of the two spaces (the lower median relative error), with the quantile fallback if
/// neither fits. None: nothing to fit (no pairs).
pub fn fit_best(pairs: &[(f32, f32)]) -> Option<Fit> {
    let log = fit_space(pairs, Space::Log);
    let inv = fit_space(pairs, Space::Inverse);
    let mut best = match (log, inv) {
        (Some(l), Some(i)) => {
            if l.error.is_finite() && (!i.error.is_finite() || l.error <= i.error) {
                Some(l)
            } else {
                Some(i)
            }
        }
        (Some(l), None) => Some(l),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    };
    // A fit that misses the median pixel by more than a factor of 1.65 is not explaining the data
    // (a different model, or a scene the two disagree about): the quantile match at least maps the
    // two ranges onto each other, so it's the one kept then.
    if best.is_none_or(|f| !f.log_error.is_finite() || f.log_error > 0.5) {
        if let Some(q) = quantile_fit(pairs) {
            best = Some(q);
        }
    }
    best
}

/// A plain (unweighted) least squares in `space`, for one tile's scale and shift: the whole tile
/// is usually one surface, and a tile whose fit is not rising is dropped by the caller.
pub fn fit_plain(pairs: &[(f32, f32)], space: Space) -> Option<(f64, f64)> {
    if pairs.len() < 8 {
        return None;
    }
    let x: Vec<f64> = pairs.iter().map(|&(_, v)| space.forward(v as f64)).collect();
    let y: Vec<f64> = pairs.iter().map(|&(z, _)| space.forward(z as f64)).collect();
    let w = vec![1.0f64; x.len()];
    let (a, b) = line(&x, &y, &w)?;
    (a.is_finite() && b.is_finite() && a > 0.0).then_some((a, b))
}

// --- Tiles ---------------------------------------------------------------------------------------

/// A window of the frame, in pixels of whatever grid it's applied to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}

/// The windows to run over a `w` x `h` image whose model sees `side` pixels square, at most `max`
/// of them, about 50% overlap, spread evenly over the frame (as even a grid of them as the count
/// allows, so the budget is spent over the whole picture rather than one corner). Empty: the
/// picture is barely bigger than one window (the global pass already has that detail), or no budget.
pub fn plan(w: usize, h: usize, side: usize, max: usize) -> Vec<Tile> {
    let side = side.max(16);
    if w == 0 || h == 0 || max == 0 || (w <= side + side / 4 && h <= side + side / 4) {
        return Vec::new();
    }
    let tw = side.min(w);
    let th = side.min(h);
    // How many windows at a 50% step would cover each side.
    let need = |len: usize, tile: usize| -> usize {
        if tile >= len {
            1
        } else {
            (len - tile).div_ceil((tile / 2).max(1)) + 1
        }
    };
    let (need_x, need_y) = (need(w, tw), need(h, th));
    // Scaled down evenly to the budget (so the windows still spread both ways), then whatever is
    // left of the budget goes on the longer side.
    let (mut nx, mut ny) = if need_x * need_y <= max {
        (need_x, need_y)
    } else {
        let f = (max as f32 / (need_x * need_y) as f32).sqrt();
        (((need_x as f32 * f).round() as usize).max(1), ((need_y as f32 * f).round() as usize).max(1))
    };
    while nx * ny > max {
        if nx >= ny && nx > 1 {
            nx -= 1;
        } else if ny > 1 {
            ny -= 1;
        } else {
            break;
        }
    }
    while nx * ny < max {
        let across = w as f32 / tw as f32;
        let down = h as f32 / th as f32;
        if across >= down {
            if nx == need_x {
                break;
            }
            nx += 1;
        } else {
            if ny == need_y {
                break;
            }
            ny += 1;
        }
    }
    let place = |n: usize, len: usize, tile: usize| -> Vec<usize> {
        if n <= 1 || tile >= len {
            vec![(len.saturating_sub(tile)) / 2]
        } else {
            (0..n).map(|i| (i * (len - tile)) / (n - 1)).collect()
        }
    };
    let xs = place(nx, w, tw);
    let ys = place(ny, h, th);
    let mut tiles = Vec::with_capacity(xs.len() * ys.len());
    for &y0 in &ys {
        for &x0 in &xs {
            tiles.push(Tile { x0, y0, w: tw, h: th });
        }
    }
    tiles
}

/// A raised-cosine weight (0 at the window's edges, 1 in its middle), for one axis.
fn raised(i: usize, n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / n as f32).cos()
}

/// Blends windows into the global map: each window's own grid (already fitted to the global) is
/// weighted by a raised cosine so its middle counts most and its edges fade to their neighbour,
/// and the global keeps the weight the windows leave (`1 - the strongest window`) so nothing is
/// double counted. Everywhere no window reaches, the global is untouched.
pub fn blend(global: &Grid, parts: &[(Tile, &Grid)]) -> Grid {
    let (w, h) = (global.w, global.h);
    let mut data = global.data.clone();
    // The strongest window weight at each pixel.
    let mut cover = vec![0.0f32; w * h];
    for (tile, _) in parts {
        let (x1, y1) = ((tile.x0 + tile.w).min(w), (tile.y0 + tile.h).min(h));
        for y in tile.y0..y1 {
            for x in tile.x0..x1 {
                let wt = raised(x - tile.x0, tile.w) * raised(y - tile.y0, tile.h);
                let at = y * w + x;
                if wt > cover[at] {
                    cover[at] = wt;
                }
            }
        }
    }
    // Sum the windows in, then normalise by their total weight (the global's own share is what the
    // strongest window leaves).
    let mut sum = vec![0.0f32; w * h];
    let mut total: Vec<f32> = cover.iter().map(|c| 1.0 - c).collect();
    for (tile, grid) in parts {
        let (x1, y1) = ((tile.x0 + tile.w).min(w), (tile.y0 + tile.h).min(h));
        for y in tile.y0..y1 {
            for x in tile.x0..x1 {
                let wt = raised(x - tile.x0, tile.w) * raised(y - tile.y0, tile.h);
                if wt <= 0.0 {
                    continue;
                }
                let (sx, sy) = ((x - tile.x0).min(grid.w - 1), (y - tile.y0).min(grid.h - 1));
                let v = grid.data[sy * grid.w + sx];
                if !v.is_finite() {
                    continue;
                }
                let at = y * w + x;
                sum[at] += wt * v;
                total[at] += wt;
            }
        }
    }
    for i in 0..data.len() {
        let t = total[i];
        if t > 1e-6 {
            data[i] = (global.data[i] * (1.0 - cover[i]) + sum[i]) / t.max(1e-6);
        }
    }
    Grid { w, h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic pair set: `z_moge = 1 / (a/z + b)` in inverse depth (and its log equivalent),
    /// with a few gross outliers.
    fn pairs_inverse(n: usize, a: f64, b: f64) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let z_model = 0.5 + 3.5 * i as f64 / n as f64;
                let t = a / z_model + b;
                ((1.0 / t) as f32, z_model as f32)
            })
            .collect()
    }

    #[test]
    fn a_robust_fit_recovers_the_affine_relation_and_ignores_outliers() {
        let mut pairs = pairs_inverse(2000, 4.0, 0.15);
        // 5% of the pixels are nonsense: a streak of sky read as very near, or a clipped pixel.
        for i in (0..pairs.len()).step_by(20) {
            pairs[i] = (pairs[i].0 * 0.05, pairs[i].1 * 12.0);
        }
        let fit = fit_space(&pairs, Space::Inverse).expect("a fit");
        assert!((fit.a - 4.0).abs() < 0.05, "a = {} (want 4)", fit.a);
        assert!((fit.b - 0.15).abs() < 0.02, "b = {} (want 0.15)", fit.b);
        // Every fitted distance is within a few percent of the truth (the outliers aside).
        for (i, &(z, v)) in pairs.iter().enumerate().step_by(97) {
            if i % 20 == 0 {
                continue;
            }
            let got = fit.metres(v) as f64;
            assert!(((got - z as f64) / z as f64).abs() < 0.05, "{got} vs {z}");
        }
    }

    #[test]
    fn the_better_space_is_kept_and_compared_in_metres() {
        // A model affine in inverse depth: that space fits far better than the log one.
        let pairs = pairs_inverse(1500, 3.0, 0.1);
        let fit = fit_best(&pairs).expect("a fit");
        assert_eq!(fit.space, Space::Inverse);
        assert!(fit.error < 1e-3, "{}", fit.error);
        let log = fit_space(&pairs, Space::Log).expect("a log fit");
        assert!(log.error > fit.error, "log {} vs inverse {}", log.error, fit.error);
        // And a model affine in log distance: the log space wins.
        let log_pairs: Vec<(f32, f32)> = (0..1500)
            .map(|i| {
                let z = 1.0 + 40.0 * i as f64 / 1500.0;
                (z as f32, (z / 2.0).powf(1.0 / 1.3) as f32)
            })
            .collect();
        let fit = fit_best(&log_pairs).expect("a fit");
        assert_eq!(fit.space, Space::Log);
        assert!(fit.error < 1e-3, "{}", fit.error);
    }

    #[test]
    fn a_fit_that_explains_nothing_falls_back_to_quantile_matching() {
        // The two models' values are scrambled against each other: a line through them explains
        // almost nothing, so the quantile match (the two ranges mapped onto each other) is used.
        let pairs: Vec<(f32, f32)> = (0..600).map(|i| (((i * 137) % 500) as f32 + 0.5, (i % 500) as f32 + 0.5)).collect();
        let fit = fit_best(&pairs).expect("the fallback still gives something");
        assert_eq!(fit.space, Space::Inverse, "quantile matching is in inverse depth");
        assert!(fit.a > 0.0 && fit.a.is_finite() && fit.b.is_finite());
        // The quantile fallback matches the two models' 10% and 90% ends exactly.
        let xs = sorted(pairs.iter().map(|&(_, v)| 1.0 / v as f64));
        let ys = sorted(pairs.iter().map(|&(z, _)| 1.0 / z as f64));
        let (x0, x1, y0, y1) = (quantile(&xs, 0.1), quantile(&xs, 0.9), quantile(&ys, 0.1), quantile(&ys, 0.9));
        let metres = |x: f64| 1.0 / (fit.a * x + fit.b);
        assert!((metres(x0) - 1.0 / y0).abs() < 1e-3);
        assert!((metres(x1) - 1.0 / y1).abs() < 1e-3);
    }

    #[test]
    fn too_few_pairs_fit_nothing() {
        assert!(fit_best(&[]).is_none());
        assert!(fit_best(&[(1.0, 1.0), (2.0, 2.0)]).is_none());
    }

    #[test]
    fn tiles_cover_the_frame_evenly_within_the_budget() {
        // A 6000 x 4000 picture, 1036 px windows, 3 of them: a row across the width.
        let t = plan(6000, 4000, 1036, 3);
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.w == 1036 && t.h == 1036));
        assert_eq!(t[0].x0, 0);
        assert_eq!(t[2].x0, 6000 - 1036);
        assert_eq!(t[1].x0, (6000 - 1036) / 2);
        // All the same row: the budget is spent across the picture.
        assert!(t.iter().all(|t| t.y0 == (4000 - 1036) / 2));
        // A bigger budget spreads in both directions, and never exceeds it.
        let t = plan(6000, 4000, 1036, 12);
        assert!(t.len() <= 12 && t.len() >= 6, "{}", t.len());
        assert!(t.iter().any(|a| t.iter().any(|b| a.y0 != b.y0)), "two rows");
        // The windows stay inside the frame.
        for tile in &t {
            assert!(tile.x0 + tile.w <= 6000 && tile.y0 + tile.h <= 4000);
        }
        // A picture barely bigger than one window (or a tiny budget) gets none.
        assert!(plan(1100, 1100, 1036, 3).is_empty());
        assert!(plan(6000, 4000, 1036, 0).is_empty());
        // A tile can't be bigger than a narrow picture.
        let t = plan(800, 6000, 1036, 2);
        assert!(t.iter().all(|t| t.w <= 800 && t.h == 1036));
    }

    /// A smooth field like a control map: a log ramp across the frame with a soft bump.
    fn smooth(w: usize, h: usize) -> Grid {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                data.push(0.2 + 0.6 * u + 0.1 * (3.0 * v).sin() + 0.05 * (6.0 * u).cos());
            }
        }
        Grid { w, h, data }
    }

    #[test]
    fn blending_perfect_tiles_reproduces_the_global_field_and_keeps_it_smooth() {
        let (w, h) = (240, 160);
        let global = smooth(w, h);
        // Windows whose own values are exactly the global's: the blend must return it unchanged.
        let tiles = plan(w, h, 96, 6);
        assert!(tiles.len() >= 2);
        let parts: Vec<(Tile, Grid)> = tiles
            .iter()
            .map(|&t| {
                let mut data = Vec::with_capacity(t.w * t.h);
                for y in 0..t.h {
                    for x in 0..t.w {
                        data.push(global.data[(t.y0 + y) * w + t.x0 + x]);
                    }
                }
                (t, Grid { w: t.w, h: t.h, data })
            })
            .collect();
        let out = blend(&global, &parts.iter().map(|(t, g)| (*t, g)).collect::<Vec<_>>());
        let worst = out.data.iter().zip(&global.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "perfect tiles change nothing: {worst}");

        // A tile that holds real extra detail (a hair-thin step) shows it, fading at its edges,
        // and never changes anything outside its own window.
        let t = tiles[0];
        let mut detail = parts[0].1.clone();
        for y in 0..t.h {
            for x in 0..t.w {
                if x > t.w / 2 {
                    detail.data[y * t.w + x] += 0.08;
                }
            }
        }
        let mut parts2: Vec<(Tile, Grid)> = parts.clone();
        parts2[0] = (t, detail);
        let out = blend(&global, &parts2.iter().map(|(t, g)| (*t, g)).collect::<Vec<_>>());
        // Inside the tile's middle right, the extra 0.08 is mostly there.
        let mid = (t.y0 + t.h / 2) * w + t.x0 + t.w * 3 / 4;
        assert!(out.data[mid] - global.data[mid] > 0.03, "{}", out.data[mid] - global.data[mid]);
        // Nothing outside every window moved.
        let tileset: Vec<Tile> = tiles.clone();
        for y in 0..h {
            for x in 0..w {
                if tileset.iter().any(|t| x >= t.x0 && x < t.x0 + t.w && y >= t.y0 && y < t.y0 + t.h) {
                    continue;
                }
                let at = y * w + x;
                assert!((out.data[at] - global.data[at]).abs() < 1e-6, "({x},{y}) moved");
            }
        }
    }
}
