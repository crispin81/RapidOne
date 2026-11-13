//! Clipped highlights: a channel cut off at the sensor's ceiling is estimated again from the
//! channels that weren't, using how the channels relate in the unclipped pixels round it.
//!
//! The general "opposed channels" idea of highlight reconstruction, written for RapidOne (12 Nov).
//! Until then a pixel with two channels near the ceiling was blended towards neutral, which on a
//! glow stick's tip (P1347354: green and blue clipped) still made a white disc with an edge. Now a
//! clipped channel can only go up, from the colour round it, and the colour fades only partly as
//! every channel reaches its ceiling, so the tip turns paler and brighter but stays green.
//!
//! Works on white-balanced camera RGB, before the colour matrix, on the CPU, so the CPU and GPU
//! renders get the same pixels.

use rayon::prelude::*;

/// A channel starts to be estimated at this share of its ceiling...
const START: f32 = 0.90;
/// ...and is fully estimated by this share.
const FULL: f32 = 0.985;
/// Pixels darker than this share of a channel's ceiling are too noisy to learn from.
const LEARN_FLOOR: f32 = 0.2;
/// How far (pixels) a learning pixel must be from any channel that's starting to clip.
const KEEP_CLEAR: usize = 2;
/// Estimate, re-estimate: each pass uses the last pass's values for the clipped channels.
const PASSES: usize = 3;
/// The colour left when every channel is at its ceiling...
const MIN_COLOUR: f32 = 0.35;
/// ...and the headroom (share of the ceiling) an unclipped channel needs for all of it to stay.
const COLOUR_HEADROOM: f32 = 0.3;
/// Where all three channels clipped, the colour comes from nearby pixels with one clipped channel,
/// as log ratios red/green and blue/green. Those differing by more than this count less...
const TINT_SPREAD: f32 = 0.25;
/// ...the tint is used at this strength...
const TINT_STRENGTH: f32 = 0.5;
/// ...and fades out as the neighbours' ratios spread over this range (they disagree on the colour).
const TINT_DISAGREE: (f32, f32) = (0.25, 0.6);

fn ease(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn cbrt0(v: f32) -> f32 {
    if v > 0.0 { v.cbrt() } else { 0.0 }
}

/// Channel `k` as the other two suggest: the cube of the mean of their cube roots.
fn from_others(roots: [f32; 3], k: usize) -> f32 {
    let m = 0.5 * (roots[(k + 1) % 3] + roots[(k + 2) % 3]);
    m * m * m
}

/// How far each channel of `c` is into the estimated range (0 untouched, 1 fully estimated).
fn amounts(c: [f32; 3], clip: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| ease(START * clip[k], FULL * clip[k], c[k]))
}

/// A coarse grid of per-channel corrections, spread from the cells that have some into the cells
/// that don't by averaging down to one cell and blending back up (a push-pull pyramid).
struct Corrections {
    cell: usize,
    w: usize,
    h: usize,
    value: Vec<[f32; 3]>,
}

