//! Colour noise reduction (the Colour slider, Lightroom's Color): the blotches and speckle of colour
//! smoothed away, brightness kept exactly as it was. On the scene light, before every other tool.
//!
//! Each pixel's colour, as a difference of cube roots, is averaged over its area, following brightness. A colour far from that
//! average is real (a small coloured detail, an edge) and stays. Box blurs, so the time doesn't grow with the radius.

use crate::pipeline::SceneImage;
use rayon::prelude::*;
use std::sync::Arc;

/// Rec. 2020's brightness weights (the scene light's primaries).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];
/// Added to the light before its cube root, so near-black doesn't swing wildly.
const FLOOR: f32 = 0.002;
/// The blur's radius at Colour 25, in full-sensor pixels (two box passes: reaches twice this). Fitted with
/// `REAL_25` to Lightroom's Color at the same numbers (`examples/colournoise_match.rs fit`, 1 Nov).
const RADIUS_25: f32 = 10.0;
/// How far from the area's colour counts as real, at Colour 25 (in cube-root units),
/// and the brightness below which that widens (noise grows in the dark).
const REAL_25: f32 = 0.04;
const DARK: f32 = 0.18;
/// Brightness variance (cube-root units) that counts as noise rather than a shape to follow.
const GUIDE_EPS: f32 = 1.6e-3;

/// A blur radius under half a pixel of this picture: the downsizing has already averaged it.
fn radius(amount: f32, scale: f32, radius_25: f32) -> usize {
    (radius_25 * (amount / 25.0).sqrt() * scale.clamp(0.0, 1.0)).round() as usize
}

/// Two box passes of radius `r` over a plane, rows then columns.
fn soften(v: &mut Vec<f32>, w: usize, h: usize, r: usize) {
    let rows = |src: &[f32], out: &mut [f32], w: usize, r: usize| {
        out.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, s)| {
            let n = 2 * r + 1;
            let at = |i: isize| s[i.clamp(0, w as isize - 1) as usize];
            let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
            for (x, p) in o.iter_mut().enumerate() {
                *p = sum / n as f32;
                sum += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
            }
        });
    };
    let flip = |src: &[f32], w: usize, h: usize| -> Vec<f32> {
        let mut out = vec![0.0; w * h];
        out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
            for (y, c) in col.iter_mut().enumerate() {
                *c = src[y * w + x];
            }
        });
        out
    };
    let mut tmp = vec![0.0; v.len()];
    for _ in 0..2 {
        rows(v, &mut tmp, w, r);
        std::mem::swap(v, &mut tmp);
    }
    let mut t = flip(v, w, h);
    tmp.resize(t.len(), 0.0);
    for _ in 0..2 {
        rows(&t, &mut tmp, h, r);
        std::mem::swap(&mut t, &mut tmp);
    }
    *v = flip(&t, h, w);
}

/// The picture with its colour noise reduced (`amount` 0..100; `scale`: its long edge over the
/// sensor's); None when that changes nothing.
pub fn apply(img: &SceneImage, amount: f32, scale: f32) -> Option<SceneImage> {
    apply_tuned(img, amount, scale, RADIUS_25, REAL_25, GUIDE_EPS)
}

