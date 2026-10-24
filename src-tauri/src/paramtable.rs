//! The measured parametric (region) tone curve: Lightroom's curve for each
//! region slider, measured on the gray ramp of the `chartkit CHART=param` kit
//! (`examples/paramfit.rs` builds the table from Lightroom's exports; nothing
//! of Adobe's is copied, only its renders measured).
//!
//! The curve works on gamma-2.2 ProPhoto. Stored: each region alone at the
//! default splits (25 / 50 / 75) at every measured amount, and Darks with
//! Lights together on a grid (they don't simply add). A curve is built as
//! Shadows and Highlights first - each read where its splits put it, its
//! change scaled with its own split's distance from the end (shadow split /
//! 25, (100 - highlight split) / 25) - then Darks and Lights, read with the
//! midtone split moved and scaled with the span on that side of it. On the
//! kit: default splits exact (Shadows+Darks, Highlights+Lights and Chris's
//! own curve within 0.2 of an 8-bit level); moved shadow/highlight splits 1.05
//! levels on average, a moved midtone split 1.40 (other rules: 1.39 / 1.86).

use std::sync::OnceLock;

/// Points along the tone range (0..1, even steps).
pub const SAMPLES: usize = 257;
/// Measured amounts for each region alone.
pub const AMOUNTS: [f32; 9] = [-100.0, -75.0, -50.0, -25.0, 0.0, 25.0, 50.0, 75.0, 100.0];
/// Darks x Lights grid.
pub const GRID: [f32; 7] = [-100.0, -50.0, -25.0, 0.0, 25.0, 50.0, 100.0];

pub struct ParamTable {
    /// [region: shadows, darks, lights, highlights][amount] -> change along the range.
    pub single: Vec<Vec<Vec<f32>>>,
    /// [darks][lights] -> change along the range.
    pub dl: Vec<Vec<Vec<f32>>>,
}

impl Default for ParamTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The change at `x` (0..1) of a sampled curve, linearly interpolated.
fn read(d: &[f32], x: f32) -> f32 {
    let f = x.clamp(0.0, 1.0) * (SAMPLES - 1) as f32;
    let i = (f as usize).min(SAMPLES - 2);
    d[i] + (d[i + 1] - d[i]) * (f - i as f32)
}

/// Where `v` falls among ascending `knots`: (lower index, fraction).
fn locate(knots: &[f32], v: f32) -> (usize, f32) {
    let v = v.clamp(knots[0], knots[knots.len() - 1]);
    let i = knots.iter().rposition(|&k| k <= v).unwrap_or(0).min(knots.len() - 2);
    (i, (v - knots[i]) / (knots[i + 1] - knots[i]))
}

/// Piecewise-linear map taking each `from` knot to the matching `to` knot.
pub fn warp(x: f32, from: &[f32], to: &[f32]) -> f32 {
    let (i, f) = locate(from, x);
    to[i] + (to[i + 1] - to[i]) * f
}

impl ParamTable {
    pub fn new() -> ParamTable {
        ParamTable { single: vec![vec![vec![0.0; SAMPLES]; AMOUNTS.len()]; 4], dl: vec![vec![vec![0.0; SAMPLES]; GRID.len()]; GRID.len()] }
    }

    /// One region's change at `x` for `amount`, at the default splits.
    pub fn region(&self, r: usize, amount: f32, x: f32) -> f32 {
        let (i, f) = locate(&AMOUNTS, amount);
        read(&self.single[r][i], x) * (1.0 - f) + read(&self.single[r][i + 1], x) * f
    }

    /// Darks with Lights at `x`, at the default midtone split. Along an axis
    /// (one of them at 0) the finer single-region measurements are used.
    pub fn darks_lights(&self, d: f32, l: f32, x: f32) -> f32 {
        if l == 0.0 {
            return self.region(1, d, x);
        }
        if d == 0.0 {
            return self.region(2, l, x);
        }
        let (i, fi) = locate(&GRID, d);
        let (j, fj) = locate(&GRID, l);
        let v = |a: usize, b: usize| read(&self.dl[a][b], x);
        (v(i, j) * (1.0 - fj) + v(i, j + 1) * fj) * (1.0 - fi) + (v(i + 1, j) * (1.0 - fj) + v(i + 1, j + 1) * fj) * fi
    }

    /// The curve's output at `x` (gamma-2.2 ProPhoto) for region amounts
    /// [Shadows, Darks, Lights, Highlights] (-100..100) and splits (0..100).
    pub fn eval(&self, x: f32, amounts: [f32; 4], splits: [f32; 3]) -> f32 {
        let [s1, m, s3] = splits.map(|v| (v / 100.0).clamp(0.02, 0.98));
        let mut y = x;
        // Shadows below the midtone split, Highlights above it.
        if amounts[0] != 0.0 && x < m {
            let at = warp(x, &[0.0, s1, m], &[0.0, 0.25, 0.5]);
            y += self.region(0, amounts[0], at) * (s1 / 0.25);
        }
        if amounts[3] != 0.0 && x > m {
            let at = warp(x, &[m, s3, 1.0], &[0.5, 0.75, 1.0]);
            y += self.region(3, amounts[3], at) * ((1.0 - s3) / 0.25);
        }
        // Then Darks and Lights, over the whole range.
        if amounts[1] != 0.0 || amounts[2] != 0.0 {
            let at = warp(y, &[0.0, m, 1.0], &[0.0, 0.5, 1.0]);
            let span = if y < m { m / 0.5 } else { (1.0 - m) / 0.5 };
            y += self.darks_lights(amounts[1], amounts[2], at) * span;
        }
        y.clamp(0.0, 1.0)
    }

    /// File form: "RPC1", then every sample as i16 (change x 16384): the
    /// single regions, then the Darks x Lights grid.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = b"RPC1".to_vec();
        for v in self.single.iter().flatten().flatten().chain(self.dl.iter().flatten().flatten()) {
            b.extend(((v * 16384.0).round().clamp(-32767.0, 32767.0) as i16).to_le_bytes());
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<ParamTable> {
        if b.get(0..4)? != b"RPC1" {
            return None;
        }
        let n = (4 * AMOUNTS.len() + GRID.len() * GRID.len()) * SAMPLES;
        if b.len() != 4 + 2 * n {
            return None;
        }
        let mut vals = b[4..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 16384.0);
        let mut t = ParamTable::new();
        for v in t.single.iter_mut().flatten().flatten().chain(t.dl.iter_mut().flatten().flatten()) {
            *v = vals.next()?;
        }
        Some(t)
    }
}

/// The table built into the app, if there is one yet.
pub fn table() -> Option<&'static ParamTable> {
    static TABLE: OnceLock<Option<ParamTable>> = OnceLock::new();
    TABLE.get_or_init(|| ParamTable::from_bytes(include_bytes!("param_table.bin"))).as_ref()
}
