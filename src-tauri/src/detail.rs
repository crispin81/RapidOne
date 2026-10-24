//! Detail tools on the finished picture: sharpening, (simple) noise
//! reduction and grain, all on brightness only, so colours stay put.
//!
//! Sharpening is Richardson-Lucy deconvolution on linear brightness, fitted
//! to Lightroom's Sharpening (Amount 25-150, Radius 1.0, Detail 25, Masking
//! 0) on a 12-photo kit (`tools/ml/sharpen_fit.py`, 16 Oct): 38% of
//! Lightroom's change matched on held-out photos, against 31% for an unsharp
//! mask. Its radius is in full-sensor pixels, scaled to the picture being
//! finished (with a floor, so the preview still shows it).
//!
//! The simple noise reduction and grain radii are in pixels of the picture
//! being finished; the viewer's preview is about 2400 px on the long edge.

use image::{ImageBuffer, Rgb};
use rayon::prelude::*;

/// Richardson-Lucy fit (sharpen_fit.json): Gaussian sigma in full-sensor
/// pixels, rounds, halo limit (x the 3x3 local range). The strength curve
/// gain = G x (Amount / 100) ^ P matches how much Lightroom changes the
/// picture at the same Amount (RMS change in linear brightness over the
/// kit's most detailed crops, median of 12 photos; sharpen_strength.py, 16 Oct):
/// the first fit (2.2, 1.3) was 30-35% weaker than Lightroom.
const RL_SIGMA: f32 = 0.7;
const RL_ROUNDS: usize = 20;
const RL_HALO: f32 = 0.5;
const RL_G: f32 = 2.873;
const RL_P: f32 = 1.201;
/// Smallest sigma used on a shrunken picture (below it the effect vanishes).
const RL_MIN_SIGMA: f32 = 0.45;

/// A picture's channel type: 8-bit (the viewer) or 16-bit (the hand-off to
/// RapidRetouch, TIFF export).
pub trait Channel: image::Primitive + Send + Sync + 'static {
    /// Full scale: 255 or 65535.
    const MAX: f32;
    fn to_f32(self) -> f32;
    /// Rounded and clipped to the channel's range.
    fn from_f32(v: f32) -> Self;
}

impl Channel for u8 {
    const MAX: f32 = 255.0;
    fn to_f32(self) -> f32 {
        self as f32
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
}

impl Channel for u16 {
    const MAX: f32 = 65535.0;
    fn to_f32(self) -> f32 {
        self as f32
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
}

/// Rec. 709 brightness of a pixel, 0..1.
fn luma<T: Channel>(p: &Rgb<T>) -> f32
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    (0.2126 * p[0].to_f32() + 0.7152 * p[1].to_f32() + 0.0722 * p[2].to_f32()) / T::MAX
}

/// A 3x3 box blur, applied `passes` times (two passes is close to a Gaussian
/// of about 1 px).
fn blur(src: &[f32], w: usize, h: usize, passes: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0; a.len()];
    for _ in 0..passes {
        // Rows, then columns.
        for y in 0..h {
            let row = &a[y * w..(y + 1) * w];
            for x in 0..w {
                let l = row[x.saturating_sub(1)];
                let r = row[(x + 1).min(w - 1)];
                b[y * w + x] = (l + row[x] + r) / 3.0;
            }
        }
        for y in 0..h {
            let up = y.saturating_sub(1) * w;
            let dn = (y + 1).min(h - 1) * w;
            for x in 0..w {
                a[y * w + x] = (b[up + x] + b[y * w + x] + b[dn + x]) / 3.0;
            }
        }
    }
    a
}

