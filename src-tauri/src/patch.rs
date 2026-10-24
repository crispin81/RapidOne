//! Patch (Photoshop's way): an area is drawn round and dragged onto clean texture; the area takes
//! what's there. Two modes, switched on the tool:
//! - Heal: the source's texture, on the tone and colour from round the area (so it picks up the
//!   light where it lands): the source divided by its own blur, times the blur of the area's
//!   surroundings, in linear light (Retouch's Patch does the same with frequency separation).
//! - Clone: the source exactly, pixel for pixel.
//! Either way the edge is feathered.
//!
//! Non-destructive, as Remove's removals (and kept in the same list, `removal::Spot`): a patch is
//! its outline, where it copies from, its mode and feather, all as fractions of the upright,
//! uncropped photo; it's worked out again on every render, before the removals and every other
//! tool, so the sliders all work on top. The pixels are read through `sample` (fractions of the
//! whole photo), so a part of the photo (zoomed-in detail, `zoom::render_part`) can take its
//! source from outside the part.

use crate::pipeline::{Frame, SceneImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Heal,
    Clone,
}

/// Marks a patch in the list it shares with the removals (always "patch").
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Patch,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub kind: Kind,
    /// The area fixed: its outline, [x, y] fractions of the upright, uncropped photo.
    pub polygon: Vec<[f32; 2]>,
    /// Where its new content comes from: the outline moved by this (fractions).
    pub offset: [f32; 2],
    pub mode: Mode,
    /// Edge softness, as a share of the area's size (0: the default for the mode).
    #[serde(default)]
    pub feather: f32,
}

/// Broad tone is everything coarser than this share of the area's size (Heal).
const TONE_SCALE: f32 = 0.25;

fn default_feather(mode: Mode) -> f32 {
    match mode {
        Mode::Heal => 0.08,
        Mode::Clone => 0.15,
    }
}

