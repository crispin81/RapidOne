//! Demosaicing: turning the sensor's one-colour-per-pixel mosaic into full
//! colour at every pixel.
//!
//! * `bilinear` - the simplest method (average the nearest same-colour
//!   samples). Soft, and makes false colour on fine detail. Kept as the
//!   baseline the better methods are measured against, and for image borders.
//! * `rcd` - Ratio Corrected Demosaicing (Luis Sanz Rodríguez, 2017), written
//!   here from the algorithm's published description. It decides per pixel
//!   whether detail runs vertically or horizontally (and, for red/blue, along
//!   which diagonal) and interpolates along it, using colour *differences*
//!   and low-pass *ratios* so edges stay sharp without colour fringes.
//! * `suppress_false_colour` - a clean-up pass for fine repeating patterns
//!   (checked fabric, mesh): median-filters the colour-difference channels,
//!   which removes isolated colour speckles while leaving brightness detail.
//!
//! All functions take white-balanced sensor data scaled to 0..1 (Bayer only;
//! X-Trans uses `xtrans_average` until Markesteijn is written).

use rayon::prelude::*;

const EPS: f32 = 1e-5;

/// Three full-resolution planes: red, green, blue.
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub r: Vec<f32>,
    pub g: Vec<f32>,
    pub b: Vec<f32>,
}

impl Planes {
    fn new(width: usize, height: usize) -> Self {
        let n = width * height;
        Self { width, height, r: vec![0.0; n], g: vec![0.0; n], b: vec![0.0; n] }
    }

    fn plane_mut(&mut self, c: usize) -> &mut Vec<f32> {
        match c {
            0 => &mut self.r,
            1 => &mut self.g,
            _ => &mut self.b,
        }
    }
}

/// Colour of the sensor pixel at (row, col): 0 red, 1 green, 2 blue.
pub trait CfaPattern: Sync {
    fn color_at(&self, row: usize, col: usize) -> usize;
}

impl CfaPattern for rawler::CFA {
    fn color_at(&self, row: usize, col: usize) -> usize {
        rawler::CFA::color_at(self, row, col).min(2)
    }
}

/// Bayer pattern given by its top-left 2x2 block, e.g. RGGB = [[0, 1], [1, 2]].
pub struct Bayer(pub [[usize; 2]; 2]);

impl CfaPattern for Bayer {
    fn color_at(&self, row: usize, col: usize) -> usize {
        self.0[row % 2][col % 2]
    }
}

/// Average of the same-colour samples in a (2r+1)x(2r+1) window. Works for
/// any pattern (Bayer with r = 1, X-Trans with r = 2).
fn window_average(cfa: &[f32], w: usize, h: usize, pat: &dyn CfaPattern, r: usize) -> Planes {
    let mut out = Planes::new(w, h);
    // Each row written straight into the three planes, on all cores (copied in on one core before).
    let Planes { r: pr, g: pg, b: pb, .. } = &mut out;
    pr.par_chunks_mut(w).zip(pg.par_chunks_mut(w)).zip(pb.par_chunks_mut(w)).enumerate().for_each(|(y, ((rr, rg), rb))| {
        for x in 0..w {
            let mut sum = [0.0f32; 3];
            let mut n = [0u32; 3];
            for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                    let c = pat.color_at(yy, xx);
                    sum[c] += cfa[yy * w + xx];
                    n[c] += 1;
                }
            }
            let own = pat.color_at(y, x);
            let px: [f32; 3] = std::array::from_fn(|c| if c == own { cfa[y * w + x] } else { sum[c] / n[c].max(1) as f32 });
            (rr[x], rg[x], rb[x]) = (px[0], px[1], px[2]);
        }
    });
    out
}

pub fn bilinear(cfa: &[f32], w: usize, h: usize, pat: &dyn CfaPattern) -> Planes {
    window_average(cfa, w, h, pat, 1)
}

/// Stand-in for X-Trans until Markesteijn is implemented.
pub fn xtrans_average(cfa: &[f32], w: usize, h: usize, pat: &dyn CfaPattern) -> Planes {
    window_average(cfa, w, h, pat, 2)
}

