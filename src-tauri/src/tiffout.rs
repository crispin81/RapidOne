//! The 16-bit TIFF that Export writes, laid out as photo editors and print labs expect one:
//! uncompressed, the whole picture in one strip, the header and tags first and the pixels after,
//! an embedded sRGB profile (what the engine renders), 300 pixels per inch, the photo's camera
//! details (EXIF, GPS) and an XMP packet, with RapidOne as the software.
//!
//! Written by hand rather than with the `tiff` crate, whose encoder puts the tags after the
//! pixels and has no sub-IFDs for EXIF. Little-endian classic TIFF (up to 4 GB: a 100 MP photo is
//! 600 MB). The sRGB profile is built here too (ICC v2, the published sRGB primaries and curve),
//! so no profile file is bundled.
//!
//! The Retouch hand-off (`handoff.rs`) is a cache, not an export, and stays compressed.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Pixels per inch written into the file (no size of its own until it's printed).
pub const PPI: u32 = 300;

/// A tag's value, in the TIFF types used here.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(Vec<u8>),
    /// Stored with its closing NUL.
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
    Undefined(Vec<u8>),
}

impl Value {
    fn kind(&self) -> u16 {
        match self {
            Value::Byte(_) => 1,
            Value::Ascii(_) => 2,
            Value::Short(_) => 3,
            Value::Long(_) => 4,
            Value::Rational(_) => 5,
            Value::Undefined(_) => 7,
            Value::SRational(_) => 10,
        }
    }

    fn count(&self) -> u32 {
        (match self {
            Value::Byte(v) | Value::Undefined(v) => v.len(),
            Value::Ascii(s) => s.len() + 1,
            Value::Short(v) => v.len(),
            Value::Long(v) => v.len(),
            Value::Rational(v) => v.len(),
            Value::SRational(v) => v.len(),
        }) as u32
    }

    fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Value::Byte(v) | Value::Undefined(v) => b.extend_from_slice(v),
            Value::Ascii(s) => {
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
            Value::Short(v) => v.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes())),
            Value::Long(v) => v.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes())),
            Value::Rational(v) => v.iter().for_each(|(n, d)| {
                b.extend_from_slice(&n.to_le_bytes());
                b.extend_from_slice(&d.to_le_bytes());
            }),
            Value::SRational(v) => v.iter().for_each(|(n, d)| {
                b.extend_from_slice(&n.to_le_bytes());
                b.extend_from_slice(&d.to_le_bytes());
            }),
        }
        b
    }
}

/// What a photo's export carries over from the original: tags for the main directory, the EXIF
/// directory and the GPS directory, and the few things the XMP packet repeats.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub main: Vec<(u16, Value)>,
    pub exif: Vec<(u16, Value)>,
    pub gps: Vec<(u16, Value)>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    /// "YYYY:MM:DD HH:MM:SS", as EXIF writes it.
    pub date_original: Option<String>,
    /// "+01:00" and the like.
    pub offset_original: Option<String>,
    /// Stars from the photo's .rpd (-1 rejected, 1-5); None for none.
    pub rating: Option<i8>,
}

// Tags taken from the original. Orientation is not: the export is already the right way up.
const MAIN_TAGS: &[u16] = &[271, 272, 315, 33432]; // Make, Model, Artist, Copyright
const EXIF_TAGS: &[u16] = &[
    33434, // ExposureTime
    33437, // FNumber
    34850, // ExposureProgram
    34855, // ISO
    34864, // SensitivityType
    34866, // RecommendedExposureIndex
    36867, // DateTimeOriginal
    36868, // DateTimeDigitized
    36881, // OffsetTimeOriginal
    36882, // OffsetTimeDigitized
    37377, // ShutterSpeedValue
    37378, // ApertureValue
    37380, // ExposureBiasValue
    37381, // MaxApertureValue
    37383, // MeteringMode
    37384, // LightSource
    37385, // Flash
    37386, // FocalLength
    37521, // SubSecTimeOriginal
    37522, // SubSecTimeDigitized
    41986, // ExposureMode
    41987, // WhiteBalance
    41989, // FocalLengthIn35mmFilm
    41990, // SceneCaptureType
    42032, // CameraOwnerName
    42033, // BodySerialNumber
    42034, // LensSpecification
    42035, // LensMake
    42036, // LensModel
    42037, // LensSerialNumber
];

