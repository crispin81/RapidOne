//! Lens-aware Bokeh: a thin-lens circle of confusion from the photo's metric depth, rendered with
//! a layered, occlusion-aware composite (our own code; Dr.Bokeh's paper and BokehMe's ideas only).
//!
//! The scene light goes in before the tone curve, so the blur is a light-space convolution and
//! bright highlights keep their energy. Distances come from `scene::Refined`'s control depth
//! (log distance between the scene's anchors) and its two surfaces at edges: every pixel's light
//! is split between its near and far surface by the edge coverage, so a hair line doesn't halo.
//!
//! Layers: one focus slab and six buckets each side of focus, each blurred at its own radius,
//! composited front to back with "over" and renormalised by the remaining coverage. Colour and
//! coverage are blurred together premultiplied (normalised convolution), so at a sharp edge the
//! blur gathers the light that is actually visible there, and the disocclusion behind an occluder
//! comes from the background's own surrounding pixels: no halo.

use crate::creative::{Bokeh, BokehMode, BokehShape};
use crate::gradient::Grid;
use crate::lensprofile::LensShot;
use crate::pipeline::{Frame, SceneImage};
use crate::scene::Refined;

/// Layers either side of the focus slab (the brief's ~6 each side; one more covers the soft edge
/// between the last two).
const SIDE_BUCKETS: usize = 7;
/// The focus slab's layer index (the near buckets come before it, the far ones after).
const FOCUS_LAYER: usize = SIDE_BUCKETS + 1;
/// The largest radius the composite works at, in pixels: bigger radii are processed on a smaller
/// copy (the pyramid level) and upsampled.
const MAX_WORK_RADIUS: f32 = 16.0;
/// The blur circle that still counts as sharp, as a share of the sensor's width (0.03 mm on full frame).
const SHARP_SHARE: f32 = 1.0 / 1200.0;
/// The most the blur may spread, as a fraction of the picture's long side (the brief's 4%).
pub const MAX_RADIUS_SHARE: f32 = 0.04;

/// The lens facts the maths needs, from a photo's EXIF (missing facts fall back sensibly).
#[derive(Clone, Copy, Debug)]
pub struct LensFacts {
    pub focal_mm: f32,
    pub f_number: f32,
    pub shot_focal_mm: f32,
    pub shot_f_number: f32,
    pub sensor_width_mm: f32,
    pub exif_focus_m: Option<f32>,
}

impl LensFacts {
    pub fn from_shot(shot: Option<&LensShot>) -> LensFacts {
        let focal = shot.map(|s| s.focal).filter(|f| *f > 0.0).unwrap_or(50.0);
        let f = shot.map(|s| s.fnumber).filter(|n| *n > 0.0).unwrap_or(4.0);
        let sensor = shot.and_then(|s| s.sensor_width_mm).filter(|w| *w > 1.0).unwrap_or(36.0);
        LensFacts {
            focal_mm: focal,
            f_number: f,
            shot_focal_mm: focal,
            shot_f_number: f,
            sensor_width_mm: sensor,
            exif_focus_m: shot.and_then(|s| s.focus_distance_m).filter(|d| *d > 0.0),
        }
    }
}

/// A thin-lens blurred diameter in sensor millimetres, signed: negative in front of the focus
/// distance `s`, positive behind it. `f` is the focal length and `n` the f-number (both mm / plain),
/// `s` and `z` are metres. This is the standard thin-lens circle of confusion
/// `c = f²/(N(s−f)) · (z−s)/z`.
pub fn coc_sensor_mm(f_mm: f32, n: f32, s_m: f32, z_m: f32) -> f32 {
    let (f, s, z) = (f_mm / 1000.0, s_m.max(1e-3), z_m.max(1e-3));
    if !(f > 0.0 && n > 0.0) || (s - f).abs() < 1e-6 {
        return 0.0;
    }
    let base = f * f / (n * (s - f));
    // At infinity c is f²/(N(s−f)): the sign of z−s is positive there.
    let diameter_m = if !z.is_finite() { base } else { base * (z - s) / z };
    diameter_m * 1000.0
}

/// A circle-of-confusion radius in pixels of a `width_px`-wide frame, from the sensor diameter.
pub fn coc_radius_px(diameter_mm: f32, sensor_width_mm: f32, width_px: f32) -> f32 {
    if !(sensor_width_mm > 0.0) {
        return 0.0;
    }
    diameter_mm / sensor_width_mm * width_px / 2.0
}

/// **Open up**: the extra blur only, added in quadrature to the shot's own, so an already-soft
/// background isn't blurred twice. Both radii are unsigned here; the sign comes from `z − s`.
pub fn open_up_radius(f_mm: f32, n_shot: f32, n_target: f32, s_m: f32, z_m: f32, sensor_mm: f32, width_px: f32) -> f32 {
    let r_shot = coc_radius_px(coc_sensor_mm(f_mm, n_shot, s_m, z_m), sensor_mm, width_px);
    let r_target = coc_radius_px(coc_sensor_mm(f_mm, n_target.max(0.5), s_m, z_m), sensor_mm, width_px);
    let sign = if z_m >= s_m { 1.0 } else { -1.0 };
    sign * (r_target * r_target - r_shot * r_shot).max(0.0).sqrt()
}

/// **Simulate another lens**: the same subject size, so the camera steps back: `s_t = s·f_t/f` and
/// the scene moves to `z_t = z + (s_t − s)`. Then the thin lens with the target lens, minus the
/// shot's own blur in quadrature.
pub fn simulate_radius(
    shot_f_mm: f32,
    shot_n: f32,
    f_target_mm: f32,
    n_target: f32,
    s_m: f32,
    z_m: f32,
    sensor_mm: f32,
    width_px: f32,
) -> f32 {
    let ratio = (f_target_mm / shot_f_mm.max(1e-3)).max(0.01);
    let s_t = s_m * ratio;
    let z_t = z_m + (s_t - s_m);
    let r_shot = coc_radius_px(coc_sensor_mm(shot_f_mm, shot_n, s_m, z_m), sensor_mm, width_px);
    let r_target = coc_radius_px(coc_sensor_mm(f_target_mm, n_target.max(0.5), s_t, z_t), sensor_mm, width_px);
    let sign = if z_m >= s_m { 1.0 } else { -1.0 };
    sign * (r_target * r_target - r_shot * r_shot).max(0.0).sqrt()
}