impl Corrections {
    fn build(cell: usize, w: usize, h: usize, weight: Vec<[f32; 3]>, sum: Vec<[f32; 3]>) -> Self {
        // Down: each level sums 2x2 cells of the one below.
        let mut levels = vec![(w, h, weight, sum)];
        while levels.last().is_some_and(|l| l.0 > 1 || l.1 > 1) {
            let (lw, lh, lwt, lsum) = levels.last().unwrap();
            let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
            let mut nwt = vec![[0.0f32; 3]; nw * nh];
            let mut nsum = vec![[0.0f32; 3]; nw * nh];
            for y in 0..*lh {
                for x in 0..*lw {
                    let (i, j) = (y * lw + x, (y / 2) * nw + x / 2);
                    for k in 0..3 {
                        nwt[j][k] += lwt[i][k];
                        nsum[j][k] += lsum[i][k];
                    }
                }
            }
            levels.push((nw, nh, nwt, nsum));
        }
        // Up: the coarse level's values, replaced by a cell's own average as far as it has enough
        // learning pixels (a quarter of a full cell, counting grows 4x a level).
        let top = levels.last().unwrap();
        let mut value: Vec<[f32; 3]> =
            top.2.iter().zip(&top.3).map(|(wt, s)| std::array::from_fn(|k| if wt[k] > 0.0 { s[k] / wt[k] } else { 0.0 })).collect();
        let mut enough = 0.25 * (cell * cell) as f32 * 4f32.powi(levels.len() as i32 - 1);
        let mut vw = top.0;
        for (lw, lh, lwt, lsum) in levels.iter().rev().skip(1) {
            enough /= 4.0;
            let coarse = &value;
            let (cw, ch) = (vw, coarse.len() / vw);
            let next: Vec<[f32; 3]> = (0..lw * lh)
                .map(|i| {
                    let (x, y) = (i % lw, i / lw);
                    let up = sample(coarse, cw, ch, (x as f32 + 0.5) * cw as f32 / *lw as f32 - 0.5, (y as f32 + 0.5) * ch as f32 / *lh as f32 - 0.5);
                    std::array::from_fn(|k| {
                        if lwt[i][k] > 0.0 {
                            let own = lsum[i][k] / lwt[i][k];
                            up[k] + (own - up[k]) * (lwt[i][k] / enough).min(1.0)
                        } else {
                            up[k]
                        }
                    })
                })
                .collect();
            value = next;
            vw = *lw;
        }
        Corrections { cell, w, h, value }
    }

    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let c = self.cell as f32;
        sample(&self.value, self.w, self.h, (x as f32 + 0.5) / c - 0.5, (y as f32 + 0.5) / c - 0.5)
    }
}

/// Bilinear sample of a `w` x `h` grid, clamped at its edges.
fn sample(g: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (a, b, c, d) = (g[y0 * w + x0], g[y0 * w + x1], g[y1 * w + x0], g[y1 * w + x1]);
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Which channels are starting to clip anywhere within `KEEP_CLEAR` pixels (bit k for channel k,
/// bit 3 where some pixel nearby has all three starting to clip).
fn near_clipping(width: usize, height: usize, cam: &[[f32; 3]], clip: [f32; 3]) -> Vec<u8> {
    let own: Vec<u8> = cam
        .par_iter()
        .map(|c| {
            let b = (0..3).fold(0u8, |b, k| if c[k] >= START * clip[k] { b | 1 << k } else { b });
            if b == 7 { 15 } else { b }
        })
        .collect();
    let r = KEEP_CLEAR;
    let across: Vec<u8> = (0..width * height)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % width, i / width);
            (x.saturating_sub(r)..(x + r + 1).min(width)).fold(0, |b, xx| b | own[y * width + xx])
        })
        .collect();
    (0..width * height)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % width, i / width);
            (y.saturating_sub(r)..(y + r + 1).min(height)).fold(0, |b, yy| b | across[yy * width + x])
        })
        .collect()
}