fn text(s: &str) -> Option<String> {
    let t = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!t.is_empty() && t.is_ascii()).then(|| t.to_string())
}

impl Meta {
    /// The original's details: from the RAW's metadata, or a JPEG or TIFF's own EXIF. Anything
    /// unreadable is left out; a photo with none still exports.
    pub fn read(photo: &Path) -> Meta {
        let mut meta = if crate::imagefile::is_image_file(photo) { from_exif_file(photo) } else { from_raw(photo) }.unwrap_or_default();
        meta.rating = crate::sidecar::flags(photo).map(|(r, _)| r).filter(|r| *r != 0);
        meta
    }

    fn put(list: &mut Vec<(u16, Value)>, tag: u16, v: Option<Value>) {
        if let Some(v) = v {
            if v.count() > 0 && !list.iter().any(|(t, _)| *t == tag) {
                list.push((tag, v));
            }
        }
    }
}

fn from_raw(photo: &Path) -> Option<Meta> {
    use rawler::formats::tiff::{Rational, SRational};
    let src = rawler::rawsource::RawSource::new(photo).ok()?;
    let dec = rawler::get_decoder(&src).ok()?;
    let md = dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default()).ok()?;
    let e = &md.exif;
    let ascii = |s: &Option<String>| s.as_deref().and_then(text).map(Value::Ascii);
    let rat = |r: Option<Rational>| r.filter(|r| r.d > 0).map(|r| Value::Rational(vec![(r.n, r.d)]));
    let srat = |r: Option<SRational>| r.filter(|r| r.d != 0).map(|r| Value::SRational(vec![(r.n, r.d)]));
    let short = |v: Option<u16>| v.map(|v| Value::Short(vec![v]));
    let long = |v: Option<u32>| v.map(|v| Value::Long(vec![v]));
    let mut m = Meta::default();
    Meta::put(&mut m.main, 271, text(&md.make).map(Value::Ascii));
    Meta::put(&mut m.main, 272, text(&md.model).map(Value::Ascii));
    Meta::put(&mut m.main, 315, ascii(&e.artist));
    Meta::put(&mut m.main, 33432, ascii(&e.copyright));
    let x = &mut m.exif;
    Meta::put(x, 33434, rat(e.exposure_time));
    Meta::put(x, 33437, rat(e.fnumber));
    Meta::put(x, 34850, short(e.exposure_program));
    let iso = e.iso_speed_ratings.or_else(|| e.iso_speed.or(e.recommended_exposure_index).map(|v| v.min(65535) as u16));
    Meta::put(x, 34855, short(iso));
    Meta::put(x, 34864, short(e.sensitivity_type));
    Meta::put(x, 34866, long(e.recommended_exposure_index));
    Meta::put(x, 36867, ascii(&e.date_time_original));
    Meta::put(x, 36868, ascii(&e.create_date));
    Meta::put(x, 36881, ascii(&e.offset_time_original));
    Meta::put(x, 36882, ascii(&e.offset_time_digitized));
    Meta::put(x, 37377, srat(e.shutter_speed_value));
    Meta::put(x, 37378, rat(e.aperture_value));
    Meta::put(x, 37380, srat(e.exposure_bias));
    Meta::put(x, 37381, rat(e.max_aperture_value));
    Meta::put(x, 37383, short(e.metering_mode));
    Meta::put(x, 37384, short(e.light_source));
    Meta::put(x, 37385, short(e.flash));
    Meta::put(x, 37386, rat(e.focal_length));
    Meta::put(x, 37521, ascii(&e.sub_sec_time_original));
    Meta::put(x, 37522, ascii(&e.sub_sec_time_digitized));
    Meta::put(x, 41986, short(e.exposure_mode));
    Meta::put(x, 41987, short(e.white_balance));
    Meta::put(x, 41990, short(e.scene_capture_type));
    Meta::put(x, 42032, ascii(&e.owner_name));
    Meta::put(x, 42033, ascii(&e.serial_number));
    let spec = e.lens_spec.filter(|s| s.iter().all(|r| r.d > 0)).map(|s| Value::Rational(s.iter().map(|r| (r.n, r.d)).collect()));
    Meta::put(x, 42034, spec);
    let lens_make = e.lens_make.clone().or_else(|| md.lens.as_ref().map(|l| l.lens_make.clone()));
    let lens_model = e.lens_model.clone().or_else(|| md.lens.as_ref().map(|l| l.lens_model.clone()));
    Meta::put(x, 42035, ascii(&lens_make));
    Meta::put(x, 42036, ascii(&lens_model));
    Meta::put(x, 42037, ascii(&e.lens_serial_number));
    if let Some(g) = &e.gps {
        let g3 = |v: Option<[Rational; 3]>| v.filter(|v| v.iter().all(|r| r.d > 0)).map(|v| Value::Rational(v.iter().map(|r| (r.n, r.d)).collect()));
        // Only a position that's really there: both halves, with their N/S and E/W.
        if g.gps_latitude.is_some() && g.gps_longitude.is_some() && g.gps_latitude_ref.is_some() && g.gps_longitude_ref.is_some() {
            let p = &mut m.gps;
            Meta::put(p, 0, Some(Value::Byte(g.gps_version_id.map(|v| v.to_vec()).unwrap_or_else(|| vec![2, 3, 0, 0]))));
            Meta::put(p, 1, ascii(&g.gps_latitude_ref));
            Meta::put(p, 2, g3(g.gps_latitude));
            Meta::put(p, 3, ascii(&g.gps_longitude_ref));
            Meta::put(p, 4, g3(g.gps_longitude));
            Meta::put(p, 5, g.gps_altitude_ref.map(|v| Value::Byte(vec![v])));
            Meta::put(p, 6, rat(g.gps_altitude));
            Meta::put(p, 7, g3(g.gps_timestamp));
            Meta::put(p, 29, ascii(&g.gps_date_stamp));
        }
    }
    m.artist = e.artist.as_deref().and_then(text);
    m.copyright = e.copyright.as_deref().and_then(text);
    m.date_original = e.date_time_original.as_deref().and_then(text);
    m.offset_original = e.offset_time_original.as_deref().and_then(text);
    Some(m)
}

