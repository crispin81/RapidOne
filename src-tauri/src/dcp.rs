//! Camera profiles in the DNG Camera Profile (.dcp) format, and the "look"
//! profiles Lightroom stores as .xmp (e.g. Adobe Color = the camera's Adobe
//! Standard .dcp plus a look table and tone curve).
//!
//! Written from the public DNG specification. The profile files themselves
//! belong to their authors: RapidOne only reads the ones already installed
//! on the user's machine (by Lightroom, Camera Raw or the free DNG
//! Converter) and never ships or copies them.

use std::path::Path;

/// A 3x3 matrix, row-major.
pub type Mat3 = [[f32; 3]; 3];

/// A hue/saturation/value table: per (hue, saturation, value) grid point, a
/// hue shift in degrees, a saturation scale and a value scale.
#[derive(Clone, Debug)]
pub struct HsvTable {
    pub hue_divs: usize,
    pub sat_divs: usize,
    pub val_divs: usize,
    /// (hue_shift_degrees, sat_scale, val_scale), indexed [val][hue][sat].
    pub data: Vec<[f32; 3]>,
    /// 1 = values are indexed in sRGB gamma rather than linearly.
    pub srgb_gamma: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub name: String,
    pub camera_model: String,
    /// EXIF LightSource codes (17 = Standard light A, 21 = D65, ...).
    pub illuminant1: u16,
    pub illuminant2: u16,
    pub color_matrix1: Option<Mat3>,
    pub color_matrix2: Option<Mat3>,
    pub forward_matrix1: Option<Mat3>,
    pub forward_matrix2: Option<Mat3>,
    pub hue_sat_map1: Option<HsvTable>,
    pub hue_sat_map2: Option<HsvTable>,
    pub look_table: Option<HsvTable>,
    /// (input, output) pairs, both 0..1.
    pub tone_curve: Vec<(f32, f32)>,
    pub baseline_exposure_offset: f32,
    /// 0 = auto (Lightroom may lift blacks), 1 = none.
    pub default_black_render: u32,
}

struct Reader<'a> {
    b: &'a [u8],
    little: bool,
}

impl Reader<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at + 2)?;
        Some(if self.little { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    }
}

/// TIFF field types and their sizes.
fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

struct Entry {
    tag: u16,
    typ: u16,
    count: usize,
    /// Byte offset of the value (inline or pointed to).
    at: usize,
}

impl Entry {
    fn numbers(&self, r: &Reader) -> Vec<f64> {
        (0..self.count)
            .filter_map(|i| {
                let at = self.at + i * type_size(self.typ);
                Some(match self.typ {
                    1 | 7 => *r.b.get(at)? as f64,
                    6 => *r.b.get(at)? as i8 as f64,
                    3 => r.u16(at)? as f64,
                    8 => r.u16(at)? as i16 as f64,
                    4 => r.u32(at)? as f64,
                    9 => r.u32(at)? as i32 as f64,
                    11 => f32::from_bits(r.u32(at)?) as f64,
                    5 => r.u32(at)? as f64 / r.u32(at + 4)?.max(1) as f64,
                    10 => r.u32(at)? as i32 as f64 / (r.u32(at + 4)? as i32).max(1) as f64,
                    12 => {
                        let lo = r.u32(at)? as u64;
                        let hi = r.u32(at + 4)? as u64;
                        f64::from_bits(if r.little { hi << 32 | lo } else { lo << 32 | hi })
                    }
                    _ => return None,
                })
            })
            .collect()
    }

    fn text(&self, r: &Reader) -> String {
        let bytes = r.b.get(self.at..self.at + self.count).unwrap_or(&[]);
        String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string()
    }
}

fn mat3(v: &[f64]) -> Option<Mat3> {
    (v.len() == 9).then(|| {
        let f = |i: usize| v[i] as f32;
        [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]]
    })
}

fn hsv_table(dims: &[f64], data: &[f64], encoding: u32) -> Option<HsvTable> {
    // A 2-D table (no value axis) has value divisions 0 or 1.
    let (h, s, v) = (*dims.first()? as usize, *dims.get(1)? as usize, dims.get(2).copied().unwrap_or(1.0).max(1.0) as usize);
    if h == 0 || s == 0 || data.len() != h * s * v * 3 {
        return None;
    }
    Some(HsvTable {
        hue_divs: h,
        sat_divs: s,
        val_divs: v,
        data: data.chunks(3).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect(),
        srgb_gamma: encoding == 1,
    })
}

/// A look profile (.xmp, e.g. Adobe Color): a base camera profile by name
/// ("Adobe Standard"), plus its own look table and tone curve.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub name: String,
    /// The camera profile it builds on, e.g. "Adobe Standard".
    pub base_profile: String,
    pub look_table: Option<HsvTable>,
    /// Point curve, (input, output) on 0..255.
    pub tone_curve: Vec<(f32, f32)>,
}