/// The thin lens's depth of field, metres: the nearest and farthest distances whose blur circle is
/// no wider than `c_mm` (infinite past the hyperfocal distance).
pub fn depth_of_field(f_mm: f32, n: f32, s_m: f32, c_mm: f32) -> (f32, f32) {
    let f = f_mm / 1000.0;
    if !(f > 0.0 && n > 0.0 && c_mm > 0.0) || s_m <= f {
        return (s_m, s_m);
    }
    let base = f * f / (n * (s_m - f)) * 1000.0;
    let far = if base > c_mm { s_m * base / (base - c_mm) } else { f32::INFINITY };
    (s_m * base / (base + c_mm), far)
}

/// The settings with the shot's own f-number and focal length where they're 0 (not moved yet).
fn with_shot(bokeh: &Bokeh, facts: &LensFacts) -> Bokeh {
    Bokeh {
        aperture: if bokeh.aperture > 0.0 { bokeh.aperture } else { facts.shot_f_number },
        focal: if bokeh.focal > 0.0 { bokeh.focal } else { facts.shot_focal_mm },
        ..bokeh.clone()
    }
}

/// The zone that renders sharp for a focus at `s_m`, metres: the target lens's depth of field,
/// never deeper than the shot's own (a blur can't be taken away). None in Manual (no lens maths).
pub fn sharp_zone(bokeh: &Bokeh, facts: &LensFacts, s_m: f32) -> Option<(f32, f32)> {
    let bokeh = &with_shot(bokeh, facts);
    let c = facts.sensor_width_mm * SHARP_SHARE;
    let shot = depth_of_field(facts.shot_focal_mm, facts.shot_f_number.max(0.5), s_m, c);
    let target = match bokeh.mode {
        BokehMode::Open => depth_of_field(facts.focal_mm, bokeh.aperture.max(0.5), s_m, c),
        BokehMode::Simulate => {
            // The camera steps back with the lens (`simulate_radius`): its zone in the shot's metres.
            let focal = bokeh.focal.max(1.0);
            let s_t = s_m * (focal / facts.shot_focal_mm.max(1e-3)).max(0.01);
            let (near, far) = depth_of_field(focal, bokeh.aperture.max(0.5), s_t, c);
            ((near - s_t + s_m).max(1e-3), far - s_t + s_m)
        }
        BokehMode::Manual => return None,
    };
    Some((shot.0.max(target.0), shot.1.min(target.1)))
}

/// **Manual**: no lens maths. The CoC grows with how far a point is from the focus plane in inverse
/// distance, normalised by the scene's own largest difference, up to `max_radius` at Amount 100.
pub fn manual_radius(inv_diff: f32, inv_max: f32, amount: f32, max_radius: f32) -> f32 {
    if inv_max <= 1e-9 {
        return 0.0;
    }
    let t = (inv_diff / inv_max).clamp(-1.0, 1.0);
    t * max_radius * (amount / 100.0).clamp(0.0, 1.0)
}

/// A blur kernel: offsets in pixels and normalised weights (they sum to 1, so energy is kept).
#[derive(Clone, Debug)]
pub struct Kernel {
    pub taps: Vec<(i32, i32, f32)>,
    pub radius: f32,
}

/// The aperture's outline at `theta` (radians), as a fraction of `radius`: a circle, or a regular
/// polygon of `blades` sides, with `rounding` 0..1 blending the polygon into the circle.
fn aperture_edge(theta: f32, blades: u32, rounding: f32) -> f32 {
    if blades < 3 || rounding >= 1.0 {
        return 1.0;
    }
    let n = blades as f32;
    let sector = std::f32::consts::TAU / n;
    let a = theta.rem_euclid(sector) - sector / 2.0;
    // A regular polygon's inradius is R·cos(pi/N): the outline's radius at its flat's middle.
    let polygon = (std::f32::consts::PI / n).cos() / a.cos().max(0.15);
    let round = rounding.clamp(0.0, 1.0);
    round + (1.0 - round) * polygon
}

/// The kernel's weight at radius fraction `t` (0 centre, 1 edge) along `theta`, before
/// normalisation: each shape's own radial profile.
fn profile(shape: BokehShape, t: f32, theta: f32, blades: u32, rounding: f32) -> f32 {
    if t > 1.0 {
        return 0.0;
    }
    match shape {
        BokehShape::Smooth => {
            // Apodised: flat inside 80% of the radius, easing to nothing at the edge.
            let e = ((1.0 - t) / 0.2).clamp(0.0, 1.0);
            e * e * (3.0 - 2.0 * e)
        }
        BokehShape::Blades => {
            let edge = aperture_edge(theta, blades.max(3), rounding.clamp(0.0, 1.0) / 100.0);
            if t <= edge * 0.92 {
                1.0
            } else if t <= edge {
                1.0 - (t - edge * 0.92) / (edge * 0.08).max(1e-4)
            } else {
                0.0
            }
        }
        // Soap bubble: a bright rim.
        BokehShape::Bubble => {
            if t > 0.98 {
                0.0
            } else {
                0.35 + 1.9 * t * t * t
            }
        }
        // Mirror lens: an annulus, nothing in the middle.
        BokehShape::Mirror => {
            if (0.55..=0.98).contains(&t) {
                1.0
            } else {
                0.0
            }
        }
        // Swirled: the disc itself; the render adds the tangential stretch (`swirl`).
        BokehShape::Swirl => {
            let e = ((1.0 - t) / 0.2).clamp(0.0, 1.0);
            e * e * (3.0 - 2.0 * e)
        }
    }
}

/// The kernel for `shape` at `radius` pixels: integer taps on a golden-angle spiral (at most 128,
/// as the brief allows), each weighted by its shape and normalised to sum to 1. `fringe_scale`
/// scales the radii per channel (1 = green, the reference).
pub fn kernel(shape: BokehShape, radius: f32, blades: u32, rounding: f32, fringe_scale: f32) -> Kernel {
    let r = (radius * fringe_scale).max(0.0);
    if r < 1.0 {
        return Kernel { taps: vec![(0, 0, 1.0)], radius: r };
    }
    let mut taps = Vec::new();
    // Two rings of fixed directions for small radii, the golden-angle spiral for large ones.
    if r <= 2.5 {
        for dy in -3..=3 {
            for dx in -3..=3 {
                let t = ((dx * dx + dy * dy) as f32).sqrt() / r;
                let w = profile(shape, t, (dy as f32).atan2(dx as f32), blades, rounding);
                if w > 0.0 {
                    taps.push((dx, dy, w));
                }
            }
        }
    } else {
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        let n = (128.0f32).min((r * r * 1.6).max(24.0)) as usize;
        for i in 0..n {
            // sqrt spreads the samples evenly over the disc's area.
            let t = (((i as f32 + 0.5) / n as f32).sqrt()).min(0.999);
            let theta = i as f32 * golden;
            let w = profile(shape, t, theta, blades, rounding);
            if w <= 0.0 {
                continue;
            }
            let x = (t * theta.cos() * r).round() as i32;
            let y = (t * theta.sin() * r).round() as i32;
            taps.push((x, y, w));
        }
    }
    let sum: f32 = taps.iter().map(|t| t.2).sum();
    if sum <= 1e-9 {
        return Kernel { taps: vec![(0, 0, 1.0)], radius: r };
    }
    for t in &mut taps {
        t.2 /= sum;
    }
    Kernel { taps, radius: r }
}