/// A JPEG or TIFF original: its EXIF copied across, tag by tag, for the tags listed above (and
/// all of GPS's own).
fn from_exif_file(photo: &Path) -> Option<Meta> {
    let f = File::open(photo).ok()?;
    let ex = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)).ok()?;
    let mut m = Meta::default();
    for field in ex.fields().filter(|f| f.ifd_num == exif::In::PRIMARY) {
        let n = field.tag.number();
        // A tag of the wrong type (some writers get rationals wrong) is left out rather than copied.
        let v = convert(&field.value).filter(|v| match n {
            33434 | 33437 | 37378 | 37381 | 37386 | 42034 => matches!(v, Value::Rational(_)),
            37377 | 37380 => matches!(v, Value::SRational(_)),
            _ => true,
        });
        match field.tag.context() {
            exif::Context::Tiff if MAIN_TAGS.contains(&n) => Meta::put(&mut m.main, n, v),
            exif::Context::Exif if EXIF_TAGS.contains(&n) => Meta::put(&mut m.exif, n, v),
            exif::Context::Gps => Meta::put(&mut m.gps, n, v),
            _ => {}
        }
    }
    let get = |list: &[(u16, Value)], tag: u16| {
        list.iter().find(|(t, _)| *t == tag).and_then(|(_, v)| if let Value::Ascii(s) = v { Some(s.clone()) } else { None })
    };
    m.artist = get(&m.main, 315);
    m.copyright = get(&m.main, 33432);
    m.date_original = get(&m.exif, 36867);
    m.offset_original = get(&m.exif, 36881);
    // GPS with no position is no use (and a half one is worse).
    if !(m.gps.iter().any(|(t, _)| *t == 2) && m.gps.iter().any(|(t, _)| *t == 4)) {
        m.gps.clear();
    }
    Some(m)
}