/// Look profiles store tables as text: base-85 groups of five characters
/// (the Z85 alphabet, with ` ' | in place of & < >, which XML can't hold;
/// least significant digit first) giving little-endian 32-bit words. The
/// bytes are a 4-byte size, then zlib data.
fn decode_table_text(s: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";
    let mut digit = [u8::MAX; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        digit[c as usize] = i as u8;
    }
    let mut bytes = Vec::with_capacity(s.len() / 5 * 4);
    for group in s.as_bytes().chunks(5) {
        let mut v: u64 = 0;
        for &c in group.iter().rev() {
            let d = digit[c as usize];
            if d == u8::MAX {
                return None;
            }
            v = v * 85 + d as u64;
        }
        bytes.extend_from_slice(&(v as u32).to_le_bytes());
    }
    let size = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let mut out = Vec::with_capacity(size);
    flate2::read::ZlibDecoder::new(bytes.get(4..)?).read_to_end(&mut out).ok()?;
    (out.len() == size).then_some(out)
}

/// The decoded table: five little-endian words (0, version 1, hue, sat and
/// value divisions), the (hue shift, sat scale, val scale) floats, then one
/// word for the encoding (1 = sRGB-gamma value axis).
fn parse_look_table(b: &[u8]) -> Option<HsvTable> {
    let word = |i: usize| b.get(i * 4..i * 4 + 4).map(|w| u32::from_le_bytes(w.try_into().unwrap()));
    let (h, s, v) = (word(2)? as usize, word(3)? as usize, word(4)? as usize);
    let n = h * s * v.max(1) * 3;
    let enc = word(5 + n)?;
    let data: Vec<f64> = b
        .get(20..20 + n * 4)?
        .chunks_exact(4)
        .map(|w| f32::from_le_bytes(w.try_into().unwrap()) as f64)
        .collect();
    hsv_table(&[h as f64, s as f64, v as f64], &data, enc)
}

impl Look {
    pub fn load(path: &Path) -> Result<Look, String> {
        let xmp = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let attr = |name: &str| {
            let key = format!("crs:{name}=\"");
            let start = xmp.find(&key)? + key.len();
            Some(xmp[start..start + xmp[start..].find('"')?].to_string())
        };
        if attr("PresetType").as_deref() != Some("Look") {
            return Err("not a look profile".into());
        }
        let look_table = attr("LookTable")
            .and_then(|id| attr(&format!("Table_{id}")))
            .and_then(|text| decode_table_text(&text))
            .and_then(|bytes| parse_look_table(&bytes));
        // <crs:ToneCurvePV2012><rdf:Seq><rdf:li>x, y</rdf:li>...
        let tone_curve = xmp
            .find("<crs:ToneCurvePV2012>")
            .map(|start| {
                let block = &xmp[start..start + xmp[start..].find("</crs:ToneCurvePV2012>").unwrap_or(0)];
                block
                    .split("<rdf:li>")
                    .skip(1)
                    .filter_map(|li| {
                        let (x, y) = li.split("</rdf:li>").next()?.split_once(',')?;
                        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The display name is in an rdf:Alt; the file name is a good stand-in.
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        Ok(Look { name, base_profile: attr("CameraProfile").unwrap_or_default(), look_table, tone_curve })
    }
}

impl Profile {
    pub fn load(path: &Path) -> Result<Profile, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
    }

    /// Parses a .dcp: a TIFF-structured file with "IIRC" (little-endian) or
    /// "MMCR" (big-endian) in place of TIFF's magic number.
    pub fn parse(b: &[u8]) -> Result<Profile, String> {
        let little = match b.get(0..4) {
            Some(b"IIRC") => true,
            Some(b"MMCR") => false,
            _ => return Err("not a DNG camera profile".into()),
        };
        let r = Reader { b, little };
        let ifd = r.u32(4).ok_or("truncated")? as usize;
        let n = r.u16(ifd).ok_or("truncated")? as usize;
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let (tag, typ, count) = (r.u16(e).ok_or("truncated")?, r.u16(e + 2).ok_or("truncated")?, r.u32(e + 4).ok_or("truncated")? as usize);
            let size = type_size(typ) * count;
            let at = if size <= 4 { e + 8 } else { r.u32(e + 8).ok_or("truncated")? as usize };
            entries.push(Entry { tag, typ, count, at });
        }
        let get = |tag: u16| entries.iter().find(|e| e.tag == tag);
        let nums = |tag: u16| get(tag).map(|e| e.numbers(&r)).unwrap_or_default();
        let one = |tag: u16| nums(tag).first().copied();

        let hsm_dims = nums(50937);
        let hsm_enc = one(51107).unwrap_or(0.0) as u32;
        let look_dims = nums(50981);
        let look_enc = one(51108).unwrap_or(0.0) as u32;
        let curve = nums(50940);
        Ok(Profile {
            name: get(50936).map(|e| e.text(&r)).unwrap_or_default(),
            camera_model: get(50708).map(|e| e.text(&r)).unwrap_or_default(),
            illuminant1: one(50778).unwrap_or(0.0) as u16,
            illuminant2: one(50779).unwrap_or(0.0) as u16,
            color_matrix1: mat3(&nums(50721)),
            color_matrix2: mat3(&nums(50722)),
            forward_matrix1: mat3(&nums(50964)),
            forward_matrix2: mat3(&nums(50965)),
            hue_sat_map1: hsv_table(&hsm_dims, &nums(50938), hsm_enc),
            hue_sat_map2: hsv_table(&hsm_dims, &nums(50939), hsm_enc),
            look_table: hsv_table(&look_dims, &nums(50982), look_enc),
            tone_curve: curve.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0] as f32, c[1] as f32)).collect(),
            baseline_exposure_offset: one(51109).unwrap_or(0.0) as f32,
            default_black_render: one(51110).unwrap_or(0.0) as u32,
        })
    }
}