/// A square box blur of one channel, radius `r`, edges clamped: the local mean the Boost gate and
/// the colour fill use.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    use rayon::prelude::*;
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let line = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            let mut sum = 0.0;
            for v in &line[lo..=hi] {
                sum += v;
            }
            row[x] = sum / (hi - lo + 1) as f32;
        }
    });
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            let mut sum = 0.0;
            for yy in lo..=hi {
                sum += tmp[yy * w + x];
            }
            row[x] = sum / (hi - lo + 1) as f32;
        }
    });
    out
}

/// Bilinear sample of a scalar map at pixel coordinates (pixel centres at +0.5), clamped.
fn sample_scalar(data: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let top = data[y0 * w + x0] * (1.0 - tx) + data[y0 * w + x1] * tx;
    let bottom = data[y1 * w + x0] * (1.0 - tx) + data[y1 * w + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

/// Bilinear sample of a colour map, clamped.
fn sample_colour(data: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    std::array::from_fn(|c| {
        let top = data[y0 * w + x0][c] * (1.0 - tx) + data[y0 * w + x1][c] * tx;
        let bottom = data[y1 * w + x0][c] * (1.0 - tx) + data[y1 * w + x1][c] * tx;
        top * (1.0 - ty) + bottom * ty
    })
}

/// Bilinear resize of colour and coverage (both maps at `w` × `h`) by `k` (≤ 1: down).
fn downscale(colour: &[[f32; 3]], cover: &[f32], w: usize, h: usize, nw: usize, nh: usize) -> (Vec<[f32; 3]>, Vec<f32>) {
    if nw == w && nh == h {
        return (colour.to_vec(), cover.to_vec());
    }
    use rayon::prelude::*;
    let mut c = vec![[0.0f32; 3]; nw * nh];
    let mut a = vec![0.0f32; nw * nh];
    c.par_chunks_mut(nw).zip(a.par_chunks_mut(nw)).enumerate().for_each(|(y, (row, arow))| {
        // Box average over the source area each output pixel covers.
        let y0 = y * h / nh;
        let y1 = (((y + 1) * h / nh).max(y0 + 1)).min(h);
        for x in 0..nw {
            let x0 = x * w / nw;
            let x1 = (((x + 1) * w / nw).max(x0 + 1)).min(w);
            let mut acc = [0.0f32; 3];
            let mut ac = 0.0;
            let mut n = 0.0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = yy * w + xx;
                    for ch in 0..3 {
                        acc[ch] += colour[i][ch];
                    }
                    ac += cover[i];
                    n += 1.0;
                }
            }
            row[x] = if n > 0.0 { acc.map(|v| v / n) } else { [0.0; 3] };
            arow[x] = if n > 0.0 { ac / n } else { 0.0 };
        }
    });
    (c, a)
}

/// A blur of one colour map and one coverage map with `kernel`, edges clamped, on all cores.
fn blur(colour: &[[f32; 3]], cover: &[f32], w: usize, h: usize, k: &Kernel) -> (Vec<[f32; 3]>, Vec<f32>) {
    use rayon::prelude::*;
    let mut out = vec![[0.0f32; 3]; w * h];
    let mut outa = vec![0.0f32; w * h];
    out.par_chunks_mut(w).zip(outa.par_chunks_mut(w)).enumerate().for_each(|(y, (row, arow))| {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            let mut ac = 0.0;
            for &(dx, dy, wt) in &k.taps {
                let xx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                let i = yy * w + xx;
                for ch in 0..3 {
                    acc[ch] += colour[i][ch] * wt;
                }
                ac += cover[i] * wt;
            }
            row[x] = acc;
            arow[x] = ac;
        }
    });
    (out, outa)
}

/// The swirl's tangential smear: rotate samples around the frame's centre, more toward the corners,
/// for the Helios-style stretched bokeh. Applied to a layer's blurred colour and coverage.
fn swirl(c: &[[f32; 3]], a: &[f32], w: usize, h: usize, strength: f32) -> (Vec<[f32; 3]>, Vec<f32>) {
    if strength <= 1e-3 {
        return (c.to_vec(), a.to_vec());
    }
    use rayon::prelude::*;
    let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
    let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
    let weights = [0.14f32, 0.2, 0.32, 0.2, 0.14];
    let angles = [-1.0f32, -0.5, 0.0, 0.5, 1.0];
    let mut oc = vec![[0.0f32; 3]; w * h];
    let mut oa = vec![0.0f32; w * h];
    oc.par_chunks_mut(w).zip(oa.par_chunks_mut(w)).enumerate().for_each(|(y, (row, arow))| {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let rf = (dx * dx + dy * dy).sqrt() / max_r;
            let angle = strength * rf * 0.45;
            let mut acc = [0.0f32; 3];
            let mut ac = 0.0;
            for (wt, k) in weights.iter().zip(angles.iter()) {
                let an = angle * k;
                let (s, co) = an.sin_cos();
                let sx = cx + dx * co - dy * s + 0.5;
                let sy = cy + dx * s + dy * co + 0.5;
                let col = sample_colour(c, w, h, sx, sy);
                for ch in 0..3 {
                    acc[ch] += col[ch] * wt;
                }
                ac += sample_scalar(a, w, h, sx, sy) * wt;
            }
            row[x] = acc;
            arow[x] = ac;
        }
    });
    (oc, oa)
}