/// `apply` with the fitted numbers given (`examples/colournoise_match.rs`).
pub fn apply_tuned(img: &SceneImage, amount: f32, scale: f32, radius_25: f32, real_25: f32, eps: f32) -> Option<SceneImage> {
    let (w, h) = (img.width, img.height);
    let r = radius(amount, scale, radius_25);
    if amount <= 0.0 || r == 0 || w < 3 || h < 3 {
        return None;
    }
    let bright = |p: &[f32; 3]| LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2];
    // Colour as a difference of cube roots (as CIELAB's a and b are): an even average of it keeps the
    // area's colour, where averaging linear light would pull it towards the area's brightest parts.
    // Signed, so light below zero (common in the dark, where noise crosses black) comes back too.
    let enc = |v: f32| (v + FLOOR).cbrt();
    let dec = |v: f32| v * v * v - FLOOR;
    let colour = |p: &[f32; 3]| {
        let e = enc(bright(p));
        (enc(p[0]) - e, enc(p[2]) - e)
    };
    // A guided filter, led by brightness: within an area, colour is fitted as a straight line of
    // brightness, so where brightness has an edge or a shape the colour follows it and doesn't spread
    // across. `eps` is the brightness variance below which a shape counts as noise, and colour goes flat.
    let plane = |f: &(dyn Fn(&[f32; 3]) -> f32 + Sync)| -> Vec<f32> { img.px.par_iter().map(f).collect() };
    let guide = plane(&|p| enc(bright(p)));
    let blur = |mut v: Vec<f32>| {
        soften(&mut v, w, h, r);
        v
    };
    let mean_i = blur(guide.clone());
    let var_i = blur(guide.par_iter().map(|g| g * g).collect());
    let fit = |c: Vec<f32>| -> Vec<f32> {
        let cross = blur(c.par_iter().zip(&guide).map(|(c, g)| c * g).collect());
        let mean_c = blur(c);
        let a: Vec<f32> = (0..w * h).into_par_iter().map(|i| (cross[i] - mean_i[i] * mean_c[i]) / ((var_i[i] - mean_i[i] * mean_i[i]).max(0.0) + eps)).collect();
        let b: Vec<f32> = (0..w * h).into_par_iter().map(|i| mean_c[i] - a[i] * mean_i[i]).collect();
        let (a, b) = (blur(a), blur(b));
        (0..w * h).into_par_iter().map(|i| a[i] * guide[i] + b[i]).collect()
    };
    let red = fit(plane(&|p| colour(p).0));
    let blue = fit(plane(&|p| colour(p).1));
    let area: Vec<f32> = mean_i.par_iter().map(|m| dec(*m).max(0.0)).collect();
    // How far each colour is from its area's, itself averaged over a few pixels: a real detail has
    // neighbours that differ the same way; a lone speckle doesn't, and is smoothed with the rest.
    let mut off_r: Vec<f32> = img.px.par_iter().enumerate().map(|(i, p)| colour(p).0 - red[i]).collect();
    let mut off_b: Vec<f32> = img.px.par_iter().enumerate().map(|(i, p)| colour(p).1 - blue[i]).collect();
    soften(&mut off_r, w, h, 1);
    soften(&mut off_b, w, h, 1);
    // The first few of the slider ease in, rather than the whole effect arriving at 1.
    let mix = (amount / 10.0).min(1.0);
    let real = real_25 * (amount / 25.0).sqrt();
    let px = img
        .px
        .par_iter()
        .enumerate()
        .map(|(i, p)| {
            let y = bright(p);
            let e = enc(y);
            let (cr, cb) = colour(p);
            // Noise is larger in the dark.
            let limit = real * (1.0 + DARK / (area[i] + FLOOR)).cbrt();
            let far = off_r[i].abs().max(off_b[i].abs()) / limit;
            let k = mix * (2.0 - far).clamp(0.0, 1.0);
            let (r2, b2) = (dec(e + cr + (red[i] - cr) * k), dec(e + cb + (blue[i] - cb) * k));
            [r2, (y - LUMA[0] * r2 - LUMA[2] * b2) / LUMA[1], b2]
        })
        .collect();
    Some(SceneImage { width: w, height: h, px, source_long: img.source_long })
}

type Recent = Vec<((u64, u32, u32), Arc<SceneImage>)>;
static RECENT: std::sync::Mutex<Recent> = std::sync::Mutex::new(Vec::new());
const RECENT_KEPT: usize = 2;
/// Bigger than this (a whole photo for export) isn't held here as well as on its Scene.
const RECENT_MAX_PIXELS: usize = 16_000_000;

