//! The measured tone sliders: Lightroom's Highlights, Shadows, Whites and
//! Blacks as a change in displayed lightness (CIELAB L*), by the pixel's L* in
//! the render without them, measured on the tone kit
//! (`RapidOne-calibration-tone2`; `examples/tonetable.rs` builds the table
//! from Lightroom's exports, nothing of Adobe's copied).
//!
//! Lightroom adapts their strength to the photo, and the measurements show
//! how: Highlights grows as the photo's median L* falls, Shadows as its 75th
//! percentile rises, Whites (+) as its brightest tones (98th percentile) leave
//! more room; Blacks and Whites (-) are the same on every photo. So each
//! slider position stores a curve (the mean over the kit's photos) and a
//! strength rule, log s = a + b (measure - mean measure). Leave-one-photo-out
//! on Lightroom's own exports: Blacks 92-97%, Whites (-) 88-90%, Whites (+)
//! 59-68%, Highlights 64-67%, Shadows 61-62% of the L* change.

use std::sync::OnceLock;

/// L* points of each curve: 0, 5 .. 100.
pub const KNOTS: usize = 21;
/// Measured positions per slider (0 is implicit: no change).
pub const POSITIONS: [[f32; 6]; 4] = [
    [-100.0, -75.0, -50.0, -25.0, 50.0, 100.0], // Highlights
    [-100.0, -50.0, 25.0, 50.0, 75.0, 100.0],   // Shadows
    [-100.0, -50.0, 50.0, 100.0, f32::NAN, f32::NAN], // Whites
    [-100.0, -50.0, 50.0, 100.0, f32::NAN, f32::NAN], // Blacks
];

/// The photo measures a strength can follow.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhotoStats {
    pub median: f32,
    pub p75: f32,
    pub p98: f32,
}

impl PhotoStats {
    /// From the L* of the photo's pixels (any order).
    pub fn of(mut l: Vec<f32>) -> PhotoStats {
        if l.is_empty() {
            return PhotoStats { median: 50.0, p75: 65.0, p98: 90.0 };
        }
        l.sort_by(|a, b| a.total_cmp(b));
        let q = |f: f32| l[((l.len() - 1) as f32 * f) as usize];
        PhotoStats { median: q(0.5), p75: q(0.75), p98: q(0.98) }
    }

