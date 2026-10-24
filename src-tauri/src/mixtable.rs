//! The measured Mixer: Lightroom's response to each Mixer slider, as a lookup
//! in HSV of linear ProPhoto RGB (where Camera Raw's Mixer works), measured on
//! the synthetic chart (`examples/mixtable.rs` builds it from Lightroom's
//! exports; nothing of Adobe's is copied, only its renders measured).
//!
//! For every band x control (Hue, Saturation, Luminance) x measured slider
//! position, a grid over hue x saturation x value holds the change Lightroom
//! made there: a hue turn in degrees and log2 factors on saturation and value.
//! A pixel's change is read trilinearly; slider values between the measured
//! positions interpolate (with no change at 0); several sliders add.

use std::sync::OnceLock;

/// Grid hues (degrees, evenly spaced from 0).
pub const HUES: usize = 72;
/// Grid saturations (HSV saturation of linear ProPhoto RGB).
pub const SATS: [f32; 8] = [0.0, 0.08, 0.16, 0.28, 0.42, 0.56, 0.7, 0.9];
/// Grid values, as value^(1/2.2) (closer to how tones are spaced).
pub const VALS: [f32; 6] = [0.05, 0.25, 0.45, 0.65, 0.85, 1.0];
const CELLS: usize = HUES * SATS.len() * VALS.len();

pub struct MixTable {
    /// Measured slider positions (e.g. -100, -50, 50, 100), ascending.
    pub positions: Vec<f32>,
    /// [band][control][position][hue][sat][val] -> [hue turn, log2 sat, log2 val].
    pub data: Vec<[f32; 3]>,
}

impl MixTable {
    pub fn new(positions: Vec<f32>) -> MixTable {
        let n = 8 * 3 * positions.len() * CELLS;
        MixTable { positions, data: vec![[0.0; 3]; n] }
    }

    pub fn index(&self, band: usize, ctl: usize, pos: usize, h: usize, s: usize, v: usize) -> usize {
        ((((band * 3 + ctl) * self.positions.len() + pos) * HUES + h) * SATS.len() + s) * VALS.len() + v
    }