/// A photo's filtered scene for this amount, kept with it: while other sliders move it's the same
/// picture each frame (and the GPU needn't be sent it again).
pub fn for_scene(scene: &crate::develop::Scene, amount: f32, scale: f32) -> Option<Arc<SceneImage>> {
    if amount <= 0.0 {
        return None;
    }
    let mut kept = scene.colour_cleaned.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((a, img)) = kept.as_ref() {
        if *a == amount {
            return Some(img.clone());
        }
    }
    // A zoomed-in part is a new Scene each render but usually the same pixels while a slider moves:
    // the last few results are kept by content, or every drag frame would pay for the filter.
    if scene.image.px.len() > RECENT_MAX_PIXELS {
        let img = Arc::new(apply(&scene.image, amount, scale)?);
        *kept = Some((amount, img.clone()));
        return Some(img);
    }
    let key = (crate::defringe::fingerprint(&scene.image), amount.to_bits(), scale.to_bits());
    let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let img = match recent.iter().find(|(k, _)| *k == key) {
        Some((_, img)) => img.clone(),
        None => {
            let img = Arc::new(apply(&scene.image, amount, scale)?);
            recent.push((key, img.clone()));
            if recent.len() > RECENT_KEPT {
                recent.remove(0);
            }
            img
        }
    };
    *kept = Some((amount, img.clone()));
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn average(img: &SceneImage) -> [f32; 3] {
        let n = img.px.len() as f32;
        std::array::from_fn(|k| img.px.iter().map(|p| p[k]).sum::<f32>() / n)
    }

    #[test]
    fn an_area_keeps_its_colour() {
        // Skin-like colour lit unevenly (dark to bright, more saturated in the dark), with speckle: its
        // colour by brightness band must not drift towards the brightest part's.
        let (w, h) = (96, 96);
        let mut seed = 7u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 16) as f32 / 65535.0 - 0.5
        };
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let t = 0.3 + 0.7 * (((i % 8) as f32 / 8.0).powi(2));
                let (r, g, b) = (0.40 * t, 0.25 * t, 0.17 * t);
                let n = rnd() * 0.01;
                [r + n, g, b - n]
            })
            .collect();
        let img = SceneImage { width: w, height: h, px, source_long: w };
        let out = apply(&img, 25.0, 1.0).unwrap();
        let (a, b) = (average(&img), average(&out));
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() / a[k] < 0.03, "{a:?} -> {b:?}");
        }
    }

    fn noisy(w: usize, h: usize) -> SceneImage {
        // A flat grey with colour speckle: red and blue swapped about, brightness held.
        let mut seed = 1u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 16) as f32 / 65535.0 - 0.5
        };
        let px = (0..w * h)
            .map(|_| {
                let (a, b) = (rnd() * 0.04, rnd() * 0.04);
                [0.18 + a, 0.18 - (LUMA[0] * a + LUMA[2] * b) / LUMA[1], 0.18 + b]
            })
            .collect();
        SceneImage { width: w, height: h, px, source_long: w }
    }

    fn speckle(img: &SceneImage) -> f32 {
        (img.px.iter().map(|p| (p[0] - p[1]).powi(2)).sum::<f32>() / img.px.len() as f32).sqrt()
    }

    #[test]
    fn colour_speckle_goes_and_brightness_stays() {
        let img = noisy(128, 96);
        let out = apply(&img, 25.0, 1.0).unwrap();
        assert!(speckle(&out) < speckle(&img) * 0.25, "{} vs {}", speckle(&out), speckle(&img));
        for (a, b) in img.px.iter().zip(&out.px) {
            let y = |p: &[f32; 3]| LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2];
            assert!((y(a) - y(b)).abs() < 1e-4);
        }
    }

    #[test]
    fn a_real_colour_edge_stays() {
        // Red beside blue at the same brightness: far from the average, so kept.
        let (w, h) = (64, 32);
        let (red, blue) = ([0.6, 0.05, 0.05], [0.05, 0.1, 1.5]);
        let px = (0..w * h).map(|i| if i % w < w / 2 { red } else { blue }).collect();
        let img = SceneImage { width: w, height: h, px, source_long: w };
        let out = apply(&img, 25.0, 1.0).unwrap();
        let p = out.px[h / 2 * w + w / 2 - 2];
        assert!((p[0] - red[0]).abs() < 0.02 && (p[2] - red[2]).abs() < 0.02, "{p:?}");
    }

    #[test]
    fn a_lone_speckle_goes() {
        let (w, h) = (64, 64);
        let mut px = vec![[0.18, 0.18, 0.18]; w * h];
        px[32 * w + 32] = [0.30, 0.12, 0.30];
        let img = SceneImage { width: w, height: h, px, source_long: w };
        let out = apply(&img, 25.0, 1.0).unwrap();
        let p = out.px[32 * w + 32];
        assert!((p[0] - p[1]).abs() < 0.02, "{p:?}");
    }

    #[test]
    fn colour_stops_at_a_brightness_edge() {
        // A dark red strip against a bright blue-white area: the white stays clean right up to the edge.
        let (w, h) = (64, 32);
        let (dark, light) = ([0.10, 0.02, 0.02], [0.72, 0.80, 0.95]);
        let px = (0..w * h).map(|i| if i % w < w / 2 { dark } else { light }).collect();
        let img = SceneImage { width: w, height: h, px, source_long: w };
        let out = apply(&img, 25.0, 1.0).unwrap();
        let (p, q) = (out.px[h / 2 * w + w / 2 + 1], out.px[h / 2 * w + w / 2 - 2]);
        assert!((p[0] / p[2] - light[0] / light[2]).abs() < 0.03, "{p:?}");
        assert!((q[0] / q[1] - dark[0] / dark[1]).abs() < 0.8, "{q:?}");
    }

    #[test]
    fn zero_and_tiny_pictures_are_left() {
        assert!(apply(&noisy(16, 16), 0.0, 1.0).is_none());
        assert!(apply(&noisy(16, 16), 25.0, 0.01).is_none());
    }
}
