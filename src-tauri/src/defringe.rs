//! Defringe, as Lightroom's Lens Corrections panel has it: purple and green
//! colour fringes (axial chromatic aberration, which no lens profile can fix)
//! taken out along high-contrast edges only, so purple flowers and green
//! leaves away from an edge keep their colour.
//!
//! Each colour has an Amount (0..20, Lightroom's scale) and a hue range
//! (0..100 along that colour's own hue band, Lightroom's defaults 30-70 for
//! purple and 40-60 for green). A pixel is cleaned by how much it is (1) near
//! a strong edge, (2) inside the hue range and (3) coloured at all; cleaning
//! moves it towards grey at its own luminance, so the fringe goes without
//! leaving a dark or bright line.
//!
//! Our own design and code (no other editor's code was looked at): an edge
//! mask from the luminance gradient in stops, widened to cover the fringe's
//! width, times soft hue and saturation windows.
//!
//! It runs on the scene light right after the lens corrections
//! (`commands::render_image_tone`, `zoom::render_part`).

use crate::pipeline::SceneImage;
use rayon::prelude::*;

/// Where each colour's hue band sits on the colour wheel (degrees, HSV hue of
/// the sRGB-encoded colour, as on screen): 0..100 on the slider spans
/// `START` .. `START + SPAN`.
pub const PURPLE_START: f32 = 220.0; // blue
pub const GREEN_START: f32 = 60.0; // yellow
pub const SPAN: f32 = 120.0; // purple: blue - violet - magenta; green: yellow - green - cyan

/// Lightroom's default hue ranges.
pub const PURPLE_LO: f32 = 30.0;
pub const PURPLE_HI: f32 = 70.0;
pub const GREEN_LO: f32 = 40.0;
pub const GREEN_HI: f32 = 60.0;

/// How wide (in full-resolution pixels) the band beside an edge that counts as
/// "near the edge" is: fringes are typically 2-8 px wide on a 24-45 MP photo.
const REACH_FULL_PX: f32 = 6.0;

/// Linear Rec.2020 -> linear sRGB (for the hue as it looks on screen).
const REC2020_TO_SRGB: [[f32; 3]; 3] = [[1.660_491, -0.587_641_1, -0.072_849_9], [-0.124_550_5, 1.132_899_9, -0.008_349_4], [-0.018_150_8, -0.100_578_9, 1.118_729_7]];

/// Rec.2020 luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A photo's defringe settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Defringe {
    pub purple: f32,
    pub purple_lo: f32,
    pub purple_hi: f32,
    pub green: f32,
    pub green_lo: f32,
    pub green_hi: f32,
}

impl Defringe {
    pub fn from_settings(s: &crate::pipeline::RenderSettings) -> Defringe {
        Defringe {
            purple: s.defringe_purple,
            purple_lo: s.defringe_purple_hue_lo,
            purple_hi: s.defringe_purple_hue_hi,
            green: s.defringe_green,
            green_lo: s.defringe_green_hue_lo,
            green_hi: s.defringe_green_hue_hi,
        }
    }

    pub fn is_on(&self) -> bool {
        self.purple > 0.0 || self.green > 0.0
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A colour's hue (degrees, 0..360) and saturation (0..1) as it looks on screen:
/// HSV of the sRGB-encoded colour. Scale doesn't change either.
pub fn hue_sat(rec2020: [f32; 3]) -> (f32, f32) {
    let lin: [f32; 3] = std::array::from_fn(|i| {
        (REC2020_TO_SRGB[i][0] * rec2020[0] + REC2020_TO_SRGB[i][1] * rec2020[1] + REC2020_TO_SRGB[i][2] * rec2020[2]).max(0.0)
    });
    let m = lin[0].max(lin[1]).max(lin[2]);
    if m <= 1e-9 {
        return (0.0, 0.0);
    }
    let [r, g, b] = lin.map(|v| crate::pipeline::srgb_encode(v / m));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d <= 1e-6 {
        return (0.0, 0.0);
    }
    let h = if max == r {
        60.0 * ((g - b) / d)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    ((h + 360.0) % 360.0, d / max)
}

/// Where a hue sits on a band starting at `start` (0..100 inside the band;
/// outside it below 0 or above 100, measured the short way round the wheel).
pub fn band_position(hue: f32, start: f32) -> f32 {
    let mid = start + SPAN / 2.0;
    let mut d = hue - mid;
    while d > 180.0 {
        d -= 360.0;
    }
    while d <= -180.0 {
        d += 360.0;
    }
    (d + SPAN / 2.0) / SPAN * 100.0
}

/// How much a band position is inside the range lo..hi, with soft edges.
fn in_range(u: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    const FEATHER: f32 = 8.0;
    smoothstep(lo - FEATHER, lo, u) * (1.0 - smoothstep(hi, hi + FEATHER, u))
}

/// Separable max filter of radius r (in place, through `tmp`).
fn max_filter(v: &mut [f32], tmp: &mut [f32], w: usize, h: usize, r: usize) {
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            row[x] = src[a..=b].iter().fold(0.0f32, |m, &e| m.max(e));
        }
    });
    v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = 0.0f32;
            for yy in a..=b {
                m = m.max(tmp[yy * w + x]);
            }
            *o = m;
        }
    });
}