/// The photo with its patches applied; None if it has none here. `frame`: a part of the photo
/// (`None`: the whole photo, `img`'s own pixels). `sample(x, y)`: the photo's scene light at a point
/// given as fractions of the whole photo, where sources are read.
pub fn apply(
    img: &SceneImage,
    spots: &[crate::removal::Spot],
    frame: Option<&Frame>,
    sample: &(dyn Fn(f32, f32) -> [f32; 3] + Sync),
) -> Option<SceneImage> {
    let patches: Vec<&Patch> = spots.iter().filter_map(|s| s.patch()).collect();
    if patches.is_empty() {
        return None;
    }
    let (w, h) = (img.width, img.height);
    let (fw, fh, ox, oy) = match frame {
        Some(f) => (f.full_w, f.full_h, f.x0, f.y0),
        None => (w as f32, h as f32, 0.0, 0.0),
    };
    let mut out = img.clone();
    for p in patches {
        if p.polygon.len() < 3 {
            continue;
        }
        let pts: Vec<[f32; 2]> = p.polygon.iter().map(|[x, y]| [x * fw - ox, y * fh - oy]).collect();
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for [x, y] in &pts {
            bx0 = bx0.min(*x);
            by0 = by0.min(*y);
            bx1 = bx1.max(*x);
            by1 = by1.max(*y);
        }
        let size = (bx1 - bx0).max(by1 - by0).max(1.0);
        let feather_px = ((if p.feather > 0.0 { p.feather } else { default_feather(p.mode) }) * size).max(1.0);
        let tone_px = (TONE_SCALE * size).max(2.0);
        let margin = 2.0 * feather_px + if p.mode == Mode::Heal { 2.5 * tone_px } else { 0.0 } + 2.0;
        let (wx0, wy0) = ((bx0 - margin).floor() as i64, (by0 - margin).floor() as i64);
        let (wx1, wy1) = ((bx1 + margin).ceil() as i64, (by1 + margin).ceil() as i64);
        // Nothing of it in this part of the photo: skipped.
        if wx1 <= 0 || wy1 <= 0 || wx0 >= w as i64 || wy0 >= h as i64 {
            continue;
        }
        let (ww, wh) = ((wx1 - wx0) as usize, (wy1 - wy0) as usize);
        if ww * wh > 40_000_000 {
            continue; // absurdly large for this resolution
        }
        let frac = |i: usize| -> (f32, f32) {
            let (x, y) = ((i % ww) as f32 + wx0 as f32 + 0.5, (i / ww) as f32 + wy0 as f32 + 0.5);
            ((x + ox) / fw, (y + oy) / fh)
        };
        let dest: Vec<[f32; 3]> = (0..ww * wh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = frac(i);
                let (lx, ly) = ((i % ww) as i64 + wx0, (i / ww) as i64 + wy0);
                // Inside this image: its own pixel (already patched by earlier patches); else the photo.
                if lx >= 0 && ly >= 0 && (lx as usize) < w && (ly as usize) < h {
                    out.px[ly as usize * w + lx as usize]
                } else {
                    sample(x, y)
                }
            })
            .collect();
        let source: Vec<[f32; 3]> = (0..ww * wh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = frac(i);
                sample(x + p.offset[0], y + p.offset[1])
            })
            .collect();
        // The area (1 inside), and its feathered edge.
        let inside: Vec<f32> = (0..ww * wh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % ww) as f32 + wx0 as f32 + 0.5, (i / ww) as f32 + wy0 as f32 + 0.5);
                if contains(&pts, x, y) { 1.0 } else { 0.0 }
            })
            .collect();
        let mut alpha = inside.clone();
        gauss(&mut alpha, ww, wh, feather_px / 2.0);
        let result: Vec<[f32; 3]> = match p.mode {
            Mode::Clone => source,
            Mode::Heal => {
                // The surroundings' broad tone (the area left out), and the source's.
                let outside: Vec<f32> = inside.iter().map(|m| 1.0 - m).collect();
                let mut weight = outside.clone();
                gauss(&mut weight, ww, wh, tone_px);
                let mut tone = [0usize, 1, 2].map(|c| {
                    let mut ch: Vec<f32> = dest.iter().zip(&outside).map(|(d, o)| d[c] * o).collect();
                    gauss(&mut ch, ww, wh, tone_px);
                    ch
                });
                for c in 0..3 {
                    for (t, wt) in tone[c].iter_mut().zip(&weight) {
                        *t /= wt.max(1e-4);
                    }
                }
                let src_tone = [0usize, 1, 2].map(|c| {
                    let mut ch: Vec<f32> = source.iter().map(|s| s[c]).collect();
                    gauss(&mut ch, ww, wh, tone_px);
                    ch
                });
                (0..ww * wh)
                    .into_par_iter()
                    .map(|i| {
                        std::array::from_fn(|c| {
                            let ratio = (source[i][c].max(0.0) + 1e-4) / (src_tone[c][i].max(0.0) + 1e-4);
                            tone[c][i].max(0.0) * ratio.clamp(0.0, 8.0)
                        })
                    })
                    .collect()
            }
        };
        // Into the image, where the window lies on it.
        let (y0, y1) = (wy0.max(0) as usize, (wy1 as usize).min(h));
        let (x0, x1) = (wx0.max(0) as usize, (wx1 as usize).min(w));
        out.px[y0 * w..y1 * w].par_chunks_mut(w).enumerate().for_each(|(r, row)| {
            let ly = (y0 + r) as i64 - wy0;
            for (x, px) in row.iter_mut().enumerate().take(x1).skip(x0) {
                let i = ly as usize * ww + (x as i64 - wx0) as usize;
                let a = alpha[i].clamp(0.0, 1.0);
                if a > 0.0 {
                    *px = std::array::from_fn(|c| dest[i][c] + (result[i][c] - dest[i][c]) * a);
                }
            }
        });
    }
    Some(out)
}

