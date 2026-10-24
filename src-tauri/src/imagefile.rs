//! TIFF and JPEG files as photos in Edit.
//!
//! Unlike a RAW these are already toned: sRGB, 8 or 16 bits per channel. The
//! file is turned upright from its EXIF / TIFF orientation tag (values 1-8,
//! as RapidRetouch does), read as sRGB (no embedded-profile conversion yet)
//! and made into the engine's scene: linear light in the Rec.2020 storage
//! space. The scene is flagged `display_referred`, so it renders with the
//! "As is" profile (`Profile::AsIs`): no baseline exposure, tone curve or
//! colour look, and the picture is unchanged until a slider moves. There is
//! no camera white balance, so Temperature and Tint act as a plain warm /
//! cool and green / magenta gain (`pipeline::simple_rebalance`).

use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;

use crate::pipeline::{self, SceneImage, REC2020_TO_XYZ};

/// Whether `path` is a TIFF or JPEG (by extension).
pub fn is_image_file(path: &Path) -> bool {
    kind(path).is_some()
}

/// "TIFF" or "JPEG" for the file's extension.
fn kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "tif" | "tiff" => Some("TIFF"),
        "jpg" | "jpeg" => Some("JPEG"),
        _ => None,
    }
}

/// The file's orientation (1-8; 1 when it has none).
fn orientation(path: &Path) -> u16 {
    let Ok(file) = std::fs::File::open(path) else { return 1 };
    let mut reader = std::io::BufReader::new(file);
    exif::Reader::new()
        .read_from_container(&mut reader)
        .ok()
        .and_then(|e| e.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)))
        .map(|o| o as u16)
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1)
}

/// sRGB's transfer curve, decoded: 0..1 encoded -> 0..1 linear.
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear sRGB -> linear Rec.2020 (both D65, so no adaptation).
fn srgb_to_rec2020() -> [[f32; 3]; 3] {
    pipeline::mul3(&crate::whitebalance::invert3(&REC2020_TO_XYZ), &crate::develop::SRGB_TO_XYZ)
}

/// The scene for an image of `width` x `height` whose pixels are `rgb`
/// (interleaved, `max` = full scale), not yet upright.
fn scene_image(width: usize, height: usize, rgb: impl Fn(usize) -> [f32; 3] + Sync, max: f32) -> SceneImage {
    let m = srgb_to_rec2020();
    let px: Vec<[f32; 3]> = (0..width * height)
        .into_par_iter()
        .map(|i| {
            let c = rgb(i).map(|v| srgb_to_linear(v / max));
            std::array::from_fn(|r| (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]).max(0.0))
        })
        .collect();
    SceneImage { width, height, px, source_long: width.max(height) }
}