/// The cat's-eye squeeze: sample a layer's blurred colour and coverage offset toward the frame's
/// centre by an amount that grows with distance from it, as a real aperture's outline does toward
/// the corners. `amount` 0..1.
fn cats_eye(c: &[[f32; 3]], a: &[f32], w: usize, h: usize, amount: f32, radius: f32) -> (Vec<[f32; 3]>, Vec<f32>) {
    if amount <= 1e-3 || radius < 0.5 {
        return (c.to_vec(), a.to_vec());
    }
    use rayon::prelude::*;
    let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
    let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
    let mut oc = vec![[0.0f32; 3]; w * h];
    let mut oa = vec![0.0f32; w * h];
    oc.par_chunks_mut(w).zip(oa.par_chunks_mut(w)).enumerate().for_each(|(y, (row, arow))| {
        for x in 0..w {
            let (dx, dy) = (cx - x as f32, cy - y as f32);
            let dist = (dx * dx + dy * dy).sqrt();
            let rf = dist / max_r;
            let push = amount * rf * radius * 0.55;
            let (ox, oy) = if dist > 1e-3 { (dx / dist * push, dy / dist * push) } else { (0.0, 0.0) };
            let sx = x as f32 + 0.5 + ox;
            let sy = y as f32 + 0.5 + oy;
            row[x] = sample_colour(c, w, h, sx, sy);
            arow[x] = sample_scalar(a, w, h, sx, sy);
        }
    });
    (oc, oa)
}

