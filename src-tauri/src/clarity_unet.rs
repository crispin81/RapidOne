//! Clarity+ U-Net inference (hand-written, CPU). Weights come from `ml/hdrunet.py --chan raw` via `ml/export_unet.py`.
//! Input is scene-linear luminance (before the tone curve); output is a per-pixel ln gain on that luminance.
//! Two input channels: clamp(ln Y - ln p99(Y), -12, 3) / 4 and sqrt(clamp(Y, 0, 4)).
use rayon::prelude::*;
use std::sync::OnceLock;

/// Trained on Lightroom's finished renders (input) and Lightroom's Clarity +100 renders (target) only.
static WEIGHTS: &[u8] = include_bytes!("../models/clarity_unet_disp.bin");

pub(crate) struct Conv { pub(crate) w: Vec<f32>, pub(crate) b: Vec<f32>, pub(crate) cin: usize, pub(crate) cout: usize, pub(crate) k: usize, pub(crate) stride: usize }
pub(crate) struct Lin { pub(crate) w: Vec<f32>, pub(crate) b: Vec<f32>, pub(crate) n_in: usize, pub(crate) n_out: usize }
pub struct UNet {
    pub(crate) e0: [Conv; 2], pub(crate) downs: Vec<Conv>, pub(crate) encs: Vec<Conv>, pub(crate) g1: Lin, pub(crate) g2: Lin, pub(crate) ups: Vec<[Conv; 2]>, pub(crate) out: Conv,
}

struct Reader<'a> { d: &'a [u8], at: usize }
impl Reader<'_> {
    fn u32(&mut self) -> u32 { let v = u32::from_le_bytes(self.d[self.at..self.at + 4].try_into().unwrap()); self.at += 4; v }
    fn f32s(&mut self, n: usize) -> Vec<f32> {
        let v = self.d[self.at..self.at + 4 * n].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        self.at += 4 * n; v
    }
    fn conv(&mut self, cin: usize, cout: usize, k: usize, stride: usize) -> Conv {
        let w = self.f32s(cout * cin * k * k); let b = self.f32s(cout);
        Conv { w, b, cin, cout, k, stride }
    }
    fn lin(&mut self, n_in: usize, n_out: usize) -> Lin { let w = self.f32s(n_out * n_in); let b = self.f32s(n_out); Lin { w, b, n_in, n_out } }
}

impl UNet {
    pub fn load(data: &[u8]) -> UNet {
        let mut r = Reader { d: data, at: 0 };
        assert_eq!(r.u32(), 0x52314E55, "bad U-Net weights");
        let in_ch = r.u32() as usize;
        let c: Vec<usize> = (0..6).map(|_| r.u32() as usize).collect();
        let e0 = [r.conv(in_ch, c[0], 3, 1), r.conv(c[0], c[0], 3, 1)];
        let downs = (0..5).map(|i| r.conv(c[i], c[i + 1], 3, 2)).collect();
        let encs = (0..5).map(|i| r.conv(c[i + 1], c[i + 1], 3, 1)).collect();
        let g1 = r.lin(c[5], 128); let g2 = r.lin(128, c[5]);
        let ups = (0..5).map(|i| [r.conv(c[i + 1] + c[i], c[i], 3, 1), r.conv(c[i], c[i], 3, 1)]).collect();
        let out = r.conv(c[0], 1, 1, 1);
        assert_eq!(r.at, data.len(), "U-Net weight file has trailing data");
        UNet { e0, downs, encs, g1, g2, ups, out }
    }
}

pub fn shared() -> &'static UNet { static N: OnceLock<UNet> = OnceLock::new(); N.get_or_init(|| UNet::load(WEIGHTS)) }

/// Planar (channel, height, width) f32 tensor.
struct T { c: usize, h: usize, w: usize, d: Vec<f32> }

fn conv(x: &T, cv: &Conv, relu: bool) -> T {
    assert_eq!(x.c, cv.cin);
    let (k, s) = (cv.k, cv.stride);
    let pad = k / 2;
    let (oh, ow) = ((x.h + 2 * pad - k) / s + 1, (x.w + 2 * pad - k) / s + 1);
    let mut d = vec![0.0f32; cv.cout * oh * ow];
    // A row of one output channel per task (by channel alone, the last layer's single channel ran on
    // one core); each value still adds its terms in the same order (input channel, then kernel row,
    // then column), so the result is the same.
    d.par_chunks_mut(ow).enumerate().for_each(|(r, orow)| {
        let (co, oy) = (r / oh, r % oh);
        orow.fill(cv.b[co]);
        for ci in 0..cv.cin {
            let ip = &x.d[ci * x.h * x.w..(ci + 1) * x.h * x.w];
            for ky in 0..k {
                let iy = (oy * s + ky) as isize - pad as isize;
                if iy < 0 || iy >= x.h as isize { continue; }
                let row = &ip[iy as usize * x.w..(iy as usize + 1) * x.w];
                for kx in 0..k {
                    let wv = cv.w[((co * cv.cin + ci) * k + ky) * k + kx];
                    // valid output columns: 0 <= ox*s + kx - pad < iw
                    let ox_lo = if kx >= pad { 0 } else { (pad - kx).div_ceil(s) };
                    let ox_hi = ((x.w + pad - kx).div_ceil(s)).min(ow);
                    if ox_hi <= ox_lo { continue; }
                    if s == 1 {
                        // Matching slices (no index sums per value), so the compiler can use the
                        // vector units; each value gets the same `+= wv * x` as before.
                        let first = ox_lo + kx - pad;
                        let src = &row[first..first + (ox_hi - ox_lo)];
                        for (o, v) in orow[ox_lo..ox_hi].iter_mut().zip(src) { *o += wv * v; }
                    } else {
                        let first = ox_lo * s + kx - pad;
                        for (o, v) in orow[ox_lo..ox_hi].iter_mut().zip(row[first..].iter().step_by(s)) { *o += wv * v; }
                    }
                }
            }
        }
        if relu { for v in orow.iter_mut() { if *v < 0.0 { *v = 0.0; } } }
    });
    T { c: cv.cout, h: oh, w: ow, d }
}