/// Separable box blur of radius r (in place, through `tmp`).
fn box_blur(v: &mut [f32], tmp: &mut [f32], w: usize, h: usize, r: usize) {
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            row[x] = src[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
        }
    });
    v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0f32;
            for yy in a..=b {
                s += tmp[yy * w + x];
            }
            *o = s / (b - a + 1) as f32;
        }
    });
}

/// How close each pixel is to a strong edge (0..1): the luminance gradient in
/// stops, widened by `reach` pixels and softened.
fn edge_mask(img: &SceneImage, reach: usize) -> Vec<f32> {
    let (w, h) = (img.width, img.height);
    let logy: Vec<f32> = img.px.par_iter().map(|p| (LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2]).max(0.004).log2()).collect();
    let mut edge: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let at = |xx: usize, yy: usize| logy[yy * w + xx];
            let gx = at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y);
            let gy = at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1));
            // Stops across two pixels: half a stop starts to count, two stops fully.
            smoothstep(0.5, 2.0, gx.abs().max(gy.abs()))
        })
        .collect();
    let mut tmp = vec![0.0f32; w * h];
    max_filter(&mut edge, &mut tmp, w, h, reach);
    box_blur(&mut edge, &mut tmp, w, h, (reach / 2).max(1));
    edge
}

/// A hash of every pixel (and the size), to know the same picture again.
pub(crate) fn fingerprint(img: &SceneImage) -> u64 {
    let part = img
        .px
        .par_chunks(8192)
        .map(|c| {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for p in c {
                for v in p {
                    h = (h ^ v.to_bits() as u64).wrapping_mul(0x0100_0000_01b3);
                }
            }
            h
        })
        .collect::<Vec<_>>();
    part.into_iter().fold((img.width as u64) << 32 | img.height as u64, |a, b| a.rotate_left(7).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b)
}

/// The last defringed pictures (a drag's smaller one and the photo's preview), kept while
/// other sliders move: the same picture with the same Defringe settings comes back without
/// the work (about 60 ms at 2400 px, 4 ms to know it again).
type Kept = (u64, Defringe, f32, std::sync::Arc<SceneImage>);
static LAST: std::sync::Mutex<Vec<Kept>> = std::sync::Mutex::new(Vec::new());
const KEEP: usize = 2;

/// The scene with its fringes taken out, or None when Defringe is off.
/// `scale`: the image's pixels per full-resolution pixel (a preview is smaller).
pub fn apply(img: &SceneImage, d: &Defringe, scale: f32) -> Option<std::sync::Arc<SceneImage>> {
    if !d.is_on() || img.width < 3 || img.height < 3 {
        return None;
    }
    let key = fingerprint(img);
    if let Some((.., out)) = LAST.lock().unwrap().iter().find(|(k, kd, ks, _)| *k == key && kd == d && *ks == scale) {
        return Some(out.clone());
    }
    let out = std::sync::Arc::new(compute(img, d, scale));
    let mut kept = LAST.lock().unwrap();
    kept.push((key, *d, scale, out.clone()));
    while kept.len() > KEEP {
        kept.remove(0);
    }
    Some(out)
}

/// `apply` for a part of the photo (zoomed in): not kept, so the whole photo's stays.
pub fn apply_part(img: &SceneImage, d: &Defringe, scale: f32) -> Option<SceneImage> {
    (d.is_on() && img.width >= 3 && img.height >= 3).then(|| compute(img, d, scale))
}

fn compute(img: &SceneImage, d: &Defringe, scale: f32) -> SceneImage {
    let reach = (REACH_FULL_PX * scale.clamp(0.01, 1.0)).round().max(1.0) as usize;
    let edge = edge_mask(img, reach);
    // Amount 0..20: at 6 a pixel fully inside every window is fully cleaned;
    // lower amounts clean part of the way, higher ones also take the pixels
    // only partly in the windows (further from the edge, at the hue range's edges).
    let (kp, kg) = (d.purple.clamp(0.0, 20.0) / 6.0, d.green.clamp(0.0, 20.0) / 6.0);
    let px = img
        .px
        .par_iter()
        .zip(edge.par_iter())
        .map(|(p, &e)| {
            if e <= 1e-3 {
                return *p;
            }
            let (hue, sat) = hue_sat(*p);
            let coloured = smoothstep(0.05, 0.18, sat);
            if coloured <= 0.0 {
                return *p;
            }
            let fp = if kp > 0.0 { in_range(band_position(hue, PURPLE_START), d.purple_lo, d.purple_hi) * kp } else { 0.0 };
            let fg = if kg > 0.0 { in_range(band_position(hue, GREEN_START), d.green_lo, d.green_hi) * kg } else { 0.0 };
            let f = (fp.max(fg) * e * coloured).min(1.0);
            if f <= 0.0 {
                return *p;
            }
            let y = LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2];
            p.map(|c| c + (y - c) * f)
        })
        .collect();
    SceneImage { width: img.width, height: img.height, px, source_long: img.source_long }
}