/// A repeatable pseudo-random number in -1..1 for a pixel, so grain doesn't
/// crawl as the sliders move.
fn hash_noise(x: u32, y: u32) -> f32 {
    let mut n = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    n ^= n >> 15;
    n = n.wrapping_mul(0x2C1B_3C6D);
    n ^= n >> 12;
    n = n.wrapping_mul(0x297A_2D39);
    n ^= n >> 15;
    (n as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn srgb_to_lin(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn lin_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Separable Gaussian blur (edges mirrored), as the fit's, handed over a row at a time: `finish(y,
/// row)` gets each finished row while it's still in the processor's cache (sharpening turns it
/// straight into its next step). Worked through in strips of rows: each strip's horizontal pass (with
/// the rows the vertical taps reach above and below it) goes into a small buffer that stays in cache,
/// rather than a whole-picture buffer written out to memory and read back (sharpening's twenty rounds
/// were held up by memory, 24 Oct). Each value is summed exactly as before.
fn gaussian_rows(src: &[f32], w: usize, h: usize, sigma: f32, finish: impl Fn(usize, &[f32]) + Sync) {
    let r = (3.0 * sigma).ceil().max(1.0) as isize;
    let k: Vec<f32> = (-r..=r).map(|x| (-((x * x) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let mirror = |i: isize, n: usize| -> usize {
        let n = n as isize;
        let i = if i < 0 { -i } else if i >= n { 2 * n - 2 - i } else { i };
        i.clamp(0, n - 1) as usize
    };
    let ru = r as usize;
    // One row's horizontal pass into `row`.
    let across = |y: usize, row: &mut [f32]| {
        let s = &src[y * w..(y + 1) * w];
        // Mirrored only at the edges.
        let edge = |x: usize| -> f32 { k.iter().enumerate().map(|(j, kv)| kv * s[mirror(x as isize + j as isize - r, w)]).sum() };
        if w <= 2 * ru {
            for (x, out) in row.iter_mut().enumerate() {
                *out = edge(x);
            }
            return;
        }
        // Inside, tap by tap across the row (vector units), each value summed in the same order as
        // `sum()` (which starts from -0.0, so from the first tap's product exactly).
        let inner = &mut row[ru..w - ru];
        for (o, v) in inner.iter_mut().zip(&s[..w - 2 * ru]) {
            *o = k[0] * v;
        }
        for (j, kv) in k.iter().enumerate().skip(1) {
            for (o, v) in inner.iter_mut().zip(&s[j..j + w - 2 * ru]) {
                *o += kv * v;
            }
        }
        for x in (0..ru).chain(w - ru..w) {
            row[x] = edge(x);
        }
    };
    const STRIP: usize = 32;
    let taps = k.len();
    (0..h.div_ceil(STRIP)).into_par_iter().for_each_init(
        || (vec![0.0f32; (STRIP + taps) * w], vec![0.0f32; w]),
        |(across_rows, row), strip| {
            let (y0, y1) = (strip * STRIP, ((strip + 1) * STRIP).min(h));
            // Buffer row t: the horizontal pass of picture row mirror(y0 - r + t).
            let reach = y1 - y0 + taps - 1;
            for t in 0..reach {
                across(mirror(y0 as isize - r + t as isize, h), &mut across_rows[t * w..(t + 1) * w]);
            }
            for y in y0..y1 {
                // Columns a whole row at a time, in the same tap order, from 0.0 as before.
                row.fill(0.0);
                for (j, kv) in k.iter().enumerate() {
                    let t = y - y0 + j;
                    for (o, v) in row.iter_mut().zip(&across_rows[t * w..(t + 1) * w]) {
                        *o += kv * v;
                    }
                }
                finish(y, row);
            }
        },
    );
}

/// Lightroom-fitted sharpening (Amount 0..150): Richardson-Lucy on linear
/// brightness, the change limited near edges (no halos), every channel
/// scaled by the brightness change. `scale`: this picture's long edge over
/// the sensor's.
fn sharpen_rl<T: Channel>(img: &mut ImageBuffer<Rgb<T>, Vec<T>>, amount: f32, scale: f32)
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 3 || h < 3 {
        return;
    }
    let sigma = (RL_SIGMA * scale).max(RL_MIN_SIGMA);
    let gain = RL_G * (amount / 100.0).powf(RL_P);
    // sRGB to linear by table (every 8- or 16-bit value once), on all cores.
    let table: Vec<f32> = (0..=T::MAX as u32).map(|v| srgb_to_lin(v as f32 / T::MAX)).collect();
    let lin: Vec<[f32; 3]> = img.as_raw().par_chunks(3).map(|p| [0, 1, 2].map(|c| table[p[c].to_f32() as usize])).collect();
    let y: Vec<f32> = lin.par_iter().map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).collect();
    let mut u = y.clone();
    // The rounds' buffer, made once; written row by row from the blurs (each row by one task only).
    let mut ratio = vec![0.0f32; y.len()];
    struct Rows(*mut f32);
    unsafe impl Send for Rows {}
    unsafe impl Sync for Rows {}
    for _ in 0..RL_ROUNDS {
        // ratio = y / blur(u), each row as its blur is finished.
        let out = Rows(ratio.as_mut_ptr());
        let out = &out;
        gaussian_rows(&u, w, h, sigma, |row_y, bu| {
            // SAFETY: row `row_y` of `ratio` (w values from row_y * w, inside it) is written by this call only.
            let r = unsafe { std::slice::from_raw_parts_mut(out.0.add(row_y * w), w) };
            for ((rv, yv), b) in r.iter_mut().zip(&y[row_y * w..][..w]).zip(bu) {
                *rv = yv / b.max(1e-5);
            }
        });
        // u *= blur(ratio), likewise.
        let out = Rows(u.as_mut_ptr());
        let out = &out;
        gaussian_rows(&ratio, w, h, sigma, |row_y, br| {
            // SAFETY: as above, row `row_y` of `u`; `u` isn't read during this blur (it reads `ratio`).
            let uv = unsafe { std::slice::from_raw_parts_mut(out.0.add(row_y * w), w) };
            for (uv, r) in uv.iter_mut().zip(br) {
                *uv *= r;
            }
        });
    }
    let out: Vec<[f32; 3]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, yy) = (i % w, i / w);
            // The 3x3 range of brightness around the pixel (edges repeated).
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let ny = (yy as isize + dy).clamp(0, h as isize - 1) as usize;
                    let v = y[ny * w + nx];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            let limit = RL_HALO * (hi - lo);
            let d = (u[i] - y[i]).clamp(-limit, limit);
            let k = if y[i] > 1e-6 { ((y[i] + gain * d) / y[i]).max(0.0) } else { 1.0 };
            lin[i].map(|c| c * k)
        })
        .collect();
    // Back to sRGB on all cores.
    let raw: &mut [T] = img;
    raw.par_chunks_mut(3).zip(out.par_iter()).for_each(|(p, l)| {
        for c in 0..3 {
            p[c] = T::from_f32(lin_to_srgb(l[c]) * T::MAX);
        }
    });
}

/// Applies sharpening (`sharpen`, 0..150, Lightroom-fitted; `scale` is the
/// picture's long edge over the sensor's), then noise reduction (`noise`,
/// 0..100, simple) and grain (`grain`, 0..100), to brightness only.
pub fn apply<T: Channel>(img: &mut ImageBuffer<Rgb<T>, Vec<T>>, sharpen: f32, noise: f32, grain: f32, scale: f32)
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    if sharpen > 0.0 {
        sharpen_rl(img, sharpen.min(150.0), scale);
    }
    if noise <= 0.0 && grain <= 0.0 {
        return;
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let y0: Vec<f32> = img.pixels().map(luma).collect();
    let mut y = y0.clone();

    // Noise reduction: pull each pixel towards its smoothed neighbourhood,
    // but only where the difference is small (noise), not across edges.
    if noise > 0.0 {
        let smooth = blur(&y, w, h, 2);
        let strength = (noise / 100.0).min(1.0);
        // Differences up to about this size count as noise (wider as the slider rises).
        let sigma = 0.01 + 0.05 * strength;
        for i in 0..y.len() {
            let d = smooth[i] - y[i];
            let keep_edge = (-(d / sigma).powi(2)).exp();
            y[i] += d * strength * keep_edge;
        }
    }

    // Grain: fine film-like noise, strongest in the mid-tones, softened a touch
    // so it reads as grain rather than single-pixel speckle.
    if grain > 0.0 {
        let raw: Vec<f32> = (0..w * h).map(|i| hash_noise((i % w) as u32, (i / w) as u32)).collect();
        let soft = blur(&raw, w, h, 1);
        let amount = grain / 100.0 * 0.12;
        for i in 0..y.len() {
            let mid = 4.0 * y[i].clamp(0.0, 1.0) * (1.0 - y[i].clamp(0.0, 1.0));
            y[i] += soft[i] * amount * (0.35 + 0.65 * mid);
        }
    }

    // Move each pixel's channels by its change in brightness.
    for (i, p) in img.pixels_mut().enumerate() {
        let d = (y[i] - y0[i]) * T::MAX;
        if d == 0.0 {
            continue;
        }
        for c in 0..3 {
            p[c] = T::from_f32(p[c].to_f32() + d);
        }
    }
}