fn sq(v: f32) -> f32 {
    v * v
}

/// Runs `f(y, x)` for every pixel with `margin` <= x < w - margin (same for
/// y), in parallel by row, writing into `out`.
fn par_fill(out: &mut [f32], w: usize, h: usize, margin: usize, f: impl Fn(usize, usize) -> f32 + Sync) {
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y < margin || y + margin >= h {
            return;
        }
        for x in margin..w - margin {
            row[x] = f(y, x);
        }
    });
}

/// RCD demosaic for Bayer sensors. Pixels within 4 of the edge use bilinear.
pub fn rcd(cfa: &[f32], w: usize, h: usize, pat: &dyn CfaPattern) -> Planes {
    let mut out = bilinear(cfa, w, h, pat);
    if w < 16 || h < 16 {
        return out;
    }
    let (w1, w2, w3, w4) = (w, 2 * w, 3 * w, 4 * w);
    let n = w * h;
    let is_green = |y: usize, x: usize| pat.color_at(y, x) == 1;

    // Step 1: vertical vs horizontal. A high-order high-pass along each axis;
    // where it's strong vertically, detail runs horizontally, and vice versa.
    let mut hp_v = vec![0.0f32; n];
    let mut hp_h = vec![0.0f32; n];
    par_fill(&mut hp_v, w, h, 3, |y, x| {
        let i = y * w + x;
        sq((cfa[i - w3] - cfa[i - w1] - cfa[i + w1] + cfa[i + w3]) - 3.0 * (cfa[i - w2] + cfa[i + w2]) + 6.0 * cfa[i])
    });
    par_fill(&mut hp_h, w, h, 3, |y, x| {
        let i = y * w + x;
        sq((cfa[i - 3] - cfa[i - 1] - cfa[i + 1] + cfa[i + 3]) - 3.0 * (cfa[i - 2] + cfa[i + 2]) + 6.0 * cfa[i])
    });
    let mut vh_dir = vec![0.5f32; n];
    par_fill(&mut vh_dir, w, h, 4, |y, x| {
        let i = y * w + x;
        let v = (hp_v[i - w1] + hp_v[i] + hp_v[i + w1]).max(EPS);
        let hz = (hp_h[i - 1] + hp_h[i] + hp_h[i + 1]).max(EPS);
        v / (v + hz)
    });
    drop((hp_v, hp_h));

    // Step 2: low-pass of all local samples, used for the ratio correction.
    let mut lpf = vec![0.0f32; n];
    par_fill(&mut lpf, w, h, 2, |y, x| {
        let i = y * w + x;
        cfa[i]
            + 0.5 * (cfa[i - w1] + cfa[i + w1] + cfa[i - 1] + cfa[i + 1])
            + 0.25 * (cfa[i - w1 - 1] + cfa[i - w1 + 1] + cfa[i + w1 - 1] + cfa[i + w1 + 1])
    });

    // Picks the direction weight: the pixel's own, unless its four diagonal
    // neighbours agree on a more decisive one (steadier on noisy detail).
    let refine = |dir: &[f32], i: usize| {
        let own = dir[i];
        let around = 0.25 * (dir[i - w1 - 1] + dir[i - w1 + 1] + dir[i + w1 - 1] + dir[i + w1 + 1]);
        if (0.5 - own).abs() < (0.5 - around).abs() {
            around
        } else {
            own
        }
    };

    // Step 3: green at red and blue pixels.
    let mut green = out.g.clone();
    par_fill(&mut green, w, h, 4, |y, x| {
        let i = y * w + x;
        if is_green(y, x) {
            return cfa[i];
        }
        let c = cfa[i];
        let n_grad = EPS
            + (cfa[i - w1] - cfa[i + w1]).abs()
            + (c - cfa[i - w2]).abs()
            + (cfa[i - w1] - cfa[i - w3]).abs()
            + (cfa[i - w2] - cfa[i - w4]).abs();
        let s_grad = EPS
            + (cfa[i + w1] - cfa[i - w1]).abs()
            + (c - cfa[i + w2]).abs()
            + (cfa[i + w1] - cfa[i + w3]).abs()
            + (cfa[i + w2] - cfa[i + w4]).abs();
        let w_grad = EPS
            + (cfa[i - 1] - cfa[i + 1]).abs()
            + (c - cfa[i - 2]).abs()
            + (cfa[i - 1] - cfa[i - 3]).abs()
            + (cfa[i - 2] - cfa[i - 4]).abs();
        let e_grad = EPS
            + (cfa[i + 1] - cfa[i - 1]).abs()
            + (c - cfa[i + 2]).abs()
            + (cfa[i + 1] - cfa[i + 3]).abs()
            + (cfa[i + 2] - cfa[i + 4]).abs();
        // Neighbouring green, scaled by how the local low-pass changes.
        let n_est = cfa[i - w1] * 2.0 * lpf[i] / (EPS + lpf[i] + lpf[i - w2]);
        let s_est = cfa[i + w1] * 2.0 * lpf[i] / (EPS + lpf[i] + lpf[i + w2]);
        let w_est = cfa[i - 1] * 2.0 * lpf[i] / (EPS + lpf[i] + lpf[i - 2]);
        let e_est = cfa[i + 1] * 2.0 * lpf[i] / (EPS + lpf[i] + lpf[i + 2]);
        // Each side weighted by the *other* side's gradient: the smoother side wins.
        let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
        let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);
        let d = refine(&vh_dir, i);
        (d * h_est + (1.0 - d) * v_est).max(0.0)
    });
    out.g.copy_from_slice(&green);
    drop(lpf);

    // Step 4a: which diagonal the detail runs along, for red/blue pixels.
    let mut hp_p = vec![0.0f32; n];
    let mut hp_q = vec![0.0f32; n];
    par_fill(&mut hp_p, w, h, 3, |y, x| {
        let i = y * w + x;
        sq((cfa[i - w3 - 3] - cfa[i - w1 - 1] - cfa[i + w1 + 1] + cfa[i + w3 + 3])
            - 3.0 * (cfa[i - w2 - 2] + cfa[i + w2 + 2])
            + 6.0 * cfa[i])
    });
    par_fill(&mut hp_q, w, h, 3, |y, x| {
        let i = y * w + x;
        sq((cfa[i - w3 + 3] - cfa[i - w1 + 1] - cfa[i + w1 - 1] + cfa[i + w3 - 3])
            - 3.0 * (cfa[i - w2 + 2] + cfa[i + w2 - 2])
            + 6.0 * cfa[i])
    });
    let mut pq_dir = vec![0.5f32; n];
    par_fill(&mut pq_dir, w, h, 4, |y, x| {
        let i = y * w + x;
        let p = (hp_p[i - w1 - 1] + hp_p[i] + hp_p[i + w1 + 1]).max(EPS);
        let q = (hp_q[i - w1 + 1] + hp_q[i] + hp_q[i + w1 - 1]).max(EPS);
        p / (p + q)
    });
    drop((hp_p, hp_q));

    // Step 4b: the missing one of red/blue at blue/red pixels, from the
    // diagonal neighbours' colour differences (colour minus green).
    // A copy, so the red and blue planes can be written while green is read.
    let g = &out.g.clone();
    for c in [0usize, 2] {
        let mut plane = if c == 0 { out.r.clone() } else { out.b.clone() };
        par_fill(&mut plane, w, h, 4, |y, x| {
            let i = y * w + x;
            let own = pat.color_at(y, x);
            if own == c {
                return cfa[i];
            }
            if own == 1 {
                return 0.0; // filled in step 4c
            }
            // Diagonal neighbours of a red pixel are blue and vice versa.
            let nw_grad = EPS + (cfa[i - w1 - 1] - cfa[i + w1 + 1]).abs() + (cfa[i - w1 - 1] - cfa[i - w3 - 3]).abs() + (g[i] - g[i - w2 - 2]).abs();
            let ne_grad = EPS + (cfa[i - w1 + 1] - cfa[i + w1 - 1]).abs() + (cfa[i - w1 + 1] - cfa[i - w3 + 3]).abs() + (g[i] - g[i - w2 + 2]).abs();
            let sw_grad = EPS + (cfa[i + w1 - 1] - cfa[i - w1 + 1]).abs() + (cfa[i + w1 - 1] - cfa[i + w3 - 3]).abs() + (g[i] - g[i + w2 - 2]).abs();
            let se_grad = EPS + (cfa[i + w1 + 1] - cfa[i - w1 - 1]).abs() + (cfa[i + w1 + 1] - cfa[i + w3 + 3]).abs() + (g[i] - g[i + w2 + 2]).abs();
            let nw_est = cfa[i - w1 - 1] - g[i - w1 - 1];
            let ne_est = cfa[i - w1 + 1] - g[i - w1 + 1];
            let sw_est = cfa[i + w1 - 1] - g[i + w1 - 1];
            let se_est = cfa[i + w1 + 1] - g[i + w1 + 1];
            let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
            let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);
            let d = refine(&pq_dir, i);
            (g[i] + d * q_est + (1.0 - d) * p_est).max(0.0)
        });
        // Keep the bilinear values at green pixels for now; step 4c replaces them.
        let target = out.plane_mut(c);
        target.par_chunks_mut(w).enumerate().filter(|(y, _)| (4..h - 4).contains(y)).for_each(|(y, row)| {
            for x in 4..w - 4 {
                if pat.color_at(y, x) != 1 {
                    row[x] = plane[y * w + x];
                }
            }
        });
    }

    // Step 4c: red and blue at green pixels, from the cardinal neighbours'
    // colour differences, now that red and blue exist at every non-green pixel.
    let (r_prev, b_prev) = (out.r.clone(), out.b.clone());
    for c in [0usize, 2] {
        let src = if c == 0 { &r_prev } else { &b_prev };
        let g = &out.g;
        let mut plane = src.clone();
        par_fill(&mut plane, w, h, 4, |y, x| {
            let i = y * w + x;
            if !is_green(y, x) {
                return src[i];
            }
            let n_grad = EPS + (g[i] - g[i - w2]).abs() + (src[i - w1] - src[i + w1]).abs() + (src[i - w1] - src[i - w3]).abs();
            let s_grad = EPS + (g[i] - g[i + w2]).abs() + (src[i + w1] - src[i - w1]).abs() + (src[i + w1] - src[i + w3]).abs();
            let w_grad = EPS + (g[i] - g[i - 2]).abs() + (src[i - 1] - src[i + 1]).abs() + (src[i - 1] - src[i - 3]).abs();
            let e_grad = EPS + (g[i] - g[i + 2]).abs() + (src[i + 1] - src[i - 1]).abs() + (src[i + 1] - src[i + 3]).abs();
            let n_est = src[i - w1] - g[i - w1];
            let s_est = src[i + w1] - g[i + w1];
            let w_est = src[i - 1] - g[i - 1];
            let e_est = src[i + 1] - g[i + 1];
            let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
            let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);
            let d = refine(&vh_dir, i);
            (g[i] + d * h_est + (1.0 - d) * v_est).max(0.0)
        });
        *out.plane_mut(c) = plane;
    }
    out
}