/// Bilinear upsample to (h, w), matching torch's align_corners=False.
fn upsample(x: &T, h: usize, w: usize) -> T {
    let axis = |n_in: usize, n_out: usize| -> Vec<(usize, usize, f32)> {
        let sc = n_in as f32 / n_out as f32;
        (0..n_out).map(|o| {
            let src = ((o as f32 + 0.5) * sc - 0.5).max(0.0);
            let i0 = (src.floor() as usize).min(n_in - 1);
            (i0, (i0 + 1).min(n_in - 1), src - i0 as f32)
        }).collect()
    };
    let (ys, xs) = (axis(x.h, h), axis(x.w, w));
    let mut d = vec![0.0f32; x.c * h * w];
    d.par_chunks_mut(h * w).enumerate().for_each(|(c, o)| {
        let p = &x.d[c * x.h * x.w..(c + 1) * x.h * x.w];
        for (oy, &(y0, y1, fy)) in ys.iter().enumerate() {
            for (ox, &(x0, x1, fx)) in xs.iter().enumerate() {
                let top = p[y0 * x.w + x0] * (1.0 - fx) + p[y0 * x.w + x1] * fx;
                let bot = p[y1 * x.w + x0] * (1.0 - fx) + p[y1 * x.w + x1] * fx;
                o[oy * w + ox] = top * (1.0 - fy) + bot * fy;
            }
        }
    });
    T { c: x.c, h, w, d }
}

fn lin(v: &[f32], l: &Lin, relu: bool) -> Vec<f32> {
    (0..l.n_out).map(|o| {
        let s = l.b[o] + l.w[o * l.n_in..(o + 1) * l.n_in].iter().zip(v).map(|(a, b)| a * b).sum::<f32>();
        if relu { s.max(0.0) } else { s }
    }).collect()
}

fn quantile(sorted: &[f32], f: f32) -> f32 {
    let pos = f * (sorted.len() - 1) as f32;
    let (i, fr) = (pos.floor() as usize, pos - pos.floor());
    sorted[i] + (sorted[(i + 1).min(sorted.len() - 1)] - sorted[i]) * fr
}

/// The network's input for a w x h luminance image: two planes, padded to multiples of 32 (their
/// width and height returned too).
pub(crate) fn net_input(y0: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    let mut s: Vec<f32> = y0.iter().step_by(7).map(|&v| v.max(1e-5)).collect();
    s.sort_by(|a, b| a.total_cmp(b));
    let lp99 = quantile(&s, 0.99).ln();
    let (pw, ph) = (w.div_ceil(32) * 32, h.div_ceil(32) * 32);
    let mut d = vec![0.0f32; 2 * pw * ph];
    for y in 0..ph {
        for x in 0..pw {
            let v = y0[y.min(h - 1) * w + x.min(w - 1)];
            d[y * pw + x] = ((v.max(1e-5).ln() - lp99).clamp(-12.0, 3.0)) / 4.0;
            d[pw * ph + y * pw + x] = v.clamp(0.0, 4.0).sqrt();
        }
    }
    (d, pw, ph)
}