    /// The file form: "RMX1", position count, positions (i16), then every cell
    /// as three i16 (hue turn x 100, log2 factors x 4096).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = b"RMX1".to_vec();
        b.push(self.positions.len() as u8);
        for p in &self.positions {
            b.extend((*p as i16).to_le_bytes());
        }
        for c in &self.data {
            let q = [c[0] * 100.0, c[1] * 4096.0, c[2] * 4096.0];
            for x in q {
                b.extend((x.round().clamp(-32767.0, 32767.0) as i16).to_le_bytes());
            }
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<MixTable> {
        if b.get(0..4)? != b"RMX1" {
            return None;
        }
        let n = *b.get(4)? as usize;
        let i16_at = |at: usize| -> Option<f32> { Some(i16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]) as f32) };
        let positions: Vec<f32> = (0..n).map(|i| i16_at(5 + i * 2)).collect::<Option<_>>()?;
        let mut t = MixTable::new(positions);
        let start = 5 + n * 2;
        if b.len() < start + t.data.len() * 6 {
            return None;
        }
        for (i, c) in t.data.iter_mut().enumerate() {
            let at = start + i * 6;
            *c = [i16_at(at)? / 100.0, i16_at(at + 2)? / 4096.0, i16_at(at + 4)? / 4096.0];
        }
        Some(t)
    }

    /// One position's change at a pixel, trilinear over the grid.
    fn at(&self, band: usize, ctl: usize, pos: usize, h: f32, s: f32, v: f32) -> [f32; 3] {
        let hf = h.rem_euclid(360.0) / (360.0 / HUES as f32);
        let (h0, ht) = (hf.floor() as usize % HUES, hf.fract());
        let h1 = (h0 + 1) % HUES;
        let axis = |grid: &[f32], x: f32| -> (usize, f32) {
            let last = grid.len() - 1;
            if x <= grid[0] {
                return (0, 0.0);
            }
            if x >= grid[last] {
                return (last - 1, 1.0);
            }
            let i = grid.windows(2).position(|w| x < w[1]).unwrap_or(last - 1);
            (i, (x - grid[i]) / (grid[i + 1] - grid[i]))
        };
        let (s0, st) = axis(&SATS, s);
        let (v0, vt) = axis(&VALS, v.max(0.0).powf(1.0 / 2.2));
        let mut out = [0.0f32; 3];
        for (hi, hw) in [(h0, 1.0 - ht), (h1, ht)] {
            for (si, sw) in [(s0, 1.0 - st), (s0 + 1, st)] {
                for (vi, vw) in [(v0, 1.0 - vt), (v0 + 1, vt)] {
                    let w = hw * sw * vw;
                    if w == 0.0 {
                        continue;
                    }
                    let c = self.data[self.index(band, ctl, pos, hi, si, vi)];
                    for k in 0..3 {
                        out[k] += c[k] * w;
                    }
                }
            }
        }
        out
    }

    /// One slider's change at slider value `x` (-100..100): between the two
    /// measured positions around it, or from no change at 0 to the nearest.
    pub fn response(&self, band: usize, ctl: usize, x: f32, h: f32, s: f32, v: f32) -> [f32; 3] {
        if x == 0.0 {
            return [0.0; 3];
        }
        let p = &self.positions;
        // Measured positions on x's side of zero, nearest first.
        let side: Vec<usize> = (0..p.len()).filter(|&i| p[i].signum() == x.signum()).collect();
        if side.is_empty() {
            return [0.0; 3];
        }
        let mut lo: Option<usize> = None; // nearer to 0 than x (or equal)
        let mut hi: Option<usize> = None; // further from 0 than x
        for &i in &side {
            if p[i].abs() <= x.abs() {
                if lo.map_or(true, |j| p[i].abs() > p[j].abs()) {
                    lo = Some(i);
                }
            } else if hi.map_or(true, |j| p[i].abs() < p[j].abs()) {
                hi = Some(i);
            }
        }
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        match (lo, hi) {
            (Some(i), Some(j)) => {
                let t = (x.abs() - p[i].abs()) / (p[j].abs() - p[i].abs());
                lerp(self.at(band, ctl, i, h, s, v), self.at(band, ctl, j, h, s, v), t)
            }
            (None, Some(j)) => lerp([0.0; 3], self.at(band, ctl, j, h, s, v), x.abs() / p[j].abs()),
            (Some(i), None) => self.at(band, ctl, i, h, s, v),
            (None, None) => [0.0; 3],
        }
    }

    /// Applies all of a Mixer setting (`mix[band] = [hue, sat, lum]`) to a
    /// linear ProPhoto colour.
    pub fn apply(&self, pp: [f32; 3], mix: &[[f32; 3]; 8]) -> [f32; 3] {
        let [r, g, b] = pp.map(|x| x.max(0.0));
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        if mx <= 1e-6 || mx - mn <= 1e-6 {
            return pp;
        }
        let d = mx - mn;
        let h = 60.0 * if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        let (s, v) = (d / mx, mx);
        let mut total = [0.0f32; 3];
        for (band, sliders) in mix.iter().enumerate() {
            for (ctl, &x) in sliders.iter().enumerate() {
                if x != 0.0 {
                    let c = self.response(band, ctl, x, h, s, v);
                    for k in 0..3 {
                        total[k] += c[k];
                    }
                }
            }
        }
        let (h2, s2, v2) = (h + total[0], (s * total[1].exp2()).clamp(0.0, 1.0), v * total[2].exp2());
        // HSV back to RGB.
        let hp = h2.rem_euclid(360.0) / 60.0;
        let f = hp - hp.floor();
        let (p, q, t) = (v2 * (1.0 - s2), v2 * (1.0 - s2 * f), v2 * (1.0 - s2 * (1.0 - f)));
        match hp as i32 {
            0 => [v2, t, p],
            1 => [q, v2, p],
            2 => [p, v2, t],
            3 => [p, q, v2],
            4 => [t, p, v2],
            _ => [v2, p, q],
        }
    }
}

/// The table built into the app (None until one has been measured).
pub fn table() -> Option<&'static MixTable> {
    static TABLE: OnceLock<Option<MixTable>> = OnceLock::new();
    TABLE.get_or_init(|| MixTable::from_bytes(include_bytes!("mixer_table.bin"))).as_ref()
}
