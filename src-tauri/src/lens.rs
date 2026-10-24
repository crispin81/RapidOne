//! Built-in lens corrections: the distortion, lateral colour fringing and
//! vignetting tables a camera stores in its RAWs, which Lightroom applies
//! by default ("built-in profile"). Fujifilm for now.
//!
//! Fujifilm RAF: in the RAW block's TIFF (offset at header byte 100), IFD0's
//! tag 0xF000 leads to the Fuji IFD, whose tags 0xF00B (distortion), 0xF00F
//! (lateral CA) and 0xF010 (vignetting) are signed rationals: a pixel scale,
//! then knots (radius as a fraction of the half diagonal), then values:
//! distortion in percent, CA as a radius fraction for red then blue,
//! vignetting as the light remaining in percent. (Layout as documented by
//! the MIT-licensed RAWmakase project's reader; this is our own code.)

use crate::pipeline::SceneImage;

/// A function of the normalized radius (0 centre, 1 half diagonal), linear
/// between knots and held beyond them.
#[derive(Clone, Debug)]
pub struct Radial {
    pub knots: Vec<f32>,
    pub values: Vec<f32>,
}

impl Radial {
    fn new(knots: Vec<f32>, values: Vec<f32>) -> Option<Radial> {
        let ok = knots.len() >= 2
            && knots.len() == values.len()
            && knots.windows(2).all(|k| k[0] < k[1])
            && knots.iter().chain(&values).all(|v| v.is_finite())
            && values.iter().all(|v| (0.2..=5.0).contains(v));
        ok.then_some(Radial { knots, values })
    }

    /// A table as given (Lensfun's corrections: worked out, not read from a file).
    pub(crate) fn raw(knots: Vec<f32>, values: Vec<f32>) -> Radial {
        Radial { knots, values }
    }

    pub fn eval(&self, r: f32) -> f32 {
        let k = &self.knots;
        let i = k.partition_point(|x| *x <= r);
        if i == 0 {
            return self.values[0];
        }
        if i == k.len() {
            return self.values[k.len() - 1];
        }
        let t = (r - k[i - 1]) / (k[i] - k[i - 1]);
        self.values[i - 1] + (self.values[i] - self.values[i - 1]) * t
    }
}

#[derive(Clone, Debug, Default)]
pub struct LensCorrection {
    /// Gain restoring the light at each radius (1 = none).
    pub vignetting: Option<Radial>,
    /// Source radius over output radius (green; 1 = none).
    pub distortion: Option<Radial>,
    /// Extra source radius scale for red and blue relative to green.
    pub chromatic: Option<[Radial; 2]>,
}

/// Lightroom applies Fujifilm's vignetting about 15% weaker in log gain
/// (RAWmakase's measurement on X100F photos: corners within 0.03 EV at 0.85).
const FUJI_VIGNETTE_STRENGTH: f32 = 0.85;

/// A TIFF block inside a larger file: byte order and offsets from `base`.
struct Tiff<'a> {
    d: &'a [u8],
    base: usize,
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.d.get(self.base + at..self.base + at + 2)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.d.get(self.base + at..self.base + at + 4)?;
        Some(if self.le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    }
    /// (type, count, value-or-offset field position) of a tag in the IFD at `ifd`.
    fn entry(&self, ifd: usize, tag: u16) -> Option<(u16, usize, usize)> {
        let n = self.u16(ifd)? as usize;
        (0..n.min(1024)).find_map(|i| {
            let e = ifd + 2 + i * 12;
            (self.u16(e)? == tag).then(|| Some((self.u16(e + 2)?, self.u32(e + 4)? as usize, e + 8))).flatten()
        })
    }
    /// A signed-rational tag's values.
    fn rationals(&self, ifd: usize, tag: u16) -> Option<Vec<f32>> {
        let (ty, count, field) = self.entry(ifd, tag)?;
        if ty != 10 || count == 0 || count > 256 {
            return None;
        }
        let at = self.u32(field)? as usize;
        (0..count)
            .map(|i| {
                let (n, d) = (self.u32(at + i * 8)? as i32, self.u32(at + i * 8 + 4)? as i32);
                Some(if d == 0 { 0.0 } else { n as f32 / d as f32 })
            })
            .collect()
    }
}

