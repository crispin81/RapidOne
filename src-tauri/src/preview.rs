use base64::{engine::general_purpose::STANDARD, Engine};
use image::{imageops::FilterType, DynamicImage, GenericImageView};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use std::path::Path;

pub struct ExtractedImage {
    pub image: DynamicImage,
    pub timestamp_ms: i64,
    pub orientation: u16,
}

/// Parses an EXIF-style "YYYY:MM:DD HH:MM:SS" timestamp (optionally with a
/// separate subsecond string) into milliseconds since the Unix epoch.
fn parse_exif_datetime(date_time: &str, sub_sec: Option<&str>) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(date_time, "%Y:%m:%d %H:%M:%S").ok()?;
    let millis = naive.and_utc().timestamp_millis();
    let sub_ms: i64 = sub_sec
        .and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            let padded = format!("{:0<3}", trimmed);
            padded[..3].parse::<i64>().ok()
        })
        .unwrap_or(0);
    Some(millis + sub_ms)
}

fn extract_from_raw(path: &Path) -> Result<ExtractedImage, String> {
    let source = RawSource::new(path).map_err(|e| e.to_string())?;
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();

    let image = decoder
        .preview_image(&source, &params)
        .map_err(|e| e.to_string())?
        .or(
            decoder
                .thumbnail_image(&source, &params)
                .map_err(|e| e.to_string())?,
        )
        .or(
            decoder
                .full_image(&source, &params)
                .map_err(|e| e.to_string())?,
        )
        .ok_or_else(|| "no embedded preview, thumbnail, or full image available".to_string())?;

    let metadata = decoder.raw_metadata(&source, &params).map_err(|e| e.to_string())?;
    let orientation = metadata.exif.orientation.unwrap_or(1);
    let timestamp_ms = metadata
        .exif
        .date_time_original
        .as_deref()
        .and_then(|dt| {
            parse_exif_datetime(dt, metadata.exif.sub_sec_time_original.as_deref())
        })
        .unwrap_or(0);

    Ok(ExtractedImage {
        image,
        timestamp_ms,
        orientation,
    })
}

fn extract_from_jpeg(path: &Path) -> Result<ExtractedImage, String> {
    // Also used for TIFFs (a 16-bit 40 MP one is over the library's default
    // memory guard): the user's own photos, so no limit.
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    reader.no_limits();
    let image = reader.decode().map_err(|e| e.to_string())?;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf_reader = std::io::BufReader::new(&file);
    let exif_reader = exif::Reader::new();
    let (orientation, timestamp_ms) = match exif_reader.read_from_container(&mut buf_reader) {
        Ok(fields) => {
            let orientation = fields
                .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
                .map(|v| v as u16)
                .unwrap_or(1);
            let date_time = fields
                .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
                .map(|f| f.display_value().to_string());
            let sub_sec = fields
                .get_field(exif::Tag::SubSecTimeOriginal, exif::In::PRIMARY)
                .map(|f| f.display_value().to_string());
            let timestamp_ms = date_time
                .and_then(|dt| parse_exif_datetime(&dt, sub_sec.as_deref()))
                .unwrap_or(0);
            (orientation, timestamp_ms)
        }
        Err(_) => (1, 0),
    };

    Ok(ExtractedImage {
        image,
        timestamp_ms,
        orientation,
    })
}

/// Extracts a display-ready image and its capture metadata. Prefers a
/// same-basename JPEG sibling (cheap to decode, already a full image) over
/// pulling the embedded preview out of the RAW file.
pub fn extract_source_image(raw_path: &Path, jpeg_path: Option<&Path>) -> Result<ExtractedImage, String> {
    if let Some(jpeg) = jpeg_path {
        return extract_from_jpeg(jpeg);
    }
    extract_from_raw(raw_path)
}

pub fn apply_orientation(img: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Downscales the image to fit within `max_dim` on its long edge (then turned
/// the right way up), encodes it as JPEG, and returns a base64 data URL plus its
/// final pixel dimensions.
pub fn make_thumb_base64(
    img: &DynamicImage,
    orientation: u16,
    max_dim: u32,
) -> Result<(String, u32, u32), String> {
    // Shrunk first, then turned: no full-size copy (a square fit is the same either way round).
    let (w, h) = img.dimensions();
    let small = if w > max_dim || h > max_dim {
        img.resize(max_dim, max_dim, FilterType::Triangle)
    } else {
        img.clone()
    };
    let resized = apply_orientation(small, orientation);
    let (out_w, out_h) = resized.dimensions();

    let mut bytes: Vec<u8> = Vec::new();
    resized
        .to_rgb8()
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;

    let b64 = STANDARD.encode(&bytes);
    Ok((format!("data:image/jpeg;base64,{}", b64), out_w, out_h))
}