/// Removes false colour from fine repeating detail: 3x3 median of the
/// colour-difference channels (red minus green, blue minus green), then
/// rebuilds red and blue on the untouched green. Brightness detail lives
/// mostly in green, so it survives; isolated colour speckles don't.
/// The middle of nine values, as sorting them (by `total_cmp`) and taking the fifth would give, by a
/// fixed network of 19 compare-and-swaps (Paeth's) instead of a sort on every pixel. Done on each
/// value's sortable integer key (`total_cmp`'s own order, so the same result), whose min / max need no
/// branches.
fn median9(p: [f32; 9]) -> f32 {
    // As `f32::total_cmp`: flip the magnitude bits of negatives, so the integers sort as the floats.
    let key = |v: f32| {
        let b = v.to_bits() as i32;
        b ^ (((b >> 31) as u32) >> 1) as i32
    };
    let unkey = |k: i32| f32::from_bits((k ^ (((k >> 31) as u32) >> 1) as i32) as u32);
    let mut q = p.map(key);
    for (a, b) in [(1, 2), (4, 5), (7, 8), (0, 1), (3, 4), (6, 7), (1, 2), (4, 5), (7, 8), (0, 3), (5, 8), (4, 7), (3, 6), (1, 4), (2, 5), (4, 7), (2, 4), (4, 6), (2, 4)] {
        let (lo, hi) = (q[a].min(q[b]), q[a].max(q[b]));
        (q[a], q[b]) = (lo, hi);
    }
    unkey(q[4])
}