/// ln gain for every pixel of a w x h scene-linear luminance image (row-major).
pub fn ln_gain(net: &UNet, y0: &[f32], w: usize, h: usize) -> Vec<f32> {
    let p_prep = crate::prof::span("      net input");
    let (d, pw, ph) = net_input(y0, w, h);
    let x = T { c: 2, h: ph, w: pw, d };
    drop(p_prep);
    let p_enc = crate::prof::span("      net encoder");
    let a = conv(&x, &net.e0[0], true);
    let mut skips = vec![conv(&a, &net.e0[1], true)];
    for i in 0..5 {
        let hcur = skips.last().unwrap();
        let t = conv(hcur, &net.downs[i], true);
        skips.push(conv(&t, &net.encs[i], true));
    }
    drop(p_enc);
    let p_dec = crate::prof::span("      net decoder");
    let mut hcur = skips.pop().unwrap();
    let plane = hcur.h * hcur.w;
    let mean: Vec<f32> = (0..hcur.c).map(|c| hcur.d[c * plane..(c + 1) * plane].iter().sum::<f32>() / plane as f32).collect();
    let g = lin(&lin(&mean, &net.g1, true), &net.g2, false);
    for c in 0..hcur.c { for v in &mut hcur.d[c * plane..(c + 1) * plane] { *v += g[c]; } }
    for i in (0..5).rev() {
        let skip = &skips[i];
        let up = upsample(&hcur, skip.h, skip.w);
        let mut cat = up.d; cat.extend_from_slice(&skip.d);
        let t = T { c: up.c + skip.c, h: skip.h, w: skip.w, d: cat };
        hcur = conv(&conv(&t, &net.ups[i][0], true), &net.ups[i][1], true);
    }
    drop(p_dec);
    let _p = crate::prof::span("      net output");
    let o = conv(&hcur, &net.out, false);
    (0..h).flat_map(|y| (0..w).map(move |x| (y, x))).map(|(y, x)| (o.d[y * pw + x] * 0.5).clamp(-3.0, 3.0)).collect()
}

/// Lift in L* at Clarity +100 per decile of display lightness (centres 5, 15, ..., 95) that the display-domain network needs to match Lightroom's mean
/// change on 150 held-out real RAW photos (`toolmatch BASE=lr VISUAL=1`, DECILES line, Lightroom change minus ours).
pub(crate) const TONE_CORR: [f32; 10] = [-0.43, -1.08, -1.18, -0.75, -0.29, -0.10, 0.34, 0.09, 0.21, 0.40];
pub fn tone_correction(lstar: f32) -> f32 {
    let t = ((lstar - 5.0) / 10.0).clamp(0.0, 9.0);
    let (i, f) = (t.floor() as usize, t - t.floor());
    TONE_CORR[i] * (1.0 - f) + TONE_CORR[(i + 1).min(9)] * f
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Compares against a PyTorch reference written by `ml/export_unet.py` (the reference file is committed, so this runs everywhere).
    #[test]
    fn matches_pytorch() {
        let r: &[u8] = include_bytes!("../models/clarity_unet_ref.bin");   // PyTorch output for a synthetic 83 x 109 image, made by ml/export_unet.py
        let (h, w) = (u32::from_le_bytes(r[0..4].try_into().unwrap()) as usize, u32::from_le_bytes(r[4..8].try_into().unwrap()) as usize);
        let f = |o: usize| -> Vec<f32> { r[o..o + 4 * h * w].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect() };
        let (y0, want) = (f(8), f(8 + 4 * h * w));
        let t = std::time::Instant::now();
        let got = ln_gain(shared(), &y0, w, h);
        eprintln!("inference {:?}", t.elapsed());
        let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("max abs diff {worst}");
        assert!(worst < 2e-3, "max diff {worst}");
    }
}

/// Per-pixel ln gain for Clarity +100 over a full-size scene-linear luminance image: the net runs on a copy shrunk to at most
/// `NET_LONG` pixels on the long edge (block averages) and its gain is stretched back up bilinearly.
pub const NET_LONG: usize = 512;
pub fn gain_map(lum: &[f32], w: usize, h: usize) -> Vec<f32> {
    let net = shared();
    let f = w.max(h).div_ceil(NET_LONG).max(1);
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    let mut small = vec![0.0f32; sw * sh];
    small.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for yy in (y * f)..((y + 1) * f).min(h) { for xx in (x * f)..((x + 1) * f).min(w) { s += lum[yy * w + xx]; n += 1.0; } }
            *o = s / n;
        }
    });
    let g = ln_gain(net, &small, sw, sh);
    let _p = crate::prof::span("      net upsample to full size");
    if f == 1 { return g; }
    let t = T { c: 1, h: sh, w: sw, d: g };
    // Sample positions follow the block centres: full-res pixel x sits at (x + 0.5) / f - 0.5 in the small grid.
    let axis = |n_in: usize, n_out: usize| -> Vec<(usize, usize, f32)> {
        (0..n_out).map(|o| {
            let src = ((o as f32 + 0.5) / f as f32 - 0.5).max(0.0);
            let i0 = (src.floor() as usize).min(n_in - 1);
            (i0, (i0 + 1).min(n_in - 1), src - i0 as f32)
        }).collect()
    };
    let (ys, xs) = (axis(sh, h), axis(sw, w));
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(oy, row)| {
        let (y0, y1, fy) = ys[oy];
        for (ox, o) in row.iter_mut().enumerate() {
            let (x0, x1, fx) = xs[ox];
            let top = t.d[y0 * sw + x0] * (1.0 - fx) + t.d[y0 * sw + x1] * fx;
            let bot = t.d[y1 * sw + x0] * (1.0 - fx) + t.d[y1 * sw + x1] * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
    });
    out
}