fn convert(v: &exif::Value) -> Option<Value> {
    Some(match v {
        exif::Value::Byte(b) => Value::Byte(b.clone()),
        exif::Value::Undefined(b, _) => Value::Undefined(b.clone()),
        exif::Value::Ascii(parts) => Value::Ascii(text(&String::from_utf8_lossy(parts.first()?))?),
        exif::Value::Short(s) => Value::Short(s.clone()),
        exif::Value::Long(l) => Value::Long(l.clone()),
        exif::Value::Rational(r) => Value::Rational(r.iter().map(|r| (r.num, r.denom)).collect()),
        exif::Value::SRational(r) => Value::SRational(r.iter().map(|r| (r.num, r.denom)).collect()),
        _ => return None,
    })
}

/// One directory: its tags in order, and where it goes in the file.
struct Ifd {
    tags: Vec<(u16, Value)>,
}

impl Ifd {
    fn new(mut tags: Vec<(u16, Value)>) -> Ifd {
        tags.sort_by_key(|(t, _)| *t);
        tags.dedup_by_key(|(t, _)| *t);
        Ifd { tags }
    }

    /// Bytes it takes: the entries, then the values too big to sit in an entry (each on an even offset).
    fn size(&self) -> usize {
        let mut n = 2 + 12 * self.tags.len() + 4;
        for (_, v) in &self.tags {
            let b = v.bytes().len();
            if b > 4 {
                n += b + (b & 1);
            }
        }
        n
    }

    fn set(&mut self, tag: u16, v: Value) {
        if let Some(e) = self.tags.iter_mut().find(|(t, _)| *t == tag) {
            e.1 = v;
        }
    }