/// A Fujifilm RAF's built-in corrections, if it stores any.
pub fn fujifilm(raf: &[u8]) -> Option<LensCorrection> {
    if !raf.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let base = u32::from_be_bytes(raf.get(100..104)?.try_into().ok()?) as usize;
    let le = match raf.get(base..base + 2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { d: raf, base, le };
    let ifd0 = t.u32(4)? as usize;
    let (_, _, field) = t.entry(ifd0, 0xF000)?;
    let fuji = t.u32(field)? as usize;
    // [scale, n knots, n values]
    let pairs = |v: &[f32], f: &dyn Fn(f32) -> f32| -> Option<Radial> {
        let n = v.len().checked_sub(1)? / 2;
        (n >= 2 && v.len() == 2 * n + 1).then(|| Radial::new(v[1..=n].to_vec(), v[n + 1..].iter().map(|x| f(*x)).collect()))?
    };
    let vignetting = t
        .rationals(fuji, 0xF010)
        .and_then(|v| pairs(&v, &|p| (100.0 / p).powf(FUJI_VIGNETTE_STRENGTH)))
        .filter(|r| r.values.iter().any(|v| (v - 1.0).abs() > 1e-4));
    let distortion = t
        .rationals(fuji, 0xF00B)
        .and_then(|v| pairs(&v, &|p| 1.0 + p / 100.0))
        .filter(|r| r.values.iter().any(|v| (v - 1.0).abs() > 1e-6));
    // [scale, n knots, n red, n blue]
    let chromatic = t.rationals(fuji, 0xF00F).and_then(|v| {
        let n = v.len().checked_sub(1)? / 3;
        if n < 2 || v.len() != 3 * n + 1 {
            return None;
        }
        let knots = v[1..=n].to_vec();
        let red = Radial::new(knots.clone(), v[n + 1..=2 * n].iter().map(|x| 1.0 + x).collect())?;
        let blue = Radial::new(knots, v[2 * n + 1..].iter().map(|x| 1.0 + x).collect())?;
        [&red, &blue].iter().any(|r| r.values.iter().any(|v| (v - 1.0).abs() > 1e-7)).then_some([red, blue])
    });
    let c = LensCorrection { vignetting, distortion, chromatic };
    (c.vignetting.is_some() || c.distortion.is_some() || c.chromatic.is_some()).then_some(c)
}

impl LensCorrection {
    /// Source radius scale for red, green and blue at output radius `r`.
    fn scale(&self, r: f32) -> [f32; 3] {
        let g = self.distortion.as_ref().map_or(1.0, |d| d.eval(r));
        match &self.chromatic {
            Some([red, blue]) => [g * red.eval(r), g, g * blue.eval(r)],
            None => [g; 3],
        }
    }

    /// Output radii are scaled by this so every corrected corner still
    /// samples inside the frame (Lightroom crops the undefined border).
    fn fill(&self) -> f32 {
        if self.distortion.is_none() && self.chromatic.is_none() {
            return 1.0;
        }
        1.0 / (0..=64).flat_map(|i| self.scale(i as f32 / 64.0)).fold(1.0f32, f32::max)
    }

    /// Where an output pixel (x, y) of a `w` x `h` photo comes from, per channel, and the
    /// vignetting gain there: for a part of the photo cut from the full-size image
    /// (`zoom::render_part`), corrected as the whole photo is.
    pub fn source_at(&self, x: f32, y: f32, w: usize, h: usize) -> ([(f32, f32); 3], f32) {
        let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
        let half_diag = (cx * cx + cy * cy).sqrt().max(1.0);
        let fill = self.fill();
        let (dx, dy) = ((x - cx) * fill, (y - cy) * fill);
        let s = self.scale((dx * dx + dy * dy).sqrt() / half_diag);
        let at: [(f32, f32); 3] = std::array::from_fn(|ch| (cx + dx * s[ch], cy + dy * s[ch]));
        let (gx, gy) = (at[1].0 - cx, at[1].1 - cy);
        let gain = self.vignetting.as_ref().map_or(1.0, |v| v.eval((gx * gx + gy * gy).sqrt() / half_diag));
        (at, gain)
    }

    /// Applies the correction to a scene image (linear light): the vignetting
    /// gain at each source position, then the radial remap per channel.
    pub fn apply(&self, img: &SceneImage) -> SceneImage {
        use rayon::prelude::*;
        let (w, h) = (img.width, img.height);
        let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
        let half_diag = (cx * cx + cy * cy).sqrt().max(1.0);
        // Vignetting first, on the source pixels.
        let lit: Vec<[f32; 3]> = match &self.vignetting {
            Some(v) => img
                .px
                .par_iter()
                .enumerate()
                .map(|(i, p)| {
                    let (x, y) = ((i % w) as f32 - cx, (i / w) as f32 - cy);
                    let g = v.eval((x * x + y * y).sqrt() / half_diag);
                    p.map(|c| c * g)
                })
                .collect(),
            None => img.px.clone(),
        };
        if self.distortion.is_none() && self.chromatic.is_none() {
            return SceneImage { width: w, height: h, px: lit, source_long: img.source_long };
        }
        let fill = self.fill();
        let sample = |ch: usize, x: f32, y: f32| -> f32 {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let at = |xx: usize, yy: usize| lit[yy * w + xx][ch];
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            top + (bottom - top) * fy
        };
        let px = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (dx, dy) = (((i % w) as f32 - cx) * fill, ((i / w) as f32 - cy) * fill);
                let s = self.scale((dx * dx + dy * dy).sqrt() / half_diag);
                std::array::from_fn(|ch| sample(ch, cx + dx * s[ch], cy + dy * s[ch]))
            })
            .collect();
        SceneImage { width: w, height: h, px, source_long: img.source_long }
    }
}

