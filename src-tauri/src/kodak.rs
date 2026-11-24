//! Kodak DCR facts our RAW decoder doesn't read: the camera's as-shot white
//! balance.
//!
//! rawler only reads Kodak's "WB set in software" table (Kodak IFD tag 1021).
//! Most DCS Pro 14n / 14nx / SLR/n and DCS 760C files don't have it, so they
//! came out green or yellow. Those cameras store the balance elsewhere in the
//! Kodak IFD (TIFF tag 0x8290), as the long-standing open-source readers
//! describe it:
//! - tag 1020: the white balance index (`wbi`) the camera used;
//! - tag 2120 + wbi: three "2048 / multiplier" values;
//! - tags 2130 + wbi / 2140 + wbi with 2118: a colour-temperature polynomial;
//! - tag 64013: the index into tags 64037..64042, each three multipliers.
//!
//! Read in that order, later tags winning, as those readers do.

use std::path::Path;

use rawler::RawImage;

/// A TIFF file's byte order and reads.
struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn new(d: &[u8]) -> Option<Tiff<'_>> {
        match d.get(0..2)? {
            b"II" => Some(Tiff { d, le: true }),
            b"MM" => Some(Tiff { d, le: false }),
            _ => None,
        }
    }
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.d.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.d.get(at..at + 4)?;
        Some(if self.le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    }
    fn u64(&self, at: usize) -> Option<u64> {
        let b: [u8; 8] = self.d.get(at..at + 8)?.try_into().ok()?;
        Some(if self.le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) })
    }

    /// The entries of the IFD at `ifd`: (tag, type, count, where the value is).
    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let Some(n) = self.u16(ifd) else { return Vec::new() };
        if n > 1024 {
            return Vec::new();
        }
        (0..n as usize)
            .filter_map(|i| {
                let e = ifd + 2 + i * 12;
                let (tag, typ, count) = (self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?);
                let size = type_size(typ)?.checked_mul(count as usize)?;
                let at = if size <= 4 { e + 8 } else { self.u32(e + 8)? as usize };
                Some(Entry { tag, typ, count, at })
            })
            .collect()
    }

    /// Value `i` of an entry as a whole number (shorts and longs).
    fn int(&self, e: &Entry, i: usize) -> Option<i64> {
        let size = type_size(e.typ)?;
        let at = e.at + i * size;
        Some(match e.typ {
            1 | 7 => *self.d.get(at)? as i64,
            3 => self.u16(at)? as i64,
            8 => self.u16(at)? as i16 as i64,
            4 => self.u32(at)? as i64,
            9 => self.u32(at)? as i32 as i64,
            _ => self.real(e, i)? as i64,
        })
    }

    /// Value `i` of an entry as a real number (any numeric type).
    fn real(&self, e: &Entry, i: usize) -> Option<f64> {
        let size = type_size(e.typ)?;
        let at = e.at + i * size;
        Some(match e.typ {
            5 => self.u32(at)? as f64 / self.u32(at + 4)?.max(1) as f64,
            10 => self.u32(at)? as i32 as f64 / (self.u32(at + 4)? as i32).max(1) as f64,
            11 => f32::from_bits(self.u32(at)?) as f64,
            12 => f64::from_bits(self.u64(at)?),
            _ => self.int(e, i)? as f64,
        })
    }
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    at: usize,
}

fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

const KODAK_IFD: u16 = 0x8290;
const EXIF_IFD: u16 = 0x8769;

/// Where the Kodak IFD starts: tag 0x8290 in IFD0's chain or its Exif IFD.
fn kodak_ifd(t: &Tiff) -> Option<usize> {
    let mut ifd = t.u32(4)? as usize;
    let mut seen = 0;
    while ifd != 0 && seen < 8 {
        let entries = t.entries(ifd);
        for e in &entries {
            if e.tag == KODAK_IFD {
                return Some(t.int(e, 0)? as usize);
            }
        }
        if let Some(exif) = entries.iter().find(|e| e.tag == EXIF_IFD).and_then(|e| t.int(e, 0)) {
            if let Some(e) = t.entries(exif as usize).iter().find(|e| e.tag == KODAK_IFD) {
                return Some(t.int(e, 0)? as usize);
            }
        }
        ifd = t.u32(ifd + 2 + 12 * t.u16(ifd)? as usize)? as usize;
        seen += 1;
    }
    None
}

