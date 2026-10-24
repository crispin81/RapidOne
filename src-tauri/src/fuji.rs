//! Fujifilm RAF facts our RAW decoder doesn't read: the exposure bias the
//! camera applied for its dynamic-range modes (DR200 / DR400 record the RAW a
//! stop or two darker to protect highlights, and say so in the maker note),
//! which Lightroom adds back.
//!
//! The maker note sits in the Exif block of the JPEG preview inside the RAF:
//! "FUJIFILM", then the offset of a little-endian IFD whose value offsets are
//! counted from the start of the maker note.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FujiExposure {
    /// RawExposureBias (0x9650) in stops: negative when the RAW was recorded darker.
    pub raw_exposure_bias: Option<f32>,
    /// DevelopmentDynamicRange (0x1403): 100, 200 or 400 (%).
    pub development_dr: Option<u16>,
}

fn u16_be(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn u32_be(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A TIFF block's byte order and reads.
struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.d.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.d.get(at..at + 4)?;
        Some(if self.le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    }
    /// The entry with this tag in the IFD at `ifd`: (type, count, position of
    /// its value-or-offset field).
    fn find(&self, ifd: usize, tag: u16) -> Option<(u16, u32, usize)> {
        let n = self.u16(ifd)? as usize;
        (0..n).find_map(|i| {
            let e = ifd + 2 + i * 12;
            (self.u16(e)? == tag).then(|| Some((self.u16(e + 2)?, self.u32(e + 4)?, e + 8))).flatten()
        })
    }
}

/// RawExposureBias from the RAF's own header directory (offset at byte 92: a
/// count, then entries of tag, size and big-endian data); newer bodies keep it
/// here rather than in the maker note. Two signed 16-bit values (numerator,
/// denominator).
fn header_exposure_bias(raf: &[u8]) -> Option<f32> {
    let dir = u32_be(raf, 92)? as usize;
    let count = u32_be(raf, dir)? as usize;
    let mut p = dir + 4;
    for _ in 0..count.min(256) {
        let (tag, size) = (u16_be(raf, p)?, u16_be(raf, p + 2)? as usize);
        if tag == 0x9650 && size >= 4 {
            let (n, d) = (u16_be(raf, p + 4)? as i16, u16_be(raf, p + 6)? as i16);
            return (d != 0).then(|| n as f32 / d as f32);
        }
        p += 4 + size;
    }
    None
}

/// The exposure facts from a RAF file's bytes; None if it isn't a RAF or has
/// no Fujifilm maker note.
pub fn exposure_info(raf: &[u8]) -> Option<FujiExposure> {
    if !raf.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let header_bias = header_exposure_bias(raf);
    // The header gives the embedded JPEG's offset (big-endian, at byte 84).
    let jpeg_at = u32_be(raf, 84)? as usize;
    let jpeg = raf.get(jpeg_at..)?;
    // Walk the JPEG markers to APP1 "Exif".
    let mut p = 2;
    let exif = loop {
        if jpeg.get(p)? != &0xFF {
            return None;
        }
        let marker = *jpeg.get(p + 1)?;
        let len = u16_be(jpeg, p + 2)? as usize;
        if marker == 0xE1 && jpeg.get(p + 4..p + 10)? == b"Exif\0\0" {
            break jpeg.get(p + 10..p + 2 + len)?;
        }
        if marker == 0xDA {
            return None;
        }
        p += 2 + len;
    };
    let t = Tiff { d: exif, le: exif.get(0..2)? == b"II" };
    let ifd0 = t.u32(4)? as usize;
    let exif_ifd = t.u32(t.find(ifd0, 0x8769)?.2)? as usize;
    let mn_off = t.u32(t.find(exif_ifd, 0x927C)?.2)? as usize;
    let mn = exif.get(mn_off..)?;
    if !mn.starts_with(b"FUJIFILM") {
        return None;
    }
    let m = Tiff { d: mn, le: true };
    let ifd = m.u32(8)? as usize;
    // RawExposureBias: a signed rational, or a pair of signed 16-bit values
    // (numerator, denominator), depending on the camera's firmware.
    let raw_exposure_bias = m.find(ifd, 0x9650).and_then(|(ty, _count, field)| match ty {
        10 => {
            let at = m.u32(field)? as usize;
            let (n, d) = (m.u32(at)? as i32, m.u32(at + 4)? as i32);
            (d != 0).then(|| n as f32 / d as f32)
        }
        8 => {
            let (n, d) = (m.u16(field)? as i16, m.u16(field + 2)? as i16);
            (d != 0).then(|| n as f32 / d as f32)
        }
        _ => None,
    });
    let development_dr = m.find(ifd, 0x1403).and_then(|(_, _, field)| m.u16(field));
    Some(FujiExposure { raw_exposure_bias: raw_exposure_bias.or(header_bias), development_dr })
}
