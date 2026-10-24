//! The measured Grade: Lightroom's Color Grading as a curve per channel of
//! linear ProPhoto RGB (a channel's result depends on that channel's input
//! only, as measured), read off the synthetic chart's exports
//! (`examples/gradetable.rs`; only Lightroom's renders are measured).
//!
//! Tint: for each range (shadows, midtones, highlights, global) x hue (every
//! 30 degrees) x saturation (25, 50, 75, 100), three curves of log2 gain
//! along the channel's own encoded value. Luminance: per range and slider
//! position, one curve for all channels, applied before the tint. Ranges
//! multiply; values between hues, saturations and positions interpolate
//! (no change at 0).

use std::sync::OnceLock;

/// Curve samples along value^(1/2.2), 0..1.
pub const SAMPLES: usize = 32;
pub const HUES: usize = 12;
pub const SATS: [f32; 4] = [25.0, 50.0, 75.0, 100.0];
pub const LUMS: [f32; 8] = [-100.0, -75.0, -50.0, -25.0, 25.0, 50.0, 75.0, 100.0];

pub struct GradeTable {
    /// [range][hue][sat][channel][sample] log2 gain.
    pub tint: Vec<f32>,
    /// [range][lum position][sample] log2 gain.
    pub lum: Vec<f32>,
}

impl Default for GradeTable {
    fn default() -> Self {
        GradeTable { tint: vec![0.0; 4 * HUES * SATS.len() * 3 * SAMPLES], lum: vec![0.0; 4 * LUMS.len() * SAMPLES] }
    }
}

impl GradeTable {
    pub fn tint_index(range: usize, hue: usize, sat: usize, ch: usize) -> usize {
        (((range * HUES + hue) * SATS.len() + sat) * 3 + ch) * SAMPLES
    }
    pub fn lum_index(range: usize, pos: usize) -> usize {
        (range * LUMS.len() + pos) * SAMPLES
    }

    /// "RGT1", then every value as i16 (log2 gain x 4096).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = b"RGT1".to_vec();
        for v in self.tint.iter().chain(&self.lum) {
            b.extend(((v * 4096.0).round().clamp(-32767.0, 32767.0) as i16).to_le_bytes());
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<GradeTable> {
        if b.get(0..4)? != b"RGT1" {
            return None;
        }
        let mut t = GradeTable::default();
        let n = t.tint.len() + t.lum.len();
        if b.len() < 4 + n * 2 {
            return None;
        }
        let at = |i: usize| i16::from_le_bytes([b[4 + i * 2], b[5 + i * 2]]) as f32 / 4096.0;
        let nt = t.tint.len();
        for (i, v) in t.tint.iter_mut().enumerate() {
            *v = at(i);
        }
        for (i, v) in t.lum.iter_mut().enumerate() {
            *v = at(nt + i);
        }
        Some(t)
    }

    /// A curve (at `start`) read at linear value `x`.
    fn read(curve: &[f32], x: f32) -> f32 {
        let e = x.clamp(0.0, 1.0).powf(1.0 / 2.2) * (SAMPLES - 1) as f32;
        let i = (e.floor() as usize).min(SAMPLES - 2);
        let f = e - i as f32;
        curve[i] + (curve[i + 1] - curve[i]) * f
    }

    /// The log2 gain on channel `ch` at value `x` for one range's tint.
    fn tint_gain(&self, range: usize, hue: f32, sat: f32, ch: usize, x: f32) -> f32 {
        if sat <= 0.0 {
            return 0.0;
        }
        let hf = hue.rem_euclid(360.0) / 30.0;
        let (h0, ht) = (hf.floor() as usize % HUES, hf.fract());
        let h1 = (h0 + 1) % HUES;
        // Saturation: between the measured strengths, from no change at 0.
        let (s_lo, s_hi, st) = if sat <= SATS[0] {
            (None, 0, sat / SATS[0])
        } else {
            let i = SATS.windows(2).position(|w| sat <= w[1]).unwrap_or(SATS.len() - 2);
            (Some(i), i + 1, ((sat - SATS[i]) / (SATS[i + 1] - SATS[i])).min(1.0))
        };
        let at = |h: usize, s: usize| Self::read(&self.tint[Self::tint_index(range, h, s, ch)..][..SAMPLES], x);
        let by_hue = |s: usize| at(h0, s) * (1.0 - ht) + at(h1, s) * ht;
        let hi = by_hue(s_hi);
        let lo = s_lo.map_or(0.0, by_hue);
        lo + (hi - lo) * st
    }

    /// The log2 gain of one range's Luminance slider at value `x`.
    fn lum_gain(&self, range: usize, lum: f32, x: f32) -> f32 {
        if lum == 0.0 {
            return 0.0;
        }
        let side: Vec<usize> = (0..LUMS.len()).filter(|&i| LUMS[i].signum() == lum.signum()).collect();
        let mut sorted = side.clone();
        sorted.sort_by(|&a, &b| LUMS[a].abs().partial_cmp(&LUMS[b].abs()).unwrap());
        let at = |p: usize| Self::read(&self.lum[Self::lum_index(range, p)..][..SAMPLES], x);
        let a = lum.abs();
        if a <= LUMS[sorted[0]].abs() {
            return at(sorted[0]) * a / LUMS[sorted[0]].abs();
        }
        for w in sorted.windows(2) {
            let (p, q) = (LUMS[w[0]].abs(), LUMS[w[1]].abs());
            if a <= q {
                let t = (a - p) / (q - p);
                return at(w[0]) + (at(w[1]) - at(w[0])) * t;
            }
        }
        at(*sorted.last().unwrap())
    }

    /// Grades a linear ProPhoto colour. `ranges` = shadows, midtones,
    /// highlights, global as (hue, saturation 0..100, luminance -100..100).
    pub fn apply(&self, pp: [f32; 3], ranges: &[(f32, f32, f32); 4]) -> [f32; 3] {
        // Luminance first: one curve, all channels, ranges multiplying.
        let lit: [f32; 3] = std::array::from_fn(|ch| {
            let x = pp[ch].max(0.0);
            let g: f32 = ranges.iter().enumerate().map(|(r, &(_, _, lum))| self.lum_gain(r, lum, x)).sum();
            x * g.exp2()
        });
        std::array::from_fn(|ch| {
            let x = lit[ch];
            let g: f32 = ranges.iter().enumerate().map(|(r, &(hue, sat, _))| self.tint_gain(r, hue, sat, ch, x)).sum();
            x * g.exp2()
        })
    }
}

/// The table built into the app (None until one has been measured).
pub fn table() -> Option<&'static GradeTable> {
    static TABLE: OnceLock<Option<GradeTable>> = OnceLock::new();
    TABLE.get_or_init(|| GradeTable::from_bytes(include_bytes!("grade_table.bin"))).as_ref()
}