/// Nikon's built-in distortion data (NEF maker note tag 0x002B, DistortInfo):
/// the correction flag and three radial coefficients.
#[derive(Clone, Copy, Debug)]
pub struct NikonDistortion {
    pub version: [u8; 4],
    pub correction: u8,
    pub k: [f32; 3],
}

/// Reads a NEF's DistortInfo: IFD0 -> Exif IFD (0x8769) -> maker note
/// (0x927C: "Nikon\0", 4 bytes, then a TIFF block whose offsets count from
/// its own start) -> tag 0x002B.
pub fn nikon_distortion(nef: &[u8]) -> Option<NikonDistortion> {
    let le = match nef.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { d: nef, base: 0, le };
    let ifd0 = t.u32(4)? as usize;
    let (_, _, f) = t.entry(ifd0, 0x8769)?;
    let exif = t.u32(f)? as usize;
    let (_, _, f) = t.entry(exif, 0x927C)?;
    let mn = t.u32(f)? as usize;
    if nef.get(mn..mn + 6)? != b"Nikon\0" {
        return None;
    }
    let base = mn + 10;
    let le = match nef.get(base..base + 2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let m = Tiff { d: nef, base, le };
    let ifd = m.u32(4)? as usize;
    let (_, count, f) = m.entry(ifd, 0x002B)?;
    if count < 0x2C {
        return None;
    }
    let at = m.u32(f)? as usize;
    let b = |i: usize| nef.get(base + at + i).copied();
    let rational = |i: usize| -> Option<f32> {
        let (n, d) = (m.u32(at + i)? as i32, m.u32(at + i + 4)? as i32);
        Some(if d == 0 { 0.0 } else { n as f32 / d as f32 })
    };
    Some(NikonDistortion {
        version: [b(0)?, b(1)?, b(2)?, b(3)?],
        correction: b(4)?,
        k: [rational(0x14)?, rational(0x1C)?, rational(0x24)?],
    })
}