/// `cam` with its clipped channels estimated again, or `None` when no channel comes near its
/// ceiling (the picture is then used as it is). `clip` is each channel's ceiling.
pub fn rebuild(width: usize, height: usize, cam: &[[f32; 3]], clip: [f32; 3]) -> Option<Vec<[f32; 3]>> {
    if width == 0 || height == 0 || !cam.par_iter().any(|c| (0..3).any(|k| c[k] >= START * clip[k])) {
        return None;
    }
    let near = near_clipping(width, height, cam, clip);

    // Learn, per coarse cell and channel, that channel's ratio to what the other two suggest (as a log),
    // in pixels where it's well clear of clipping and bright enough to trust (brighter counts more).
    // A ratio, not a difference: it holds as the light gets brighter towards a highlight's centre. A
    // difference (12 Nov, first version) let the estimated green of P1347354's glow-stick LED fall
    // below its clipped blue, a cool lavender ring inside the white.
    let cell = (width.max(height) / 1024).clamp(1, 8);
    let corrections = learn(width, height, cell, |i| {
        let c = cam[i];
        let roots = c.map(cbrt0);
        let (mut wt, mut v) = ([0.0f32; 3], [0.0f32; 3]);
        for k in 0..3 {
            let share = c[k] / clip[k];
            let others = from_others(roots, k);
            if near[i] & (1 << k) == 0 && share > LEARN_FLOOR && others > 0.0 {
                wt[k] = share * share;
                v[k] = (c[k] / others).ln();
            }
        }
        (wt, v)
    });

    let mut out: Vec<[f32; 3]> = (0..width * height)
        .into_par_iter()
        .map(|i| estimate(cam[i], clip, corrections.at(i % width, i / width)))
        .collect();

    // All three clipped: nothing in the pixel knows its colour. Take it from the pixels round the
    // blown area that still had two channels (only one clipped), spread inwards, and tint a neutral
    // at least as bright as recorded with it. With no such neighbours it stays neutral.
    let blown = |c: [f32; 3]| (0..3).map(|k| ease(START * clip[k], FULL * clip[k], c[k])).fold(1.0f32, f32::min);
    if cam.par_iter().any(|&c| blown(c) > 0.0) {
        let tints = learn(width, height, cell, |i| {
            let p = out[i];
            if near[i] & 8 != 0 || near[i] & 7 == 0 || (0..3).filter(|&k| cam[i][k] >= START * clip[k]).count() != 1 || p.iter().any(|&v| v <= 0.0) {
                return ([0.0; 3], [0.0; 3]);
            }
            let (rg, bg) = ((p[0] / p[1]).ln(), (p[2] / p[1]).ln());
            let spread = (rg.max(bg).max(0.0) - rg.min(bg).min(0.0)) / TINT_SPREAD;
            let w = 1.0 / (1.0 + spread * spread);
            ([w, w, 0.0], [rg, bg, 0.0])
        });
        out.par_iter_mut().enumerate().for_each(|(i, p)| {
            let c = cam[i];
            let full = blown(c);
            if full <= 0.0 {
                return;
            }
            let [rg, bg, _] = tints.at(i % width, i / width);
            let spread = rg.max(bg).max(0.0) - rg.min(bg).min(0.0);
            let strength = TINT_STRENGTH * (1.0 - ease(TINT_DISAGREE.0, TINT_DISAGREE.1, spread));
            let tint = [(rg * strength).exp(), 1.0, (bg * strength).exp()];
            let level = (0..3).map(|k| c[k] / tint[k]).fold((p[0] + p[1] + p[2]) / (tint[0] + tint[1] + tint[2]), f32::max);
            *p = std::array::from_fn(|k| p[k] + (level * tint[k] - p[k]) * full);
        });
    }
    Some(out)
}

/// Per coarse cell, the weighted average of what `sample` gives each pixel (weight, value per
/// channel), spread into the cells without any.
fn learn(width: usize, height: usize, cell: usize, sample: impl Fn(usize) -> ([f32; 3], [f32; 3]) + Sync) -> Corrections {
    let (gw, gh) = (width.div_ceil(cell), height.div_ceil(cell));
    let (weight, sum): (Vec<[f32; 3]>, Vec<[f32; 3]>) = (0..gw * gh)
        .into_par_iter()
        .map(|g| {
            let (cx, cy) = (g % gw, g / gw);
            let (mut wt, mut sum) = ([0.0f32; 3], [0.0f32; 3]);
            for y in cy * cell..((cy + 1) * cell).min(height) {
                for x in cx * cell..((cx + 1) * cell).min(width) {
                    let (w, v) = sample(y * width + x);
                    for k in 0..3 {
                        wt[k] += w[k];
                        sum[k] += w[k] * v[k];
                    }
                }
            }
            (wt, sum)
        })
        .unzip();
    Corrections::build(cell, gw, gh, weight, sum)
}