pub fn suppress_false_colour(p: &mut Planes, passes: usize) {
    let (w, h) = (p.width, p.height);
    for _ in 0..passes {
        for c in [0usize, 2] {
            let diff: Vec<f32> = {
                let plane = if c == 0 { &p.r } else { &p.b };
                plane.par_iter().zip(&p.g).map(|(v, g)| v - g).collect()
            };
            let mut filtered: Vec<f32> = diff.par_iter().copied().collect();
            filtered.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                if y == 0 || y + 1 >= h {
                    return;
                }
                for x in 1..w - 1 {
                    let mut win = [0.0f32; 9];
                    let mut k = 0;
                    for yy in y - 1..=y + 1 {
                        for xx in x - 1..=x + 1 {
                            win[k] = diff[yy * w + xx];
                            k += 1;
                        }
                    }
                    row[x] = median9(win);
                }
            });
            let (plane, g) = if c == 0 { (&mut p.r, &p.g) } else { (&mut p.b, &p.g) };
            plane.par_iter_mut().zip(g.par_iter().zip(filtered.par_iter())).for_each(|(v, (gv, f))| *v = (gv + f).max(0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    //! The test bench: render a known full-colour image, turn it into a Bayer
    //! mosaic the way a sensor would, demosaic, and score the result against
    //! the original.
    use super::*;

    #[test]
    fn median9_matches_a_sort() {
        let mut seed = 12345u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            // Few distinct values, so ties are common; and signed zeros.
            match (seed >> 60) % 3 {
                0 => [-0.0f32, 0.0, 1.0, -1.0, 2.5, 0.5, 3.0][(seed >> 33) as usize % 7],
                _ => f32::from_bits((seed >> 16) as u32) % 1e6,
            }
        };
        for _ in 0..200_000 {
            let v: [f32; 9] = std::array::from_fn(|_| rnd());
            let mut s = v;
            s.sort_by(f32::total_cmp);
            assert_eq!(median9(v).to_bits(), s[4].to_bits(), "{v:?}");
        }
    }

    const RGGB: Bayer = Bayer([[0, 1], [1, 2]]);

    struct Truth {
        w: usize,
        h: usize,
        rgb: Vec<[f32; 3]>,
    }

    fn mosaic(t: &Truth) -> Vec<f32> {
        (0..t.w * t.h).map(|i| t.rgb[i][RGGB.color_at(i / t.w, i % t.w)]).collect()
    }

    /// Grey zone plate: concentric rings getting finer outward - every
    /// frequency and direction at once, the classic moiré stress test.
    fn zone_plate(size: usize) -> Truth {
        let c = size as f32 / 2.0;
        let rgb = (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f32 - c, (i / size) as f32 - c);
                let v = 0.5 + 0.4 * (std::f32::consts::PI * (x * x + y * y) / (size as f32 * 1.6)).cos();
                [v, v, v]
            })
            .collect();
        Truth { w: size, h: size, rgb }
    }

    /// Fine grey check of the given square size: like a gingham shirt.
    fn checks(size: usize, square: usize) -> Truth {
        let rgb = (0..size * size)
            .map(|i| {
                let on = ((i % size) / square + (i / size) / square) % 2 == 0;
                let v = if on { 0.8 } else { 0.2 };
                [v, v, v]
            })
            .collect();
        Truth { w: size, h: size, rgb }
    }

    /// Smooth colour gradients with a few hard colour edges.
    fn colour_scene(size: usize) -> Truth {
        let rgb = (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f32 / size as f32, (i / size) as f32 / size as f32);
                let edge = if x > 0.5 { 0.3 } else { 0.0 };
                [0.2 + 0.6 * x, 0.3 + 0.4 * y, (0.7 - 0.5 * x + edge).min(1.0)]
            })
            .collect();
        Truth { w: size, h: size, rgb }
    }

    /// (PSNR in dB over all channels, mean false colour) away from the border.
    /// False colour = how far each pixel's red and blue stray from its green,
    /// relative to the truth - for grey targets, any colour at all is false.
    fn score(t: &Truth, p: &Planes) -> (f64, f64) {
        let (mut se, mut fc, mut n) = (0.0f64, 0.0f64, 0usize);
        for y in 6..t.h - 6 {
            for x in 6..t.w - 6 {
                let i = y * t.w + x;
                let got = [p.r[i], p.g[i], p.b[i]];
                let want = t.rgb[i];
                for c in 0..3 {
                    se += ((got[c] - want[c]) as f64).powi(2);
                }
                let fr = ((got[0] - got[1]) - (want[0] - want[1])).abs();
                let fb = ((got[2] - got[1]) - (want[2] - want[1])).abs();
                fc += (fr + fb) as f64 / 2.0;
                n += 1;
            }
        }
        let mse = se / (n * 3) as f64;
        (10.0 * (1.0 / mse).log10(), fc / n as f64)
    }

    fn run(t: &Truth) -> [(f64, f64); 3] {
        let m = mosaic(t);
        let bil = bilinear(&m, t.w, t.h, &RGGB);
        let rcd_out = rcd(&m, t.w, t.h, &RGGB);
        let mut cleaned = rcd(&m, t.w, t.h, &RGGB);
        suppress_false_colour(&mut cleaned, 1);
        [score(t, &bil), score(t, &rcd_out), score(t, &cleaned)]
    }

    fn report(name: &str, s: &[(f64, f64); 3]) {
        println!(
            "{name:12} bilinear {:5.1} dB fc {:.4} | rcd {:5.1} dB fc {:.4} | rcd+fcs {:5.1} dB fc {:.4}",
            s[0].0, s[0].1, s[1].0, s[1].1, s[2].0, s[2].1
        );
    }

    #[test]
    fn rcd_beats_bilinear_on_zone_plate() {
        let s = run(&zone_plate(256));
        report("zone plate", &s);
        assert!(s[1].0 > s[0].0 + 1.0, "RCD should be sharper: {s:?}");
        assert!(s[2].1 < s[0].1 * 0.6, "false colour should drop well below bilinear: {s:?}");
    }

    #[test]
    fn checks_stay_grey() {
        for square in [2usize, 3, 4] {
            let s = run(&checks(192, square));
            report(&format!("checks {square}px"), &s);
            assert!(s[2].1 <= s[0].1, "false colour must not be worse than bilinear: {s:?}");
        }
    }

    #[test]
    fn colour_scene_is_faithful() {
        let s = run(&colour_scene(192));
        report("colour", &s);
        // Smooth colour is easy for any method: RCD should simply not be worse.
        assert!(s[1].0 >= s[0].0 - 0.5, "{s:?}");
        assert!(s[2].0 > 35.0, "smooth colour shouldn't be damaged by suppression: {s:?}");
    }

    #[test]
    fn flat_grey_is_exact() {
        let t = Truth { w: 64, h: 64, rgb: vec![[0.4, 0.4, 0.4]; 64 * 64] };
        let p = rcd(&mosaic(&t), t.w, t.h, &RGGB);
        let (psnr, fc) = score(&t, &p);
        assert!(psnr > 80.0 && fc < 1e-4, "{psnr} {fc}");
    }
}