    /// Measure 0 none, 1 median, 2 75th percentile, 3 98th percentile.
    fn get(&self, which: u8) -> f32 {
        match which {
            1 => self.median,
            2 => self.p75,
            3 => self.p98,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// L* change at each knot (at strength 1).
    pub curve: [f32; KNOTS],
    /// Colour strength change at each knot: log of the C* ratio (at strength 1).
    pub chroma: [f32; KNOTS],
    /// Strength rule: log s = a + b (measure - centre).
    pub measure: u8,
    pub a: f32,
    pub b: f32,
    pub centre: f32,
}

impl Entry {
    fn strength(&self, st: &PhotoStats) -> f32 {
        if self.measure == 0 {
            return 1.0;
        }
        // Kept within the range the kit measured (no wild extrapolation).
        let m = (st.get(self.measure) - self.centre).clamp(-15.0, 15.0);
        (self.a + self.b * m).exp().clamp(0.1, 5.0)
    }

    fn at(&self, l: f32) -> f32 {
        read(&self.curve, l)
    }

    fn chroma_at(&self, l: f32) -> f32 {
        read(&self.chroma, l)
    }
}

fn read(c: &[f32; KNOTS], l: f32) -> f32 {
    let f = (l.clamp(0.0, 100.0) / 5.0).min((KNOTS - 1) as f32 - 1e-4);
    let i = f as usize;
    c[i] + (c[i + 1] - c[i]) * (f - i as f32)
}

pub struct ToneTable {
    /// [slider][position] (positions as in `POSITIONS`; missing ones None).
    pub entries: Vec<Vec<Option<Entry>>>,
}

impl ToneTable {
    /// One slider's L* change at `l` for `amount`, interpolated between the
    /// measured positions (and no change at 0).
    fn slider(&self, k: usize, amount: f32, l: f32, st: &PhotoStats) -> f32 {
        self.slider_part(k, amount, l, st, false)
    }

    /// One slider's L* change (`chroma` false) or log C* ratio (true).
    fn slider_part(&self, k: usize, amount: f32, l: f32, st: &PhotoStats, chroma: bool) -> f32 {
        if amount == 0.0 {
            return 0.0;
        }
        // (position, change) pairs on this side of 0, plus 0 itself.
        let mut pts: Vec<(f32, f32)> = vec![(0.0, 0.0)];
        for (p, e) in POSITIONS[k].iter().zip(&self.entries[k]) {
            if let Some(e) = e {
                if p.signum() == amount.signum() {
                    pts.push((*p, e.strength(st) * if chroma { e.chroma_at(l) } else { e.at(l) }));
                }
            }
        }
        pts.sort_by(|a, b| a.0.abs().total_cmp(&b.0.abs()));
        let a = amount.abs();
        for w in pts.windows(2) {
            let (p0, p1) = (w[0].0.abs(), w[1].0.abs());
            if a <= p1 {
                let t = (a - p0) / (p1 - p0).max(1e-6);
                return w[0].1 + (w[1].1 - w[0].1) * t;
            }
        }
        // Beyond the last measured position: scale the last one.
        let (p, v) = *pts.last().unwrap();
        v * a / p.abs().max(1e-6)
    }

    /// The new L* of a pixel at `l` for [Highlights, Shadows, Whites, Blacks],
    /// applied one after another.
    pub fn apply(&self, l: f32, amounts: [f32; 4], st: &PhotoStats) -> f32 {
        self.apply_both(l, amounts, st).0
    }

    /// The new L*, and the factor on C* (colour strength), for a pixel at `l`.
    pub fn apply_both(&self, l: f32, amounts: [f32; 4], st: &PhotoStats) -> (f32, f32) {
        let (mut out, mut log_c) = (l, 0.0f32);
        for k in 0..4 {
            log_c += self.slider_part(k, amounts[k], out, st, true);
            out += self.slider(k, amounts[k], out, st);
        }
        (out.clamp(0.0, 100.0), log_c.clamp(-3.0, 3.0).exp())
    }

    /// File form: "RTT1", then per slider and position: a present flag, the
    /// curve and chroma curve (f32 x KNOTS each), measure (u8), a, b, centre (f32).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = b"RTT2".to_vec();
        for row in &self.entries {
            for e in row {
                match e {
                    None => b.push(0),
                    Some(e) => {
                        b.push(1);
                        for v in e.curve.iter().chain(&e.chroma) {
                            b.extend(v.to_le_bytes());
                        }
                        b.push(e.measure);
                        for v in [e.a, e.b, e.centre] {
                            b.extend(v.to_le_bytes());
                        }
                    }
                }
            }
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<ToneTable> {
        if b.get(0..4)? != b"RTT2" {
            return None;
        }
        let mut i = 4;
        let f32_at = |b: &[u8], i: &mut usize| -> Option<f32> {
            let v = f32::from_le_bytes(b.get(*i..*i + 4)?.try_into().ok()?);
            *i += 4;
            Some(v)
        };
        let mut entries = Vec::new();
        for _ in 0..4 {
            let mut row = Vec::new();
            for _ in 0..6 {
                let present = *b.get(i)?;
                i += 1;
                if present == 0 {
                    row.push(None);
                    continue;
                }
                let mut curve = [0.0; KNOTS];
                for c in curve.iter_mut() {
                    *c = f32_at(b, &mut i)?;
                }
                let mut chroma = [0.0; KNOTS];
                for c in chroma.iter_mut() {
                    *c = f32_at(b, &mut i)?;
                }
                let measure = *b.get(i)?;
                i += 1;
                let (a, bb, centre) = (f32_at(b, &mut i)?, f32_at(b, &mut i)?, f32_at(b, &mut i)?);
                row.push(Some(Entry { curve, chroma, measure, a, b: bb, centre }));
            }
            entries.push(row);
        }
        Some(ToneTable { entries })
    }
}

/// The table built into the app, if there is one yet.
pub fn table() -> Option<&'static ToneTable> {
    static TABLE: OnceLock<Option<ToneTable>> = OnceLock::new();
    TABLE.get_or_init(|| ToneTable::from_bytes(include_bytes!("tone_table.bin"))).as_ref()
}