/// Opens a TIFF or JPEG as a scene, at most `max_dim` on the long edge, upright.
pub fn develop_scene(path: &Path, max_dim: usize) -> Result<crate::develop::Scene, String> {
    let started = Instant::now();
    let name = kind(path).ok_or("not a TIFF or JPEG")?;
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    // Photos of the user's own: a 100 MP 16-bit frame is far over the default memory guard.
    reader.no_limits();
    let img = reader.decode().map_err(|e| e.to_string())?;
    let decode_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let sixteen = matches!(
        img,
        image::DynamicImage::ImageRgb16(_) | image::DynamicImage::ImageRgba16(_) | image::DynamicImage::ImageLuma16(_) | image::DynamicImage::ImageLumaA16(_)
    );
    let (w, h) = (img.width() as usize, img.height() as usize);
    let upright = if sixteen {
        let rgb = img.to_rgb16();
        let data = rgb.as_raw();
        scene_image(w, h, |i| [data[i * 3] as f32, data[i * 3 + 1] as f32, data[i * 3 + 2] as f32], 65535.0)
    } else {
        let rgb = img.to_rgb8();
        let data = rgb.as_raw();
        scene_image(w, h, |i| [data[i * 3] as f32, data[i * 3 + 1] as f32, data[i * 3 + 2] as f32], 255.0)
    };
    let long = w.max(h);
    let mut image = upright.downscaled(max_dim).oriented(orientation(path));
    image.source_long = long;
    Ok(crate::develop::Scene {
        image,
        camera: name.to_string(),
        make: String::new(),
        wb: None,
        exposure_offset: 0.0,
        display_referred: true,
        lens: None,
        decode_ms,
        develop_ms: started.elapsed().as_millis(),
        subject_mask: std::sync::OnceLock::new(),
        subject_refined: std::sync::OnceLock::new(),
        denoised: std::sync::OnceLock::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// An asymmetric pattern, so every turn and mirror is distinguishable
    /// (RapidRetouch's test uses the same idea).
    fn pattern() -> RgbImage {
        let mut img = RgbImage::from_pixel(60, 40, Rgb([30, 30, 30]));
        for y in 0..10 {
            for x in 0..20 {
                img.put_pixel(x, y, Rgb([255, 0, 0]));
            }
        }
        for y in 30..40 {
            for x in 50..60 {
                img.put_pixel(x, y, Rgb([0, 0, 255]));
            }
        }
        img
    }

    /// The engine's scene turned back into 8-bit sRGB pixels (via the "As is" render).
    fn render_as_is(scene: &crate::develop::Scene) -> RgbImage {
        let settings = pipeline::RenderSettings { profile: pipeline::Profile::AsIs, ..Default::default() };
        pipeline::render(&scene.image, &settings, 0.0, None)
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("rapidone-imagefile-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A TIFF with an orientation tag, written with the tiff encoder.
    fn write_tiff(path: &Path, img: &RgbImage, orientation: u16) {
        use tiff::encoder::{colortype::RGB8, TiffEncoder};
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut enc = TiffEncoder::new(&mut out).unwrap();
        let mut dir = enc.new_image::<RGB8>(img.width(), img.height()).unwrap();
        dir.encoder().write_tag(tiff::tags::Tag::Orientation, orientation).unwrap();
        dir.write_data(img.as_raw()).unwrap();
    }

    #[test]
    fn a_tiff_of_every_orientation_opens_upright() {
        let src = pattern();
        for o in 1..=8u16 {
            let path = temp(&format!("o{o}.tif"));
            write_tiff(&path, &src, o);
            let want = crate::preview::apply_orientation(image::DynamicImage::ImageRgb8(src.clone()), o).to_rgb8();
            let scene = develop_scene(&path, 10_000).unwrap();
            let got = render_as_is(&scene);
            assert_eq!((got.width(), got.height()), (want.width(), want.height()), "orientation {o}: size");
            let worst = got.as_raw().iter().zip(want.as_raw()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(worst <= 1, "orientation {o}: worst difference {worst}");
        }
    }

    #[test]
    fn an_untouched_file_renders_as_it_is() {
        let mut src = RgbImage::new(256, 4);
        for x in 0..256u32 {
            for y in 0..4 {
                // Greys, then saturated primaries and a skin-like colour, over the whole range.
                let px = match y {
                    0 => Rgb([x as u8, x as u8, x as u8]),
                    1 => Rgb([x as u8, 0, 255 - x as u8]),
                    2 => Rgb([0, x as u8, 128]),
                    _ => Rgb([(200 + x / 8) as u8, 150, (90 + x / 4) as u8]),
                };
                src.put_pixel(x, y, px);
            }
        }
        let path = temp("plain.tif");
        write_tiff(&path, &src, 1);
        let scene = develop_scene(&path, 10_000).unwrap();
        assert!(scene.display_referred && scene.wb.is_none());
        let got = render_as_is(&scene);
        let worst = got.as_raw().iter().zip(src.as_raw()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "an untouched file changed by up to {worst} of 255");
    }

    #[test]
    fn a_jpeg_opens() {
        let path = temp("plain.jpg");
        pattern().save(&path).unwrap();
        let scene = develop_scene(&path, 10_000).unwrap();
        assert_eq!((scene.image.width, scene.image.height), (60, 40));
        assert_eq!(scene.camera, "JPEG");
        assert!(is_image_file(&path) && !is_image_file(Path::new("a.RAF")));
    }

    #[test]
    fn temperature_and_tint_warm_cool_and_shift_green() {
        let neutral = pipeline::simple_rebalance(None, None);
        assert_eq!(neutral, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let warm = pipeline::simple_rebalance(Some(8000.0), None);
        assert!(warm[0][0] > 1.0 && warm[2][2] < 1.0);
        let cool = pipeline::simple_rebalance(Some(3500.0), None);
        assert!(cool[0][0] < 1.0 && cool[2][2] > 1.0);
        let magenta = pipeline::simple_rebalance(None, Some(50.0));
        assert!(magenta[1][1] < 1.0);
    }
}