/// A fringe's colour picked on the photo: which kind it is and the hue range
/// round it (in that band's 0..100), or None for a colour that is too grey.
pub fn range_for(rec2020: [f32; 3]) -> Option<(&'static str, f32, f32)> {
    let (hue, sat) = hue_sat(rec2020);
    if sat < 0.06 {
        return None;
    }
    let up = band_position(hue, PURPLE_START);
    let ug = band_position(hue, GREEN_START);
    // The band it is in, or nearer to.
    let dist = |u: f32| if u < 0.0 { -u } else if u > 100.0 { u - 100.0 } else { 0.0 };
    let (kind, u) = if dist(up) <= dist(ug) { ("purple", up) } else { ("green", ug) };
    let u = u.clamp(0.0, 100.0);
    const HALF: f32 = 12.0;
    let (lo, hi) = ((u - HALF).max(0.0), (u + HALF).min(100.0));
    // Keep the range a usable width at the band's ends.
    let (lo, hi) = if hi - lo < 2.0 * HALF { if lo == 0.0 { (0.0, 2.0 * HALF) } else { (100.0 - 2.0 * HALF, 100.0) } } else { (lo, hi) };
    Some((kind, lo.round(), hi.round()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(purple: f32, green: f32) -> Defringe {
        Defringe { purple, purple_lo: PURPLE_LO, purple_hi: PURPLE_HI, green, green_lo: GREEN_LO, green_hi: GREEN_HI }
    }

    /// A bright sky on the left, a dark branch on the right, with a purple
    /// fringe in the two columns next to the edge; and a purple patch (a
    /// flower) far from any edge.
    fn scene() -> SceneImage {
        let (w, h) = (64usize, 32usize);
        let mut px = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = if x < 32 {
                    [4.0, 4.0, 4.0]
                } else if x < 34 {
                    [0.25, 0.03, 0.5] // purple fringe
                } else {
                    [0.02, 0.02, 0.02]
                };
            }
        }
        SceneImage { width: w, height: h, px, source_long: w }
    }

    #[test]
    fn hues_land_in_their_bands() {
        let (h, _) = hue_sat([0.25, 0.03, 0.5]);
        let u = band_position(h, PURPLE_START);
        assert!((0.0..=100.0).contains(&u), "purple at {h} -> {u}");
        let (h, _) = hue_sat([0.05, 0.4, 0.05]);
        let u = band_position(h, GREEN_START);
        assert!((0.0..=100.0).contains(&u), "green at {h} -> {u}");
        assert!(band_position(h, PURPLE_START) < 0.0 || band_position(h, PURPLE_START) > 100.0);
    }

    #[test]
    fn removes_fringe_at_edges_only() {
        let img = scene();
        // A flat purple patch away from any edge: an image of only that colour.
        let flower = SceneImage { width: 16, height: 16, px: vec![[0.25, 0.03, 0.5]; 256], source_long: 16 };
        let out = apply(&img, &d(10.0, 0.0), 1.0).unwrap();
        let (_, before) = hue_sat(img.px[33]);
        let (_, after) = hue_sat(out.px[33]);
        assert!(after < before * 0.3, "fringe saturation {before} -> {after}");
        // Luminance kept.
        let y = |p: [f32; 3]| LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2];
        assert!((y(out.px[33]) - y(img.px[33])).abs() < 1e-4);
        // The sky and the branch untouched (they are grey).
        assert_eq!(out.px[5], img.px[5]);
        assert_eq!(out.px[60], img.px[60]);
        // The flower, with no edge near it, untouched.
        let f = apply(&flower, &d(20.0, 0.0), 1.0).unwrap();
        assert_eq!(f.px[8 * 16 + 8], flower.px[8 * 16 + 8]);
        // Green Defringe leaves a purple fringe alone.
        let g = apply(&img, &d(0.0, 20.0), 1.0).unwrap();
        assert_eq!(g.px[33], img.px[33]);
        // Off: nothing to do.
        assert!(apply(&img, &d(0.0, 0.0), 1.0).is_none());
    }

    #[test]
    fn picker_finds_the_band() {
        let (kind, lo, hi) = range_for([0.25, 0.03, 0.5]).unwrap();
        assert_eq!(kind, "purple");
        assert!(lo < hi && hi - lo >= 20.0);
        let (kind, ..) = range_for([0.05, 0.4, 0.08]).unwrap();
        assert_eq!(kind, "green");
        assert!(range_for([0.3, 0.3, 0.3]).is_none());
    }
}