/// The as-shot white balance multipliers (R, G, B) of a Kodak DCR's bytes,
/// normalised to green = 1. None if the file isn't a TIFF with a Kodak IFD
/// or stores no usable balance.
pub fn white_balance(file: &[u8]) -> Option<[f32; 3]> {
    let t = Tiff::new(file)?;
    let ifd = kodak_ifd(&t)?;
    const WBTAG: [i64; 7] = [64037, 64040, 64039, 64041, -1, -1, 64042];
    let (mut wbi, mut wbtemp) = (-2i64, 6500.0f64);
    let mut mul = [1.0f64; 3];
    let mut cam: Option<[f64; 3]> = None;
    for e in t.entries(ifd) {
        let tag = e.tag as i64;
        let three = |f: &dyn Fn(usize) -> Option<f64>| -> Option<[f64; 3]> { Some([f(0)?, f(1)?, f(2)?]) };
        if tag == 1020 {
            wbi = t.int(&e, 0).unwrap_or(-2);
        }
        if tag == 1021 && e.count == 72 {
            // White balance set in software: three shorts 40 bytes in.
            if let Some(v) = three(&|c| t.u16(e.at + 40 + 2 * c).map(|v| 2048.0 / (v.max(1) as f64))) {
                cam = Some(v);
            }
            wbi = -2;
        }
        if tag == 2118 {
            wbtemp = t.int(&e, 0).unwrap_or(6500) as f64;
        }
        if wbi >= 0 && tag == 2120 + wbi && e.count >= 3 {
            if let Some(v) = three(&|c| t.real(&e, c).map(|v| 2048.0 / v.max(1.0))) {
                cam = Some(v);
            }
        }
        if tag == 2130 + wbi && e.count >= 3 {
            if let Some(v) = three(&|c| t.real(&e, c)) {
                mul = v;
            }
        }
        if wbi >= 0 && tag == 2140 + wbi && e.count >= 12 {
            // Four polynomial terms per colour, in the colour temperature / 100.
            let v = three(&|c| {
                let num: f64 = (0..4).map(|i| Some(t.real(&e, c * 4 + i)? * (wbtemp / 100.0).powi(i as i32))).sum::<Option<f64>>()?;
                Some(2048.0 / (num * mul[c]))
            });
            if let Some(v) = v {
                cam = Some(v);
            }
        }
        if tag == 64013 {
            wbi = t.d.get(e.at).copied().map(|b| b as i64).unwrap_or(wbi);
        }
        if (0..7).contains(&wbi) && tag == WBTAG[wbi as usize] && e.count >= 3 {
            if let Some(v) = three(&|c| t.u32(e.at + 4 * c).map(|v| v as f64)) {
                cam = Some(v);
            }
        }
    }
    let [r, g, b] = cam?;
    let ok = |v: f64| v.is_finite() && v > 0.0;
    (ok(r) && ok(g) && ok(b)).then(|| [(r / g) as f32, 1.0, (b / g) as f32])
}

/// Fills in a Kodak RAW's white balance when the decoder found none, so it
/// develops with the camera's as-shot colour instead of a strong cast. Other
/// makes, and Kodak files the decoder already read, are left as they are.
pub fn fill_white_balance(raw: &mut RawImage, path: &Path) {
    let missing = raw.wb_coeffs[0].is_nan() || raw.wb_coeffs[1] <= 0.0 || raw.wb_coeffs[1].is_nan();
    if !missing || !raw.clean_make.eq_ignore_ascii_case("kodak") {
        return;
    }
    if let Some([r, g, b]) = std::fs::read(path).ok().and_then(|d| white_balance(&d)) {
        raw.wb_coeffs = [r, g, b, f32::NAN];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian TIFF: IFD0 with the Kodak IFD tag, then the Kodak IFD.
    fn tiff(kodak: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut d = b"II*\0".to_vec();
        d.extend(8u32.to_le_bytes());
        // IFD0: one entry, then next-IFD 0.
        d.extend(1u16.to_le_bytes());
        d.extend(KODAK_IFD.to_le_bytes());
        d.extend(4u16.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        let kodak_at = 8 + 2 + 12 + 4;
        d.extend((kodak_at as u32).to_le_bytes());
        d.extend(0u32.to_le_bytes());
        // Kodak IFD, with out-of-line data placed after it.
        let mut data_at = kodak_at + 2 + kodak.len() * 12 + 4;
        let mut data: Vec<u8> = Vec::new();
        d.extend((kodak.len() as u16).to_le_bytes());
        for (tag, typ, count, bytes) in kodak {
            d.extend(tag.to_le_bytes());
            d.extend(typ.to_le_bytes());
            d.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut v = bytes.clone();
                v.resize(4, 0);
                d.extend(v);
            } else {
                d.extend((data_at as u32).to_le_bytes());
                data.extend(bytes.iter().copied());
                data_at += bytes.len();
            }
        }
        d.extend(0u32.to_le_bytes());
        d.extend(data);
        d
    }

    fn shorts(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn reads_the_index_and_its_multiplier_table() {
        // wbi 1 -> tag 2121 holds 2048 / multiplier.
        let f = tiff(&[(1020, 3, 1, shorts(&[1])), (2121, 3, 3, shorts(&[1024, 2048, 1365]))]);
        let wb = white_balance(&f).unwrap();
        assert!((wb[0] - 2.0).abs() < 1e-3 && wb[1] == 1.0 && (wb[2] - 1.5).abs() < 1e-2, "{wb:?}");
    }

    #[test]
    fn reads_the_64013_index_into_its_tables() {
        // Index 2 -> tag 64039, three longs taken as multipliers.
        let f = tiff(&[(64013, 1, 1, vec![2]), (64039, 4, 3, longs(&[300, 150, 200]))]);
        assert_eq!(white_balance(&f), Some([2.0, 1.0, 200.0 / 150.0]));
    }

    #[test]
    fn none_without_a_balance_or_a_tiff() {
        assert_eq!(white_balance(&tiff(&[(1020, 3, 1, shorts(&[3]))])), None);
        assert_eq!(white_balance(b"FUJIFILMCCD-RAW"), None);
        assert_eq!(white_balance(&[]), None);
    }
}