/// Whether (x, y) is inside the outline (even-odd).
fn contains(pts: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let ([xi, yi], [xj, yj]) = (pts[i], pts[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A Gaussian blur of about `sigma` (three box blurs), in place.
fn gauss(buf: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma < 0.5 {
        return;
    }
    // Three boxes of this radius approximate the Gaussian.
    let r = ((((12.0 * sigma * sigma / 3.0) + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
    for _ in 0..3 {
        box_rows(buf, w, h, r);
        let mut t = transpose(buf, w, h);
        box_rows(&mut t, h, w, r);
        buf.copy_from_slice(&transpose(&t, h, w));
    }
}

fn box_rows(buf: &mut [f32], w: usize, _h: usize, r: usize) {
    buf.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let n = w as i64;
        let at = |i: i64| src[i.clamp(0, n - 1) as usize];
        let mut acc: f32 = (-(r as i64)..=r as i64).map(at).sum();
        let k = 1.0 / (2 * r + 1) as f32;
        for (x, v) in row.iter_mut().enumerate() {
            *v = acc * k;
            let x = x as i64;
            acc += at(x + r as i64 + 1) - at(x - r as i64);
        }
    });
}

fn transpose(buf: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut t = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            t[x * h + y] = buf[y * w + x];
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::removal::Spot;

    fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> SceneImage {
        SceneImage { width: w, height: h, px: (0..w * h).map(|i| f(i % w, i / w)).collect(), source_long: w.max(h) }
    }

    fn sampler(im: &SceneImage) -> impl Fn(f32, f32) -> [f32; 3] + Sync + '_ {
        move |x: f32, y: f32| {
            let px = ((x * im.width as f32) as i64).clamp(0, im.width as i64 - 1) as usize;
            let py = ((y * im.height as f32) as i64).clamp(0, im.height as i64 - 1) as usize;
            im.px[py * im.width + px]
        }
    }

    fn square(x0: f32, y0: f32, s: f32, offset: [f32; 2], mode: Mode) -> Spot {
        Spot::Patch(Patch { kind: Kind::Patch, polygon: vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]], offset, mode, feather: 0.02 })
    }

    #[test]
    fn clone_copies_the_source_and_leaves_the_rest() {
        // Left half dark, right half bright: the dark square at the left gets the bright source.
        let im = img(200, 100, |x, _| if x < 100 { [0.1; 3] } else { [0.8; 3] });
        // The square: x 40-80, y 30-50 (pixels); the middle of it is (60, 40).
        let out = apply(&im, &[square(0.2, 0.3, 0.2, [0.5, 0.0], Mode::Clone)], None, &sampler(&im)).unwrap();
        let mid = out.px[40 * 200 + 60];
        assert!((mid[0] - 0.8).abs() < 0.02, "inside cloned: {mid:?}");
        assert!((out.px[50 * 200 + 5][0] - 0.1).abs() < 1e-6, "outside untouched");
    }

    #[test]
    fn heal_keeps_the_destination_tone_with_the_source_texture() {
        // Destination: flat 0.2 with a dark spot; source: flat 0.6 with fine stripes. Healed, the area
        // has the stripes but the destination's brightness (about 0.2), not the source's 0.6.
        let im = img(300, 150, |x, y| {
            if x < 150 {
                if (60..80).contains(&x) && (60..80).contains(&y) { [0.02; 3] } else { [0.2; 3] }
            } else if x % 4 < 2 {
                [0.66; 3]
            } else {
                [0.54; 3]
            }
        });
        // The square: x 54-90, y 57-75 (pixels), round the dark spot (60-80, 60-80).
        let spot = square(0.18, 0.38, 0.12, [0.5, 0.0], Mode::Heal);
        let out = apply(&im, &[spot], None, &sampler(&im)).unwrap();
        let v: Vec<f32> = (64..76).map(|x| out.px[66 * 300 + x][0]).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean - 0.2).abs() < 0.04, "healed tone {mean}");
        let spread = v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread > 0.02, "texture carried: {v:?}");
    }

    #[test]
    fn a_removal_and_a_patch_share_the_list() {
        let json = r#"[{"file":"a.png","crop":[0,0,1,1],"area":[0,0,1,1],"ev":0},
            {"kind":"patch","polygon":[[0,0],[1,0],[1,1]],"offset":[0.1,0],"mode":"clone"}]"#;
        let spots: Vec<Spot> = serde_json::from_str(json).unwrap();
        assert!(spots[0].removal().is_some() && spots[0].patch().is_none());
        assert!(spots[1].patch().is_some() && spots[1].removal().is_none());
    }
}