/// Where the focus is: the control depth (0..1, 1 nearest). The subject's matte's median when
/// `focus_auto` and a matte is there, else the picked point.
pub fn focus_control(depth: &Refined, subject: Option<&Grid>, bokeh: &Bokeh) -> f32 {
    if bokeh.focus_auto {
        if let Some(m) = subject.filter(|m| m.w >= 2 && m.h >= 2) {
            let mut values: Vec<f32> = Vec::new();
            let step = (m.w * m.h / 20_000).max(1);
            for i in (0..m.data.len()).step_by(step) {
                if m.data[i] > 0.5 {
                    let (u, v) = (((i % m.w) as f32 + 0.5) / m.w as f32, ((i / m.w) as f32 + 0.5) / m.h as f32);
                    values.push(depth.control.at(u, v));
                }
            }
            if !values.is_empty() {
                values.sort_by(f32::total_cmp);
                return values[values.len() / 2].clamp(0.0, 1.0);
            }
        }
    }
    depth.control.at(bokeh.focus_u.clamp(0.0, 1.0), bokeh.focus_v.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// The bokeh render: `None` when it is off (so a photo without it stays bit-identical).
///
/// `image` is scene-linear light (after lens corrections, defringe and removals), `frame` where a
/// zoomed-in part sits in the whole photo (the CoC is in the whole photo's pixels, matching the
/// preview and the export), and `exposure` the stops the pipeline will apply afterwards, for the
/// Boost's mid-grey reference.
pub fn apply(
    image: &SceneImage,
    depth: &Refined,
    bokeh: &Bokeh,
    lens: Option<&LensShot>,
    subject: Option<&Grid>,
    frame: Option<&Frame>,
    exposure: f32,
) -> Option<SceneImage> {
    if !bokeh.enabled {
        return None;
    }
    let (w, h) = (image.width, image.height);
    if w < 4 || h < 4 {
        return None;
    }
    let facts = LensFacts::from_shot(lens);
    let bokeh = &with_shot(bokeh, &facts);
    let full_w = frame.map_or(w as f32, |f| f.full_w.max(1.0));
    let full_h = frame.map_or(h as f32, |f| f.full_h.max(1.0));
    let long_px = full_w.max(full_h);
    let max_radius = MAX_RADIUS_SHARE * long_px;
    let (d_near, d_far) = depth.anchors.unwrap_or((1.0, 10.0));
    let metric = depth.anchors.is_some();

    // The focus, and the metric rescale the brief asks for: when EXIF knows the focus distance,
    // the scene's own metres are scaled so the focus point sits at that distance.
    let focus = focus_control(depth, subject, bokeh);
    let focus_z = if metric { crate::scene::metres_of(focus, d_near, d_far) } else { 1.0 };
    let scale = facts.exif_focus_m.filter(|d| *d > 0.0).map(|d| d / focus_z.max(1e-3)).unwrap_or(1.0);
    let s_m = focus_z * scale;
    let range = (bokeh.focus_range / 100.0).clamp(0.0, 1.0);
    let ratio = (d_far / d_near).max(1.001);
    // The in-focus band, in the same units the CoC is worked in: metres when the scene has metric
    // anchors, else control depth.
    let manual_mode = bokeh.mode == BokehMode::Manual || !metric;
    // With a lens, the f-number's own depth of field, and Focus range kept sharp beyond it either side.
    let (band_lo, band_hi) = if metric {
        let (lo, hi) = sharp_zone(bokeh, &facts, s_m).unwrap_or((s_m, s_m));
        let grow = ratio.powf(0.5 * range);
        (lo / grow, hi * grow)
    } else {
        let half = 0.5 * range;
        (focus - half, focus + half)
    };
    let band_width = if band_hi.is_finite() { band_hi - band_lo } else { 2.0 * (s_m - band_lo) };
    let band_soft = (band_width * 0.35).max(if metric { 0.01 } else { 0.002 });
    // CoC at a distance, in full-photo pixels, signed. The sensor's width is the photo's long side,
    // upright or not.
    let radius_of = |z_m: f32| -> f32 {
        let z = if z_m.is_finite() { z_m * scale } else { f32::INFINITY };
        let r = match bokeh.mode {
            BokehMode::Open => open_up_radius(
                facts.focal_mm,
                facts.shot_f_number.max(0.5),
                bokeh.aperture,
                s_m,
                z,
                facts.sensor_width_mm,
                long_px,
            ),
            BokehMode::Simulate => simulate_radius(
                facts.shot_focal_mm,
                facts.shot_f_number.max(0.5),
                bokeh.focal.max(1.0),
                bokeh.aperture,
                s_m,
                z,
                facts.sensor_width_mm,
                long_px,
            ),
            BokehMode::Manual => 0.0,
        };
        // The manual mode (or a scene with no metric anchors) works in inverse-distance space,
        // filled in below once the map's largest difference is known.
        if r.is_finite() {
            r
        } else {
            0.0
        }
    };
    let inv_max = if bokeh.mode == BokehMode::Manual || !metric {
        // The scene's own largest inverse-distance difference from the focus, sampled over the map.
        let step = ((w * h) / 4096).max(1);
        let mut peak = 0.0f32;
        for i in (0..w * h).step_by(step) {
            let (fx, fy) = frame.map_or((((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32), |f| f.frac(i, w));
            let (n, fr, _) = depth.edges.as_ref().map_or((depth.control.at(fx, fy), depth.control.at(fx, fy), 1.0), |e| e.at(fx, fy));
            for c in [n, fr] {
                let z = if metric { crate::scene::metres_of(c, d_near, d_far) * scale } else { (1.0 - c) + 0.01 };
                peak = peak.max((1.0 / z - 1.0 / s_m).abs());
            }
        }
        peak.max(1e-6)
    } else {
        1.0
    };
    let manual_z = |c: f32| if metric { crate::scene::metres_of(c, d_near, d_far) * scale } else { (1.0 - c) + 0.01 };

    // How much the working copy is shrunk so the largest radius is at most MAX_WORK_RADIUS.
    let scene_long = w.max(h) as f32;
    let probe = [0.0f32, 0.25, 0.5, 0.75, 1.0];
    let mut r_max = if manual_mode { MAX_RADIUS_SHARE * long_px * (bokeh.amount / 100.0).clamp(0.0, 1.0) } else { 0.0 };
    for &c in &probe {
        let z = manual_z(c);
        r_max = r_max.max(radius_of(z).abs());
    }
    // The brief's cap: a radius past 4% of the long side is cut, so the largest radius (and with it
    // the working copy's shrink) can't run away on a near foreground.
    r_max = r_max.min(max_radius);
    let mut k = (MAX_WORK_RADIUS / r_max.max(1e-3)).min(1.0);
    // Also keep the working picture around a million pixels for the preview's budget.
    k = k.min((1_000_000.0 / (scene_long * scene_long)).sqrt().min(1.0));
    let (ww, wh) = (((w as f32) * k).round().max(1.0) as usize, ((h as f32) * k).round().max(1.0) as usize);

    // The working colour and coverage; the original colour for the focus slab's full-resolution fill.
    let (mut colour, _cover) = downscale(&image.px, &vec![1.0f32; w * h], w, h, ww, wh);
    // Boost: bright AND standing out from their surroundings gain up to ~3 stops; skies never.
    if bokeh.boost > 1e-3 {
        let gain_exposure = 2f32.powf(exposure);
        let luma: Vec<f32> = colour.iter().map(|c| (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]) * gain_exposure).collect();
        let mean = box_blur(&luma, ww, wh, ((ww.max(wh) / 100).max(1)) as usize);
        let strength = (2f32.powf(3.0 * bokeh.boost / 100.0) - 1.0).max(0.0);
        for i in 0..ww * wh {
            let (fx, fy) = frame.map_or(
                (((i % ww) as f32 + 0.5) / ww as f32, ((i / ww) as f32 + 0.5) / wh as f32),
                |f| {
                    let px = (i % ww) as f32 * w as f32 / ww as f32;
                    let py = (i / ww) as f32 * h as f32 / wh as f32;
                    ((f.x0 + px + 0.5) / f.full_w, (f.y0 + py + 0.5) / f.full_h)
                },
            );
            let sky = depth.sky.as_ref().map_or(0.0, |s| s.at(fx, fy));
            let bright = ((luma[i] - 0.72) / 0.36).clamp(0.0, 1.0);
            let stands = ((luma[i] - mean[i]) / mean[i].max(1e-4) - 1.0).clamp(0.0, 1.0);
            let gate = bright * stands * (1.0 - sky.clamp(0.0, 1.0));
            if gate > 0.0 {
                let f = 1.0 + strength * gate;
                colour[i] = colour[i].map(|v| v * f);
            }
        }
    }

    // Layers, front to back: 7 nearer buckets (nearest first), the focus slab, 7 farther buckets.
    // Index 0 is unused so the arithmetic stays plain.
    let layers = 2 * SIDE_BUCKETS + 2;
    let mut lc = vec![vec![[0.0f32; 3]; ww * wh]; layers];
    let mut la = vec![vec![0.0f32; ww * wh]; layers];
    // Radii per layer: the focus slab is sharp; bucket b has the geometric mean of its range.
    let r0 = (r_max * k / 64.0).max(0.6);
    let mut lr = vec![0.0f32; layers];
    for b in 0..SIDE_BUCKETS {
        let r = r0 * 2f32.powi(b as i32) * 1.4142;
        // Nearer buckets: the far-in-front one (b = last) is nearest the camera and comes first.
        lr[SIDE_BUCKETS - b] = r;
        lr[FOCUS_LAYER + 1 + b] = r;
    }
    /// The layer for a signed radius, and the share going to the next bucket out: soft edges
    /// between buckets so a CoC that grows across a boundary isn't banded.
    fn bucket(r: f32, r0: f32) -> (usize, usize, f32) {
        let a = r.abs();
        if a <= r0 {
            return (FOCUS_LAYER, FOCUS_LAYER, 0.0);
        }
        let u = (a / r0).log2().clamp(0.0, (SIDE_BUCKETS as f32) - 0.001);
        let b = u.floor() as usize;
        let f = u - b as f32;
        if r < 0.0 {
            let z = SIDE_BUCKETS - b;
            let next = if b + 1 < SIDE_BUCKETS { z - 1 } else { z };
            (z, next, f)
        } else {
            let z = FOCUS_LAYER + 1 + b;
            let next = if b + 1 < SIDE_BUCKETS { z + 1 } else { z };
            (z, next, f)
        }
    }

    let mut assign = |i: usize, layer: usize, wt: f32, c: [f32; 3]| {
        if wt <= 0.0 {
            return;
        }
        let a = &mut la[layer][i];
        let cur = *a;
        let next = cur + wt;
        if next > 1e-9 {
            for ch in 0..3 {
                lc[layer][i][ch] = (lc[layer][i][ch] * cur + c[ch] * wt) / next;
            }
        }
        *a = next;
    };
    for y in 0..wh {
        for x in 0..ww {
            let i = y * ww + x;
            let (fx, fy) = frame.map_or((((x as f32 + 0.5) / ww as f32), ((y as f32 + 0.5) / wh as f32)), |f| {
                // The working pixel's place in the whole photo, through the part's frame.
                let (px, py) = (x as f32 * w as f32 / ww as f32, y as f32 * h as f32 / wh as f32);
                (((f.x0 + px + 0.5) / f.full_w), ((f.y0 + py + 0.5) / f.full_h))
            });
            let (n, fr, alpha) = depth.edges.as_ref().map_or((depth.control.at(fx, fy), depth.control.at(fx, fy), 1.0), |e| e.at(fx, fy));
            let mut rn = radius_of(manual_z(n));
            let mut rf = radius_of(manual_z(fr));
            if bokeh.mode == BokehMode::Manual || !metric {
                let max_r = MAX_RADIUS_SHARE * long_px * (bokeh.amount / 100.0).clamp(0.0, 1.0);
                rn = manual_radius(1.0 / manual_z(n) - 1.0 / s_m, inv_max, 100.0, max_r);
                rf = manual_radius(1.0 / manual_z(fr) - 1.0 / s_m, inv_max, 100.0, max_r);
            }
            // Inside the focus band the CoC eases to zero, never steps (the brief's smooth ramp).
            let ease = |r: f32, value: f32| -> f32 {
                if range <= 1e-4 {
                    return r;
                }
                let outside = if value < band_lo {
                    (band_lo - value) / band_soft
                } else if value > band_hi {
                    (value - band_hi) / band_soft
                } else {
                    0.0
                };
                r * outside.clamp(0.0, 1.0)
            };
            let zn = manual_z(n);
            let zf = manual_z(fr);
            let ease_value = |c: f32, z: f32| if metric { z } else { c };
            rn = ease(rn, ease_value(n, zn)).clamp(-max_radius, max_radius) * k;
            rf = ease(rf, ease_value(fr, zf)).clamp(-max_radius, max_radius) * k;
            let c = colour[i];
            // The two surfaces: the nearer one's share is alpha.
            for (r, wt) in [(rf, 1.0 - alpha.clamp(0.0, 1.0)), (rn, alpha.clamp(0.0, 1.0))] {
                if wt <= 1e-4 {
                    continue;
                }
                let (base, next, f) = bucket(r, r0);
                if f <= 1e-4 {
                    assign(i, base, wt, c);
                } else {
                    assign(i, base, wt * (1.0 - f), c);
                    assign(i, next, wt * f, c);
                }
            }
        }
    }

    // Blur every layer at its own radius (the focus slab stays sharp), then composite front to
    // back. Colour goes in premultiplied by its coverage, so a blur that reaches past an
    // occluder's edge takes the light of what is visible there (normalised convolution): the
    // "scatter as gather" the brief asks for, with no dark or bright ring round a sharp edge.
    let mut out_c = vec![[0.0f32; 3]; ww * wh];
    let mut out_a = vec![0.0f32; ww * wh];
    for layer in 0..layers {
        let (c, a) = (std::mem::take(&mut lc[layer]), &la[layer]);
        let premul: Vec<[f32; 3]> = c.iter().zip(a.iter()).map(|(c, w)| c.map(|v| v * w)).collect();
        let r = lr[layer];
        let fringe = bokeh.fringe / 100.0;
        let mut bc;
        let mut ba;
        if r >= 0.7 {
            let shape = bokeh.shape;
            let blades = bokeh.blades.clamp(3, 12);
            let rounding = bokeh.blade_rounding;
            // Longitudinal CA: in front of focus purple (red and blue spread more), behind green.
            let front = layer < FOCUS_LAYER;
            let scale_r = if fringe > 0.0 && front { 1.0 + 0.035 * fringe } else { 1.0 };
            let scale_b = scale_r;
            let scale_g = if fringe > 0.0 && !front { 1.0 + 0.035 * fringe } else { 1.0 };
            let kg = kernel(shape, r, blades, rounding, scale_g);
            if (scale_r - 1.0).abs() > 1e-6 || (scale_b - 1.0).abs() > 1e-6 {
                let kr = kernel(shape, r, blades, rounding, scale_r);
                let (cr, _) = blur(&premul, a, ww, wh, &kr);
                let (cg, ag) = blur(&premul, a, ww, wh, &kg);
                let (cb, _) = blur(&premul, a, ww, wh, &kr);
                bc = cg;
                for i in 0..ww * wh {
                    bc[i] = [cr[i][0], bc[i][1], cb[i][2]];
                }
                ba = ag;
            } else {
                let (cc, aa) = blur(&premul, a, ww, wh, &kg);
                bc = cc;
                ba = aa;
            }
            if shape == BokehShape::Swirl {
                let (sc, sa) = swirl(&bc, &ba, ww, wh, 1.0);
                bc = sc;
                ba = sa;
            }
            if bokeh.cats_eye > 1e-3 {
                let (cc, aa) = cats_eye(&bc, &ba, ww, wh, bokeh.cats_eye / 100.0, r);
                bc = cc;
                ba = aa;
            }
        } else {
            // The sharp slab: premultiplied directly (no blur, no resampling).
            bc = premul;
            ba = a.to_vec();
        }
        for i in 0..ww * wh {
            let keep = (1.0 - out_a[i]).max(0.0);
            let a = ba[i].clamp(0.0, 1.0) * keep;
            if a <= 0.0 {
                continue;
            }
            // bc is the layer's premultiplied light: its share is already the coverage `a`.
            for ch in 0..3 {
                out_c[i][ch] += bc[i][ch] * keep;
            }
            out_a[i] += a;
        }
    }
    for i in 0..ww * wh {
        let a = out_a[i].max(1e-6);
        out_c[i] = out_c[i].map(|v| v / a);
    }

    // The sharp slab's coverage, at the **photo's own** resolution and straight from the depth map:
    // taken from the shrunken working copy and stretched back up, it stepped at each of its pixels
    // (a quarter-size copy turned every 4 x 4 block of the outline into a stair, which read as a
    // jagged edge round a face as Bokeh grew). The band is in metres, so it goes to control depth
    // first; the two surfaces at a depth edge share it by the edge alpha, as the blur layers do.
    let (far_c, near_c, c_soft) = if metric {
        let near_c = crate::scene::control_of(band_lo, d_near, d_far);
        let far_c = if band_hi.is_finite() { crate::scene::control_of(band_hi, d_near, d_far) } else { 0.0 };
        let soft = (crate::scene::control_of(band_lo + band_soft, d_near, d_far) - near_c).abs().max(0.004);
        (far_c.min(near_c), near_c.max(far_c), soft)
    } else {
        (band_lo, band_hi, band_soft)
    };
    let smooth = |lo: f32, hi: f32, x: f32| {
        let t = ((x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let cover = |c: f32| smooth(far_c - c_soft, far_c, c) * (1.0 - smooth(near_c, near_c + c_soft, c));
    use rayon::prelude::*;
    let focus_full: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (fx, fy) = frame.map_or((((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32), |f| f.frac(i, w));
            let (n, fr, a) = depth.edges.as_ref().map_or((depth.control.at(fx, fy), depth.control.at(fx, fy), 1.0), |e| e.at(fx, fy));
            let a = a.clamp(0.0, 1.0);
            (a * cover(n) + (1.0 - a) * cover(fr)).clamp(0.0, 1.0)
        })
        .collect();
    // Back up to the photo's size; the focus slab comes from the original at full resolution, so
    // in-focus detail is untouched.
    let px: Vec<[f32; 3]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (fx, fy) = (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
            let b = sample_colour(&out_c, ww, wh, fx * ww as f32, fy * wh as f32);
            let f = focus_full[i];
            // The blurred result where the focus slab doesn't reach; the original inside it.
            let orig = image.px[i];
            std::array::from_fn(|ch| b[ch] + (orig[ch] - b[ch]) * f)
        })
        .collect();
    Some(SceneImage { width: w, height: h, px, source_long: image.source_long })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::EdgeMaps;
    use std::sync::Arc;

    fn flat_depth(w: usize, h: usize, control: f32) -> Refined {
        let grid = Grid { w, h, data: vec![control; w * h] };
        Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: None,
            normals: None,
        }
    }

    #[test]
    fn a_known_thin_lens_circle() {
        // 85 mm at f/1.8, focused at 2 m, a point at 3 m, a 36 mm sensor and a 2400 px frame:
        // sensor diameter 0.085² / (1.8 · 1.915) · 1/3 = 0.699 mm, 46.6 px across, radius 23.3 px.
        let d = coc_sensor_mm(85.0, 1.8, 2.0, 3.0);
        assert!((d - 0.6986).abs() < 0.005, "{d}");
        let r = coc_radius_px(d, 36.0, 2400.0);
        assert!((r - 23.29).abs() < 0.05, "{r}");
        // In front of focus is negative; at the focus distance it is zero.
        assert!(coc_sensor_mm(85.0, 1.8, 2.0, 1.5) < 0.0);
        assert!(coc_sensor_mm(85.0, 1.8, 2.0, 2.0).abs() < 1e-6);
        // At infinity it is the base diameter, positive.
        assert!((coc_sensor_mm(85.0, 1.8, 2.0, f32::INFINITY) - 2.096).abs() < 0.005);
    }

    #[test]
    fn opening_up_adds_only_the_extra_blur_in_quadrature() {
        let (f, s, z, sensor, w) = (85.0, 2.0, 5.0, 36.0, 2400.0);
        let shot = coc_radius_px(coc_sensor_mm(f, 4.0, s, z), sensor, w);
        let target = coc_radius_px(coc_sensor_mm(f, 1.4, s, z), sensor, w);
        let extra = open_up_radius(f, 4.0, 1.4, s, z, sensor, w);
        assert!((extra - (target * target - shot * shot).sqrt()).abs() < 1e-4);
        // Stopping down can't add blur.
        assert_eq!(open_up_radius(f, 4.0, 8.0, s, z, sensor, w), 0.0);
        // And a point already in focus gains blur behind it, none at it.
        assert!(open_up_radius(f, 4.0, 1.4, s, s, sensor, w).abs() < 1e-5);
    }

    #[test]
    fn simulating_the_same_lens_is_no_change_and_a_longer_one_blurs_more() {
        let (s, z, sensor, w) = (2.0, 6.0, 36.0, 2400.0);
        // The "step back" has nothing to remove when the lens is the shot's own.
        assert!(simulate_radius(50.0, 2.0, 50.0, 2.0, s, z, sensor, w).abs() < 1e-4);
        // 85 mm f/1.2 from a 50 mm f/2 frame: more blur.
        let more = simulate_radius(50.0, 2.0, 85.0, 1.2, s, z, sensor, w);
        assert!(more > 10.0, "{more}");
        // The focus point itself stays sharp whatever lens is simulated (the subject is kept the
        // same size by stepping back).
        assert!(simulate_radius(50.0, 2.0, 85.0, 1.2, s, s, sensor, w).abs() < 1e-4);
    }

    #[test]
    fn the_sharp_zone_follows_the_f_number() {
        // 85 mm at f/1.8 focused at 2 m, 0.03 mm: about 1.97 to 2.03 m.
        let (near, far) = depth_of_field(85.0, 1.8, 2.0, 0.03);
        assert!((near - 1.9718).abs() < 0.002 && (far - 2.0290).abs() < 0.002, "{near} {far}");
        // Its ends blur by exactly the circle that counts as sharp.
        assert!((coc_sensor_mm(85.0, 1.8, 2.0, far) - 0.03).abs() < 1e-3);
        assert!((coc_sensor_mm(85.0, 1.8, 2.0, near) + 0.03).abs() < 1e-3);
        // Past the hyperfocal distance it runs to infinity.
        assert!(depth_of_field(24.0, 11.0, 10.0, 0.03).1.is_infinite());

        let shot = LensFacts { focal_mm: 50.0, f_number: 4.0, shot_focal_mm: 50.0, shot_f_number: 4.0, sensor_width_mm: 36.0, exif_focus_m: None };
        let at = |mode: BokehMode, aperture: f32, focal: f32| sharp_zone(&Bokeh { mode, aperture, focal, ..Default::default() }, &shot, 3.0);
        let width = |z: Option<(f32, f32)>| z.map(|(a, b)| b - a).unwrap();
        // Opening up thins it, and it can't grow past the shot's own.
        assert!(width(at(BokehMode::Open, 1.4, 50.0)) < width(at(BokehMode::Open, 2.8, 50.0)));
        assert!(width(at(BokehMode::Open, 2.8, 50.0)) < width(at(BokehMode::Open, 4.0, 50.0)));
        assert_eq!(at(BokehMode::Open, 8.0, 50.0), at(BokehMode::Open, 4.0, 50.0));
        // Simulating the shot's own lens is its own zone, and the focus is always inside.
        let (a, b) = (at(BokehMode::Simulate, 4.0, 50.0).unwrap(), at(BokehMode::Open, 4.0, 50.0).unwrap());
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
        let z = at(BokehMode::Simulate, 1.2, 135.0).unwrap();
        assert!(z.0 < 3.0 && z.1 > 3.0 && z.1 - z.0 < width(Some(b)));
        assert!(at(BokehMode::Manual, 1.4, 50.0).is_none());
    }

    #[test]
    fn every_aperture_shape_keeps_its_energy() {
        for shape in [BokehShape::Smooth, BokehShape::Blades, BokehShape::Bubble, BokehShape::Swirl, BokehShape::Mirror] {
            for radius in [1.5f32, 4.0, 12.0] {
                let k = kernel(shape, radius, 7, 40.0, 1.0);
                let sum: f32 = k.taps.iter().map(|t| t.2).sum();
                assert!((sum - 1.0).abs() < 1e-3, "{shape:?} at {radius}: {sum}");
                assert!(k.taps.iter().all(|t| t.2 >= 0.0));
                let far = k.taps.iter().map(|t| ((t.0 * t.0 + t.1 * t.1) as f32).sqrt()).fold(0.0, f32::max);
                assert!(far <= radius * 1.5 + 2.0, "{shape:?} at {radius}: tap {far} px out");
            }
        }
        // Bubble brightens toward its rim; mirror has nothing in the middle.
        let bubble = kernel(BokehShape::Bubble, 8.0, 7, 0.0, 1.0);
        let centre = bubble.taps.iter().filter(|t| t.0.abs() + t.1.abs() <= 3).map(|t| t.2).sum::<f32>();
        let rim = bubble.taps.iter().filter(|t| ((t.0 * t.0 + t.1 * t.1) as f32).sqrt() > 6.0).map(|t| t.2).sum::<f32>();
        assert!(rim > centre * 2.0, "bubble: centre {centre}, rim {rim}");
        let mirror = kernel(BokehShape::Mirror, 8.0, 7, 0.0, 1.0);
        assert!(mirror.taps.iter().all(|t| ((t.0 * t.0 + t.1 * t.1) as f32).sqrt() > 3.0));
    }

    #[test]
    fn a_sharp_subject_over_a_bright_background_has_no_halo() {
        // 160 x 100: a dark subject square in the middle, a bright background, the subject in focus
        // and the background far behind. The blurred picture must not grow a bright ring round it.
        let (w, h) = (160usize, 100usize);
        let mut px = vec![[1.6f32, 1.6, 1.6]; w * h];
        for y in 30..70 {
            for x in 60..100 {
                px[y * w + x] = [0.05, 0.05, 0.05];
            }
        }
        let image = SceneImage { width: w, height: h, px: px.clone(), source_long: 160 };
        // Control depth: subject 0.9 (near), background 0.1 (far), hard at its edge.
        let mut data = vec![0.1f32; w * h];
        for y in 30..70 {
            for x in 60..100 {
                data[y * w + x] = 0.9;
            }
        }
        let grid = Grid { w, h, data };
        let depth = Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: None,
            normals: None,
        };
        let bokeh = Bokeh { enabled: true, mode: BokehMode::Manual, amount: 100.0, focus_range: 0.0, focus_auto: false, focus_u: 0.5, focus_v: 0.5, ..Default::default() };
        let out = apply(&image, &depth, &bokeh, None, None, None, 0.0).unwrap();
        // After the render the subject is still dark, the background about its own brightness, and
        // just outside the subject's edge there is no ring brighter than the far background.
        let luma = |p: [f32; 3]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
        let inside = luma(out.px[50 * w + 80]);
        assert!(inside < 0.4, "subject washed out: {inside}");
        let far = luma(out.px[5 * w + 5]);
        let mut ring = 0.0f32;
        for y in 26..74 {
            for x in 56..104 {
                let on_edge = (30..70).contains(&y) && (60..100).contains(&x);
                if on_edge {
                    continue;
                }
                let near_edge = y >= 26 && y < 30 && (60..100).contains(&x) || y >= 70 && y < 74 && (60..100).contains(&x);
                let near_side = x >= 56 && x < 60 && (30..70).contains(&y) || x >= 100 && x < 104 && (30..70).contains(&y);
                if near_edge || near_side {
                    ring = ring.max(luma(out.px[y * w + x]));
                }
            }
        }
        assert!(ring <= far * 1.06 + 0.02, "a bright ring: {ring} against the background {far}");
        // Inside the subject, away from the edge, the original light is kept (the focus slab).
        assert!((luma(out.px[50 * w + 80]) - 0.05).abs() < 0.03);
    }

    #[test]
    fn a_slider_not_moved_is_the_shots_own_lens() {
        let shot = LensFacts { focal_mm: 50.0, f_number: 2.8, shot_focal_mm: 50.0, shot_f_number: 2.8, sensor_width_mm: 36.0, exif_focus_m: None };
        let own = with_shot(&Bokeh::default(), &shot);
        assert_eq!((own.aperture, own.focal), (2.8, 50.0));
        assert_eq!(simulate_radius(50.0, 2.8, own.focal, own.aperture, 2.0, 6.0, 36.0, 2400.0), 0.0);
        let moved = with_shot(&Bokeh { aperture: 1.4, focal: 85.0, ..Default::default() }, &shot);
        assert_eq!((moved.aperture, moved.focal), (1.4, 85.0));
    }

    #[test]
    fn the_blur_scales_with_the_photo_upright_or_not() {
        // How far a hard edge behind the focus spreads, as a share of the long side.
        let spread = |w: usize, h: usize| {
            let control = crate::scene::control_of(10.0, 1.0, 100.0);
            let mut data = vec![control; w * h];
            for v in &mut data[w * (h * 9 / 10)..] {
                *v = crate::scene::control_of(2.6, 1.0, 100.0);
            }
            let grid = Grid { w, h, data };
            let depth = Refined { control: Arc::new(grid.clone()), sky: None, edges: Some(Arc::new(EdgeMaps::flat(&grid))), anchors: Some((1.0, 100.0)), focal_px: Some(120.0), ground: None, normals: None };
            let px = (0..w * h).map(|i| if i % w < w / 2 { [0.1; 3] } else { [0.5; 3] }).collect();
            let image = SceneImage { width: w, height: h, px, source_long: w.max(h) };
            let bokeh = Bokeh { enabled: true, aperture: 0.95, focus_auto: false, focus_u: 0.5, focus_v: 0.95, ..Default::default() };
            let out = apply(&image, &depth, &bokeh, None, None, None, 0.0).unwrap();
            let row = &out.px[w * (h * 2 / 5)..][..w];
            let at = |t: f32| row.iter().position(|p| p[0] >= 0.1 + 0.4 * t).unwrap() as f32;
            (at(0.9) - at(0.1)) / w.max(h) as f32
        };
        let wide = spread(1200, 800);
        assert!((spread(2400, 1600) / wide - 1.0).abs() < 0.2, "{wide} {}", spread(2400, 1600));
        assert!((spread(800, 1200) / wide - 1.0).abs() < 0.2, "{wide} {}", spread(800, 1200));
    }

    #[test]
    fn bokeh_off_returns_nothing() {
        let image = SceneImage { width: 8, height: 8, px: vec![[0.5; 3]; 64], source_long: 8 };
        let bokeh = Bokeh::default();
        assert!(!bokeh.enabled);
        assert!(apply(&image, &flat_depth(8, 8, 0.5), &bokeh, None, None, None, 0.0).is_none());
    }
}
