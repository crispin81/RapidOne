//! The subject-aware vignette's weights: how much to darken each part of the
//! frame (0 = not at all, 1 = fully), from the subject mask (`mask.rs`).
//!
//! Nothing inside the subject. Outside, the darkening fades in with distance
//! from the subject's edge over a fixed falloff, and is a little fuller
//! towards the frame's edges, so it hugs the people rather than an oval.

use crate::mask::Mask;

/// Long edge of the weight grid: plenty for a smooth falloff, and fast.
const GRID: usize = 256;
/// How far from the subject's edge the darkening takes to reach full, as a
/// share of the frame's long edge (long, for a gradual feather).
const FALLOFF: f32 = 0.55;
/// Shape of the feather: the darkening starts right at the subject's edge
/// and eases towards full (higher = darker sooner).
const EASE: f32 = 2.2;

#[derive(Clone, Debug, PartialEq)]
pub struct VignetteMap {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) data: Vec<f32>,
}

impl VignetteMap {
    /// The weight at (u, v), each -1..1 across the frame, bilinear.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        let x = ((u + 1.0) * 0.5 * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = ((v + 1.0) * 0.5 * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * self.w + x];
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

fn smoothstep(x: f32, from: f32, to: f32) -> f32 {
    let t = ((x - from) / (to - from).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The weights for a frame `width` x `height` from its subject mask, or None
/// when there's no clear subject (nothing found, or it fills the frame), so
/// the plain oval vignette is used.
pub fn from_mask(mask: &Mask, width: usize, height: usize) -> Option<VignetteMap> {
    let scale = width.max(height) as f32 / GRID as f32;
    let w = ((width as f32 / scale).round() as usize).max(2);
    let h = ((height as f32 / scale).round() as usize).max(2);
    let soft: Vec<f32> = (0..w * h).map(|i| mask.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)).collect();
    let area = soft.iter().filter(|&&m| m > 0.5).count() as f32 / (w * h) as f32;
    if !(0.005..=0.85).contains(&area) {
        return None;
    }

    // Distance from each pixel to the nearest subject pixel (two-pass
    // chamfer, in grid pixels).
    const BIG: f32 = 1e9;
    let mut d: Vec<f32> = soft.iter().map(|&m| if m > 0.5 { 0.0 } else { BIG }).collect();
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 { v = v.min(d[i - 1] + a); }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 { v = v.min(d[i - w - 1] + b); }
                if x + 1 < w { v = v.min(d[i - w + 1] + b); }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w { v = v.min(d[i + 1] + a); }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w { v = v.min(d[i + w + 1] + b); }
                if x > 0 { v = v.min(d[i + w - 1] + b); }
            }
            d[i] = v;
        }
    }

    let long = w.max(h) as f32;
    let data = (0..w * h)
        .map(|i| {
            let (u, v) = (((i % w) as f32 + 0.5) / w as f32 * 2.0 - 1.0, ((i / w) as f32 + 0.5) / h as f32 * 2.0 - 1.0);
            // Fuller towards the frame's edges and corners: 0.7x inwards, 1.2x there.
            let edge = smoothstep((u * u + v * v).sqrt(), 0.35, 1.4);
            // Starts at the subject's edge (tight), then eases gradually to full.
            let t = (d[i] / long / FALLOFF).clamp(0.0, 1.0);
            let away = 1.0 - (1.0 - t).powf(EASE);
            // The soft edge of the mask itself (hair, fingers) blends in too.
            away * (0.7 + 0.5 * edge * edge) * (1.0 - soft[i])
        })
        .collect();
    Some(VignetteMap { w, h, data })
}