    /// The directory as it sits at offset `at`.
    fn encode(&self, at: usize) -> Vec<u8> {
        let mut dir = Vec::with_capacity(self.size());
        let mut data = Vec::new();
        let data_at = at + 2 + 12 * self.tags.len() + 4;
        dir.extend_from_slice(&(self.tags.len() as u16).to_le_bytes());
        for (tag, v) in &self.tags {
            dir.extend_from_slice(&tag.to_le_bytes());
            dir.extend_from_slice(&v.kind().to_le_bytes());
            dir.extend_from_slice(&v.count().to_le_bytes());
            let mut b = v.bytes();
            if b.len() <= 4 {
                b.resize(4, 0);
                dir.extend_from_slice(&b);
            } else {
                dir.extend_from_slice(&((data_at + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(&b);
                if b.len() & 1 == 1 {
                    data.push(0);
                }
            }
        }
        dir.extend_from_slice(&0u32.to_le_bytes());
        dir.extend_from_slice(&data);
        dir
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// "2026:11:15 20:41:07" (and "+01:00") as XMP's "2026-11-15T20:41:07+01:00".
fn xmp_date(exif: &str, offset: Option<&str>) -> Option<String> {
    let b = exif.as_bytes();
    if b.len() < 19 || b[4] != b':' || b[7] != b':' || b[10] != b' ' {
        return None;
    }
    let off = offset.filter(|o| o.len() == 6 && (o.starts_with('+') || o.starts_with('-'))).unwrap_or("");
    Some(format!("{}-{}-{}T{}{off}", &exif[0..4], &exif[5..7], &exif[8..10], &exif[11..19]))
}

/// The XMP packet: the software, the dates, stars, creator and rights.
fn xmp(meta: &Meta, software: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    let now = now.format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let mut attrs = format!(
        "\n   xmp:CreatorTool=\"{}\"\n   xmp:ModifyDate=\"{now}\"\n   xmp:MetadataDate=\"{now}\"\n   photoshop:ColorMode=\"3\"\n   photoshop:ICCProfile=\"sRGB IEC61966-2.1\"",
        xml(software)
    );
    if let Some(d) = meta.date_original.as_deref().and_then(|d| xmp_date(d, meta.offset_original.as_deref())) {
        attrs += &format!("\n   xmp:CreateDate=\"{d}\"\n   photoshop:DateCreated=\"{d}\"");
    }
    if let Some(r) = meta.rating {
        attrs += &format!("\n   xmp:Rating=\"{r}\"");
    }
    let mut body = String::new();
    if let Some(a) = &meta.artist {
        body += &format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", xml(a));
    }
    if let Some(c) = &meta.copyright {
        body += &format!("   <dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:rights>\n", xml(c));
    }
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\"\n   xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n   xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n   xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\"{attrs}>\n{body}  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
}

/// The software tag: "RapidOne 1.2.2".
pub fn software() -> String {
    format!("RapidOne {}", env!("CARGO_PKG_VERSION"))
}

/// Writes `img` (16-bit sRGB, as the engine renders) to `path` as described at the top.
pub fn write(path: &Path, img: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>, meta: &Meta) -> Result<(), String> {
    let (w, h) = img.dimensions();
    let pixel_bytes = w as u64 * h as u64 * 6;
    let sw = software();
    let now = chrono::Local::now();
    let mut main = vec![
        (256, Value::Long(vec![w])),
        (257, Value::Long(vec![h])),
        (258, Value::Short(vec![16, 16, 16])),
        (259, Value::Short(vec![1])), // no compression
        (262, Value::Short(vec![2])), // RGB
        (273, Value::Long(vec![0])),  // the strip's offset, filled in below
        (274, Value::Short(vec![1])), // the right way up
        (277, Value::Short(vec![3])),
        (278, Value::Long(vec![h])), // one strip
        (279, Value::Long(vec![pixel_bytes.min(u32::MAX as u64) as u32])),
        (282, Value::Rational(vec![(PPI, 1)])),
        (283, Value::Rational(vec![(PPI, 1)])),
        (284, Value::Short(vec![1])), // chunky (RGBRGB...)
        (296, Value::Short(vec![2])), // inches
        (305, Value::Ascii(sw.clone())),
        (306, Value::Ascii(now.format("%Y:%m:%d %H:%M:%S").to_string())),
        (700, Value::Byte(xmp(meta, &sw, &now).into_bytes())),
        (34675, Value::Undefined(srgb_icc())),
    ];
    main.extend(meta.main.iter().cloned());
    let mut exif = vec![(36864, Value::Undefined(b"0232".to_vec())), (40961, Value::Short(vec![1]))]; // version; sRGB
    exif.extend(meta.exif.iter().cloned());
    let has_gps = !meta.gps.is_empty();
    main.push((34665, Value::Long(vec![0])));
    if has_gps {
        main.push((34853, Value::Long(vec![0])));
    }
    let mut ifd0 = Ifd::new(main);
    let exif = Ifd::new(exif);
    let gps = Ifd::new(meta.gps.clone());
    // Header (8), the main directory, EXIF's, GPS's, then the pixels on a 16-byte boundary.
    let exif_at = 8 + ifd0.size();
    let gps_at = exif_at + exif.size();
    let end = gps_at + if has_gps { gps.size() } else { 0 };
    let pixels_at = end.div_ceil(16) * 16;
    if pixels_at as u64 + pixel_bytes > u32::MAX as u64 {
        return Err("This photo is too big for a TIFF at this size (over 4 GB). Export it with a smaller long edge.".into());
    }
    ifd0.set(273, Value::Long(vec![pixels_at as u32]));
    ifd0.set(34665, Value::Long(vec![exif_at as u32]));
    ifd0.set(34853, Value::Long(vec![gps_at as u32]));

    let mut out = BufWriter::with_capacity(1 << 20, File::create(path).map_err(|e| e.to_string())?);
    let mut head = Vec::with_capacity(pixels_at);
    head.extend_from_slice(b"II*\0");
    head.extend_from_slice(&8u32.to_le_bytes());
    head.extend(ifd0.encode(8));
    head.extend(exif.encode(exif_at));
    if has_gps {
        head.extend(gps.encode(gps_at));
    }
    debug_assert_eq!(head.len(), end);
    head.resize(pixels_at, 0);
    out.write_all(&head).map_err(|e| e.to_string())?;
    // The pixels, little-endian, a slice at a time (a full copy of a 100 MP photo would be 600 MB more).
    let mut buf = Vec::with_capacity(1 << 20);
    for chunk in img.as_raw().chunks(1 << 19) {
        buf.clear();
        chunk.iter().for_each(|v| buf.extend_from_slice(&v.to_le_bytes()));
        out.write_all(&buf).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

/// sRGB IEC61966-2.1 as an ICC v2 display profile: its primaries adapted to D50 (Bradford) and
/// its tone curve as a 1024-point table. Built here, so the profile is RapidOne's own.
pub fn srgb_icc() -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut b = b"XYZ \0\0\0\0".to_vec();
        [x, y, z].iter().for_each(|v| b.extend_from_slice(&s15(*v)));
        b
    }
    fn desc(s: &str) -> Vec<u8> {
        let mut b = b"desc\0\0\0\0".to_vec();
        b.extend_from_slice(&(s.len() as u32 + 1).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        b.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]); // no Unicode or ScriptCode names
        b
    }
    let curve = {
        let mut b = b"curv\0\0\0\0".to_vec();
        let n = 1024u32;
        b.extend_from_slice(&n.to_be_bytes());
        for i in 0..n {
            let v = i as f64 / (n - 1) as f64;
            let lin = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            b.extend_from_slice(&((lin * 65535.0).round() as u16).to_be_bytes());
        }
        b
    };
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend_from_slice(b"No copyright, use freely\0");
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("sRGB IEC61966-2.1")),
        (b"cprt", cprt),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz(0.4360747, 0.2225045, 0.0139322)),
        (b"gXYZ", xyz(0.3850649, 0.7168786, 0.0971045)),
        (b"bXYZ", xyz(0.1430804, 0.0606169, 0.7141733)),
        (b"rTRC", curve.clone()),
    ];
    // The three tone curves share one table.
    let table_len = 4 + 12 * (tags.len() + 2);
    let mut data = Vec::new();
    let mut entries = Vec::new();
    let mut curve_at = (0, 0);
    for (sig, body) in &tags {
        let at = 128 + table_len + data.len();
        entries.push((**sig, at as u32, body.len() as u32));
        if *sig == b"rTRC" {
            curve_at = (at as u32, body.len() as u32);
        }
        data.extend_from_slice(body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    entries.push((*b"gTRC", curve_at.0, curve_at.1));
    entries.push((*b"bTRC", curve_at.0, curve_at.1));
    let size = 128 + table_len + data.len();
    let mut p = Vec::with_capacity(size);
    p.extend_from_slice(&(size as u32).to_be_bytes());
    p.extend_from_slice(&[0; 4]); // preferred CMM
    p.extend_from_slice(&[2, 0x10, 0, 0]); // version 2.1
    p.extend_from_slice(b"mntrRGB XYZ ");
    for v in [2026u16, 11, 15, 0, 0, 0] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(b"acsp");
    p.extend_from_slice(&[0; 4 + 4 + 4 + 4 + 8 + 4]); // platform, flags, maker, model, attributes, intent
    [0.9642, 1.0, 0.8249].iter().for_each(|v| p.extend_from_slice(&s15(*v))); // D50
    p.extend_from_slice(&[0; 4 + 16 + 28]); // creator, ID, reserved
    debug_assert_eq!(p.len(), 128);
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (sig, at, len) in entries {
        p.extend_from_slice(&sig);
        p.extend_from_slice(&at.to_be_bytes());
        p.extend_from_slice(&len.to_be_bytes());
    }
    p.extend_from_slice(&data);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> image::ImageBuffer<image::Rgb<u16>, Vec<u16>> {
        image::ImageBuffer::from_fn(w, h, |x, y| image::Rgb([(x * 977 + y) as u16, (y * 613 ^ x) as u16, ((x + y) * 331) as u16]))
    }

    fn sample_meta() -> Meta {
        let mut m = Meta::default();
        m.main.push((271, Value::Ascii("Panasonic".into())));
        m.main.push((272, Value::Ascii("DC-S5M2".into())));
        m.exif.push((33434, Value::Rational(vec![(1, 125)])));
        m.exif.push((33437, Value::Rational(vec![(28, 10)])));
        m.exif.push((34855, Value::Short(vec![800])));
        m.exif.push((36867, Value::Ascii("2026:11:14 21:03:05".into())));
        m.exif.push((42036, Value::Ascii("LUMIX S 20-60/F3.5-5.6".into())));
        m.gps.push((1, Value::Ascii("N".into())));
        m.gps.push((2, Value::Rational(vec![(51, 1), (30, 1), (0, 1)])));
        m.gps.push((3, Value::Ascii("W".into())));
        m.gps.push((4, Value::Rational(vec![(0, 1), (7, 1), (30, 1)])));
        m.artist = Some("A & B".into());
        m.date_original = Some("2026:11:14 21:03:05".into());
        m.rating = Some(4);
        m
    }

    /// Uncompressed, one strip, the tags before the pixels, the pixels exactly as rendered, and
    /// the profile, resolution, software and camera details all readable by another TIFF reader.
    #[test]
    fn writes_an_uncompressed_tagged_tiff_that_reads_back() {
        let dir = std::env::temp_dir().join(format!("rapidone-tiffout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.tif");
        let img = picture(301, 157);
        write(&path, &img, &sample_meta()).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
        use tiff::tags::Tag;
        assert_eq!(d.dimensions().unwrap(), (301, 157));
        assert_eq!(d.get_tag_u32(Tag::Compression).unwrap(), 1);
        let offsets = d.get_tag_u32_vec(Tag::StripOffsets).unwrap();
        assert_eq!(offsets.len(), 1);
        assert!(offsets[0] as usize > 8, "the header and tags come first");
        assert_eq!(bytes.len(), offsets[0] as usize + 301 * 157 * 6, "the pixels last, nothing after");
        assert_eq!(d.get_tag_u32(Tag::ResolutionUnit).unwrap(), 2);
        assert_eq!(d.get_tag_ascii_string(Tag::Software).unwrap(), software());
        let icc = d.get_tag_u8_vec(Tag::IccProfile).unwrap();
        assert_eq!(icc, srgb_icc());
        let xmp = String::from_utf8(d.get_tag_u8_vec(Tag::Unknown(700)).unwrap()).unwrap();
        assert!(xmp.contains("xmp:Rating=\"4\"") && xmp.contains("A &amp; B") && xmp.contains("2026-11-14T21:03:05"));
        match d.read_image().unwrap() {
            tiff::decoder::DecodingResult::U16(px) => assert_eq!(&px, img.as_raw()),
            _ => panic!("not 16-bit"),
        }

        // EXIF and GPS, as an EXIF reader sees them.
        let ex = exif::Reader::new().read_raw(bytes.clone()).unwrap();
        let get = |t| ex.get_field(t, exif::In::PRIMARY).map(|f| f.display_value().to_string());
        assert_eq!(get(exif::Tag::Model).as_deref(), Some("\"DC-S5M2\""));
        assert_eq!(get(exif::Tag::PhotographicSensitivity).as_deref(), Some("800"));
        assert_eq!(get(exif::Tag::LensModel).as_deref(), Some("\"LUMIX S 20-60/F3.5-5.6\""));
        assert_eq!(get(exif::Tag::ColorSpace).as_deref(), Some("sRGB"));
        assert_eq!(get(exif::Tag::XResolution).as_deref(), Some("300"));
        assert!(get(exif::Tag::GPSLatitude).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// No camera details at all (a picture with no EXIF): still a good file, with no GPS directory.
    #[test]
    fn writes_without_metadata() {
        let dir = std::env::temp_dir().join(format!("rapidone-tiffout-bare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bare.tif");
        let img = picture(33, 17);
        write(&path, &img, &Meta::default()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
        assert!(d.get_tag_u32(tiff::tags::Tag::Unknown(34853)).is_err());
        match d.read_image().unwrap() {
            tiff::decoder::DecodingResult::U16(px) => assert_eq!(&px, img.as_raw()),
            _ => panic!("not 16-bit"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exif_dates_become_xmp_dates() {
        assert_eq!(xmp_date("2026:11:14 21:03:05", Some("+01:00")).as_deref(), Some("2026-11-14T21:03:05+01:00"));
        assert_eq!(xmp_date("2026:11:14 21:03:05", None).as_deref(), Some("2026-11-14T21:03:05"));
        assert_eq!(xmp_date("garbage", None), None);
    }
}