/// One pixel with its clipping channels estimated from the others times the local ratio `fix` (a log).
fn estimate(c: [f32; 3], clip: [f32; 3], fix: [f32; 3]) -> [f32; 3] {
    let amount = amounts(c, clip);
    if amount.iter().all(|&a| a == 0.0) {
        return c;
    }
    // Never below what was recorded; repeated with the new values so two clipped channels settle.
    let mut p = c;
    for _ in 0..PASSES {
        let roots = p.map(cbrt0);
        p = std::array::from_fn(|k| {
            if amount[k] == 0.0 {
                return c[k];
            }
            let estimate = (from_others(roots, k) * fix[k].exp()).max(c[k]);
            c[k] + (estimate - c[k]) * amount[k]
        });
    }
    // The colour fades towards the pixel's own grey as the unclipped channels run out of headroom,
    // but never below MIN_COLOUR of it, and nothing drops below its recording.
    let headroom = (0..3).filter(|&k| c[k] < FULL * clip[k]).map(|k| 1.0 - c[k] / clip[k]).fold(0.0f32, f32::max);
    let keep = MIN_COLOUR + (1.0 - MIN_COLOUR) * ease(0.0, COLOUR_HEADROOM, headroom);
    let grey = (p[0] + p[1] + p[2]) / 3.0;
    std::array::from_fn(|k| {
        if amount[k] == 0.0 {
            return p[k];
        }
        let faded = (grey + (p[k] - grey) * keep).max(c[k]);
        c[k] + (faded - c[k]) * amount[k]
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A glow stick: green and blue clipped in a disc, green round it with blue and red lower.
    fn stick() -> (usize, usize, Vec<[f32; 3]>, [f32; 3]) {
        let clip = [2.33, 1.0, 1.46];
        let (w, h) = (64, 64);
        let cam = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 32.0, (i / w) as f32 - 32.0);
                let r = (x * x + y * y).sqrt();
                // Brightness falls from 3x the green ceiling at the centre to 0.3 at r = 30.
                let b = 3.0 * (-r / 12.0).exp() + 0.05;
                let v = [0.35 * b, b, 0.6 * b];
                std::array::from_fn(|k| v[k].min(clip[k]))
            })
            .collect();
        (w, h, cam, clip)
    }

    #[test]
    fn untouched_without_highlights() {
        let clip = [2.0, 1.0, 1.5];
        let cam = vec![[0.5, 0.4, 0.3]; 16];
        assert!(rebuild(4, 4, &cam, clip).is_none());
    }

    #[test]
    fn clipped_channels_only_go_up_and_keep_some_colour() {
        let (w, h, cam, clip) = stick();
        let out = rebuild(w, h, &cam, clip).unwrap();
        for (c, o) in cam.iter().zip(&out) {
            for k in 0..3 {
                assert!(o[k] >= c[k] - 1e-6, "dropped below its recording: {c:?} -> {o:?}");
            }
        }
        // The centre (green and blue clipped) still leans green, not white.
        let centre = out[32 * w + 32];
        assert!(centre[1] > centre[0] * 1.05, "centre went grey: {centre:?}");
    }

    #[test]
    fn one_clipped_channel_with_headroom_keeps_its_colour() {
        // A sunset's sky: red at its ceiling, green at 73%, blue at 25%, the same everywhere.
        let clip = [2.33, 1.0, 1.46];
        let sky = [2.33, 0.73, 0.25 * 1.46];
        let cam = vec![sky; 64];
        let out = rebuild(8, 8, &cam, clip).unwrap();
        // Green and blue as recorded; red can only rise.
        assert_eq!(out[27][1], sky[1]);
        assert_eq!(out[27][2], sky[2]);
        assert!(out[27][0] >= sky[0]);
    }
}
