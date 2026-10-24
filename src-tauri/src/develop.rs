//! The RAW engine, step 1: a basic but real development from sensor data.
//!
//!   1. Decode (rawler): sensor values + the camera's calibration facts.
//!   2. Black/white levels: rawler's `apply_scaling` maps every value to 0..1
//!      (0 = no light, 1 = sensor full), per colour position on the sensor.
//!   3. "Superpixel": each small block of the colour-filter pattern - 2x2 for
//!      Bayer, 3x3 for Fuji X-Trans - always holds all three colours, so the
//!      block's averages become one full-colour pixel. Half (or a third of)
//!      the size and no guessing; proper demosaicing is step 2.
//!   4. White balance: the camera's as-shot multipliers.
//!   5. Camera colour -> sRGB, with the camera's colour matrix.
//!   6. sRGB gamma, for the screen.
//!
//! Everything up to step 6 is linear light: double the number, double the light.

use crate::preview;
use image::{DynamicImage, RgbImage};
use rawler::imgop::Rect;
use rawler::decoders::RawDecodeParams;
use rawler::rawimage::{RawImage, RawImageData, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use std::path::Path;
use std::time::Instant;

pub struct Developed {
    pub image: DynamicImage,
    pub camera: String,
    pub decode_ms: u128,
    pub develop_ms: u128,
}

/// sRGB (linear) -> XYZ, D65 white.
pub(crate) const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192_0, 0.950_304_1],
];

fn mul3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn invert3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ])
}

/// Camera RGB (after white balance) -> linear sRGB.
///
/// The camera's matrix maps XYZ to camera RGB. Chaining it with sRGB->XYZ
/// gives sRGB->camera; each row is then scaled to sum to 1 so that a white
/// sRGB colour lands on camera (1,1,1), which is what white balance has
/// already made neutral greys. Inverting that gives the conversion we need,
/// and guarantees greys stay grey. (The classic dcraw approach.)
fn camera_to_srgb(raw: &RawImage) -> [[f32; 3]; 3] {
    camera_to(raw, &SRGB_TO_XYZ)
}

/// Camera RGB (after white balance) -> linear Rec.2020, the scene-referred working space.
pub fn camera_to_rec2020(raw: &RawImage) -> [[f32; 3]; 3] {
    camera_to(raw, &crate::pipeline::REC2020_TO_XYZ)
}

/// Camera RGB (after white balance) -> any linear RGB space given by its RGB->XYZ matrix.
fn camera_to(raw: &RawImage, space_to_xyz: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    use rawler::imgop::xyz::Illuminant;
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // D65 is daylight and what sRGB is defined against; other matrices are a
    // reasonable stand-in until the colour step is done properly (DNG-spec
    // interpolation between two illuminants, which DCP profiles need anyway).
    let Some((_, m)) = raw.color_matrix_find_first([
        Illuminant::D65,
        Illuminant::D55,
        Illuminant::D50,
        Illuminant::Daylight,
        Illuminant::A,
    ]) else {
        return identity;
    };
    if m.len() < 9 {
        return identity;
    }
    let xyz_to_cam = [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]];
    let mut space_to_cam = mul3(&xyz_to_cam, space_to_xyz);
    for row in space_to_cam.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-6 {
            row.iter_mut().for_each(|v| *v /= sum);
        }
    }
    invert3(&space_to_cam).unwrap_or(identity)
}

fn srgb_gamma(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Decode and develop at superpixel resolution (half size for Bayer, a third
/// for X-Trans), returned upright as 8-bit sRGB.
pub fn develop_superpixel(path: &Path) -> Result<Developed, String> {
    let started = Instant::now();
    // Orientation comes from the EXIF metadata: the decoded RawImage doesn't
    // reliably carry it for every make (portraits came out sideways).
    let source = RawSource::new(path).map_err(|e| e.to_string())?;
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();
    let mut raw = decoder.raw_image(&source, &params, false).map_err(|e| e.to_string())?;
    let orientation = decoder
        .raw_metadata(&source, &params)
        .ok()
        .and_then(|m| m.exif.orientation)
        .unwrap_or(1);
    let decode_ms = started.elapsed().as_millis();
    let started = Instant::now();

    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) => config.cfa.clone(),
        _ => return Err("not a colour-filter-array RAW (e.g. a linear DNG) - not supported yet".into()),
    };
    if raw.cpp != 1 || !cfa.is_rgb() {
        return Err("unsupported sensor layout (four-colour or multi-channel)".into());
    }
    if raw.fuji_rotation_width.is_some() {
        return Err("rotated Fuji SuperCCD sensors aren't supported yet".into());
    }
    // A 2-wide pattern is Bayer; Fuji X-Trans repeats every 6 and has all
    // three colours in each 3x3 block.
    let block = if cfa.width == 2 && cfa.height == 2 { 2 } else { 3 };

    raw.apply_scaling().map_err(|e| e.to_string())?;
    let data = match &raw.data {
        RawImageData::Float(d) => d,
        RawImageData::Integer(_) => return Err("scaling didn't produce float data".into()),
    };

    // The area of real image pixels: crop, else active area, else everything.
    // Its start is rounded down to the block grid so every block holds the
    // whole colour pattern.
    let area: Rect = raw
        .crop_area
        .or(raw.active_area)
        .unwrap_or_else(|| Rect::new(rawler::imgop::Point::new(0, 0), raw.dim()));
    let x0 = area.p.x / block * block;
    let y0 = area.p.y / block * block;
    let out_w = (area.p.x + area.d.w - x0) / block;
    let out_h = (area.p.y + area.d.h - y0) / block;
    if out_w == 0 || out_h == 0 {
        return Err("empty image area".into());
    }

    let wb = if raw.wb_coeffs[0].is_nan() || raw.wb_coeffs[1] <= 0.0 {
        [1.0, 1.0, 1.0]
    } else {
        let g = raw.wb_coeffs[1];
        [raw.wb_coeffs[0] / g, 1.0, raw.wb_coeffs[2] / g]
    };
    let to_srgb = camera_to_srgb(&raw);
    let width = raw.width;

    let mut out = vec![0u8; out_w * out_h * 3];
    let rows_per_task = (out_h / (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) * 4)).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in out.chunks_mut(rows_per_task * out_w * 3).enumerate() {
            let cfa = &cfa;
            scope.spawn(move || {
                let first_row = chunk_index * rows_per_task;
                for (r, row) in chunk.chunks_mut(out_w * 3).enumerate() {
                    let by = y0 + (first_row + r) * block;
                    for bx_i in 0..out_w {
                        let bx = x0 + bx_i * block;
                        // Average each colour within the block.
                        let mut sum = [0.0f32; 3];
                        let mut count = [0u32; 3];
                        for dy in 0..block {
                            for dx in 0..block {
                                let c = cfa.color_at(by + dy, bx + dx).min(2);
                                sum[c] += data[(by + dy) * width + bx + dx];
                                count[c] += 1;
                            }
                        }
                        // White balance, then clip at the sensor's ceiling so
                        // blown highlights come out white, not pink.
                        let cam: [f32; 3] =
                            std::array::from_fn(|c| (sum[c] / count[c].max(1) as f32 * wb[c]).min(1.0));
                        for c in 0..3 {
                            let lin = to_srgb[c][0] * cam[0] + to_srgb[c][1] * cam[1] + to_srgb[c][2] * cam[2];
                            row[bx_i * 3 + c] = (srgb_gamma(lin) * 255.0 + 0.5) as u8;
                        }
                    }
                }
            });
        }
    });

    let img = RgbImage::from_raw(out_w as u32, out_h as u32, out).ok_or("image buffer size mismatch")?;
    let upright = preview::apply_orientation(DynamicImage::ImageRgb8(img), orientation);
    Ok(Developed {
        image: upright,
        camera: format!("{} {}", raw.clean_make, raw.clean_model),
        decode_ms,
        develop_ms: started.elapsed().as_millis(),
    })
}

/// Which demosaic to use for a full-resolution development.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    Bilinear,
    Rcd,
    /// RCD followed by false-colour suppression (for fine patterns).
    RcdClean,
}

/// A sensor pattern viewed from an offset, for developing a window of the sensor.
struct Offset<'a> {
    cfa: &'a rawler::CFA,
    dy: usize,
    dx: usize,
}

impl crate::demosaic::CfaPattern for Offset<'_> {
    fn color_at(&self, row: usize, col: usize) -> usize {
        self.cfa.color_at(row + self.dy, col + self.dx).min(2)
    }
}

/// Develops a window of the sensor at full resolution (100% pixels), for
/// judging demosaic quality. `centre` is a fraction of the image area
/// (0.5, 0.5 = middle); `size` is the window's width and height in pixels.
/// Not oriented - it's the sensor's own way up.
pub fn develop_window(path: &Path, method: Method, centre: (f32, f32), size: usize) -> Result<RgbImage, String> {
    let mut raw = rawler::decode_file(path).map_err(|e| e.to_string())?;
    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) => config.cfa.clone(),
        _ => return Err("not a colour-filter-array RAW".into()),
    };
    let bayer = cfa.width == 2 && cfa.height == 2;
    raw.apply_scaling().map_err(|e| e.to_string())?;
    let RawImageData::Float(data) = &raw.data else {
        return Err("scaling didn't produce float data".into());
    };
    let area: Rect = raw
        .crop_area
        .or(raw.active_area)
        .unwrap_or_else(|| Rect::new(rawler::imgop::Point::new(0, 0), raw.dim()));

    // Window plus a margin so the demosaic has neighbours at its edges.
    let margin = 8;
    let full = size + 2 * margin;
    let cx = area.p.x + (centre.0 * area.d.w as f32) as usize;
    let cy = area.p.y + (centre.1 * area.d.h as f32) as usize;
    // Even offsets keep a Bayer pattern's phase; multiples of 6 keep X-Trans's.
    let step = if bayer { 2 } else { 6 };
    let x0 = (cx.saturating_sub(full / 2) / step * step).min(raw.width - full);
    let y0 = (cy.saturating_sub(full / 2) / step * step).min(raw.height - full);

    let wb = if raw.wb_coeffs[0].is_nan() || raw.wb_coeffs[1] <= 0.0 {
        [1.0, 1.0, 1.0]
    } else {
        let g = raw.wb_coeffs[1];
        [raw.wb_coeffs[0] / g, 1.0, raw.wb_coeffs[2] / g]
    };
    let pat = Offset { cfa: &cfa, dy: y0, dx: x0 };
    use crate::demosaic::CfaPattern;
    // White balance goes on before demosaicing, so the demosaic compares
    // like with like across colours.
    let window: Vec<f32> = (0..full * full)
        .map(|i| {
            let (y, x) = (i / full, i % full);
            data[(y0 + y) * raw.width + x0 + x] * wb[pat.color_at(y, x)]
        })
        .collect();

    let planes = if !bayer {
        crate::demosaic::xtrans_average(&window, full, full, &pat)
    } else {
        match method {
            Method::Bilinear => crate::demosaic::bilinear(&window, full, full, &pat),
            Method::Rcd => crate::demosaic::rcd(&window, full, full, &pat),
            Method::RcdClean => {
                let mut p = crate::demosaic::rcd(&window, full, full, &pat);
                crate::demosaic::suppress_false_colour(&mut p, 1);
                p
            }
        }
    };

    let to_srgb = camera_to_srgb(&raw);
    let mut out = RgbImage::new(size as u32, size as u32);
    for y in 0..size {
        for x in 0..size {
            let i = (y + margin) * full + x + margin;
            let cam = [planes.r[i].min(1.0), planes.g[i].min(1.0), planes.b[i].min(1.0)];
            let px: [u8; 3] = std::array::from_fn(|c| {
                let lin = to_srgb[c][0] * cam[0] + to_srgb[c][1] * cam[1] + to_srgb[c][2] * cam[2];
                (srgb_gamma(lin) * 255.0 + 0.5) as u8
            });
            out.put_pixel(x as u32, y as u32, image::Rgb(px));
        }
    }
    Ok(out)
}

/// A RAW developed into the scene-referred working space, ready to cache and
/// re-render as sliders move.
pub struct Scene {
    pub image: crate::pipeline::SceneImage,
    pub camera: String,
    /// Clean make, for the per-make baseline exposure.
    pub make: String,
    /// White balance facts (as-shot Kelvin/Tint, camera matrices); None if
    /// the camera has no colour matrix, in which case WB can't be changed.
    pub wb: Option<crate::whitebalance::WhiteBalance>,
    /// Extra starting brightness in stops this particular photo needs, from its
    /// own metadata (Fujifilm: the exposure bias the camera recorded, relative
    /// to its usual -0.72); 0 when nothing is known.
    pub exposure_offset: f32,
    /// A TIFF or JPEG: already toned, so it renders with the "As is" profile
    /// (`Profile::AsIs`) and has no camera white balance.
    pub display_referred: bool,
    /// The lens and shot from EXIF, for lens profile corrections.
    pub lens: Option<crate::lensprofile::LensShot>,
    pub decode_ms: u128,
    pub develop_ms: u128,
    /// The subject mask (`mask.rs`), found the first time it's needed.
    pub subject_mask: std::sync::OnceLock<Option<std::sync::Arc<crate::mask::Mask>>>,
    /// The subject mask refined against this photo (`gradient::refine_subject`), for subject masks.
    pub subject_refined: std::sync::OnceLock<Option<std::sync::Arc<crate::gradient::Grid>>>,
    /// The scene with its noise taken out (AI noise reduction, `denoise`),
    /// made the first time the Noise slider is up, then kept with the scene.
    pub denoised: std::sync::OnceLock<Option<std::sync::Arc<crate::pipeline::SceneImage>>>,
}

impl Scene {
    /// The subject mask refined against this photo, found and refined once (the subject
    /// mask as already found, e.g. from the folder's cache, or found now).
    pub fn subject_refined(&self) -> Option<std::sync::Arc<crate::gradient::Grid>> {
        self.subject_refined
            .get_or_init(|| {
                let mask = self.subject_mask.get_or_init(|| crate::mask::subject(&self.image).map(std::sync::Arc::new));
                mask.as_ref().map(|m| std::sync::Arc::new(crate::gradient::refine_subject(m, &self.image)))
            })
            .clone()
    }
}

/// Decodes and develops a RAW (superpixel for now) into linear Rec.2020
/// scene light, at most `max_dim` on the long edge, upright.
pub fn develop_scene(path: &Path, max_dim: usize) -> Result<Scene, String> {
    develop_scene_with(path, max_dim, false)
}

/// As `develop_scene`, always through the full-resolution demosaic (Bayer
/// sensors), however small the result: for exports and comparisons, where
/// the half-size superpixel image, shrunk further, comes out visibly softer
/// than Lightroom's (lashes, hair and skin texture at 2000 px).
pub fn develop_scene_export(path: &Path, max_dim: usize) -> Result<Scene, String> {
    develop_scene_with(path, max_dim, true)
}

fn develop_scene_with(path: &Path, max_dim: usize, always_full: bool) -> Result<Scene, String> {
    // A TIFF or JPEG is opened as it is (imagefile.rs), not developed from sensor data.
    if crate::imagefile::is_image_file(path) {
        return crate::imagefile::develop_scene(path, max_dim);
    }
    let started = Instant::now();
    let p_decode = crate::prof::span("decode: read + unpack RAW");
    let source = RawSource::new(path).map_err(|e| e.to_string())?;
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();
    let mut raw = decoder.raw_image(&source, &params, false).map_err(|e| e.to_string())?;
    let metadata = decoder.raw_metadata(&source, &params).ok();
    let orientation = metadata.as_ref().and_then(|m| m.exif.orientation).unwrap_or(1);
    let lens_shot = metadata.as_ref().and_then(|m| {
        let ratio = |r: Option<rawler::formats::tiff::Rational>| r.filter(|r| r.d > 0).map(|r| r.n as f32 / r.d as f32);
        Some(crate::lensprofile::LensShot {
            make: m.make.clone(),
            lens: m.exif.lens_model.clone()?,
            focal: ratio(m.exif.focal_length)?,
            fnumber: ratio(m.exif.fnumber).unwrap_or(0.0),
        })
    });
    let decode_ms = started.elapsed().as_millis();
    drop(p_decode);
    let started = Instant::now();
    let p_demosaic = crate::prof::span("decode: demosaic");

    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) => config.cfa.clone(),
        _ => return Err("not a colour-filter-array RAW (e.g. a linear DNG) - not supported yet".into()),
    };
    if raw.cpp != 1 || !cfa.is_rgb() || raw.fuji_rotation_width.is_some() {
        return Err("unsupported sensor layout".into());
    }
    let block = if cfa.width == 2 && cfa.height == 2 { 2 } else { 3 };
    raw.apply_scaling().map_err(|e| e.to_string())?;
    let RawImageData::Float(data) = &raw.data else {
        return Err("scaling didn't produce float data".into());
    };
    let area: Rect = raw
        .crop_area
        .or(raw.active_area)
        .unwrap_or_else(|| Rect::new(rawler::imgop::Point::new(0, 0), raw.dim()));
    let x0 = area.p.x / block * block;
    let y0 = area.p.y / block * block;
    let mut out_w = (area.p.x + area.d.w - x0) / block;
    let mut out_h = (area.p.y + area.d.h - y0) / block;

    let wb = if raw.wb_coeffs[0].is_nan() || raw.wb_coeffs[1] <= 0.0 {
        [1.0, 1.0, 1.0]
    } else {
        let g = raw.wb_coeffs[1];
        [raw.wb_coeffs[0] / g, 1.0, raw.wb_coeffs[2] / g]
    };
    let width = raw.width;

    // The sensor's long edge (before any full-resolution demosaic below
    // changes out_w / out_h): Texture and Clarity scale their radii by it.
    let sensor_long = out_w.max(out_h) * block;

    // Bigger than the superpixel image asked for (an export): full
    // resolution, through the RCD demosaic for Bayer sensors and rawler's
    // Markesteijn (3-pass) for X-Trans.
    use rayon::prelude::*;
    let full_res = always_full || max_dim > out_w.max(out_h);
    let cam: Vec<[f32; 3]> = if full_res && block == 3 {
        let (fw, fh) = (out_w * 3, out_h * 3);
        // White balance first, as for Bayer.
        let window: Vec<f32> = (0..fw * fh).into_par_iter().map(|i| {
            let (y, x) = (i / fw, i % fw);
            data[(y0 + y) * width + x0 + x] * wb[cfa.color_at(y0 + y, x0 + x).min(2)]
        }).collect();
        use rawler::imgop::sensor::Demosaic;
        let pix = rawler::pixarray::PixF32::new_with(window, fw, fh);
        let roi = Rect::new(rawler::imgop::Point::new(0, 0), rawler::imgop::Dim2::new(fw, fh));
        let rgb = rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic::new_pass_3()
            .demosaic(&pix, &cfa.shift(x0, y0), &rawler::cfa::PlaneColor::new("RGB"), roi);
        (out_w, out_h) = (fw, fh);
        rgb.data
    } else if full_res {
        let (fw, fh) = (out_w * 2, out_h * 2);
        let pat = Offset { cfa: &cfa, dy: y0, dx: x0 };
        use crate::demosaic::CfaPattern;
        // White balance first, so the demosaic compares like with like.
        let window: Vec<f32> = (0..fw * fh).into_par_iter().map(|i| {
            let (y, x) = (i / fw, i % fw);
            data[(y0 + y) * width + x0 + x] * wb[pat.color_at(y, x)]
        }).collect();
        let mut planes = {
            let _p = crate::prof::span("decode:   RCD");
            crate::demosaic::rcd(&window, fw, fh, &pat)
        };
        {
            let _p = crate::prof::span("decode:   false colour");
            crate::demosaic::suppress_false_colour(&mut planes, 1);
        }
        (out_w, out_h) = (fw, fh);
        (0..fw * fh).into_par_iter().map(|i| [planes.r[i], planes.g[i], planes.b[i]]).collect()
    } else {
    // Superpixel camera RGB, white balanced, *not* clipped - headroom kept.
    (0..out_w * out_h)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = (x0 + (i % out_w) * block, y0 + (i / out_w) * block);
            let mut sum = [0.0f32; 3];
            let mut count = [0u32; 3];
            for dy in 0..block {
                for dx in 0..block {
                    let c = cfa.color_at(by + dy, bx + dx).min(2);
                    sum[c] += data[(by + dy) * width + bx + dx];
                    count[c] += 1;
                }
            }
            std::array::from_fn(|c| sum[c] / count[c].max(1) as f32 * wb[c])
        })
        .collect()
    };

    drop(p_demosaic);
    let _p_scene = crate::prof::span("decode: to scene colour");
    // A raw channel saturates at 1.0, i.e. at its white-balance multiplier.
    let clip = wb;
    let white_balance = white_balance_of(&raw, wb);
    // The DNG-style chain at the as-shot white balance when the camera has
    // matrices; otherwise the simple D65-matrix conversion.
    let to_rec2020 = white_balance
        .map(|w| w.camera_to_rec2020(w.shot_kelvin, w.shot_tint))
        .unwrap_or_else(|| camera_to_rec2020(&raw));
    // Fujifilm files: their own exposure bias and built-in lens corrections.
    let fuji_bytes = if raw.clean_make.to_ascii_lowercase().starts_with("fuji") { std::fs::read(path).ok() } else { None };
    let mut scene = crate::pipeline::scene_from_camera(out_w, out_h, &cam, clip, &to_rec2020)
        .downscaled(max_dim)
        .oriented(orientation);
    // Texture and Clarity scale their radii by how much smaller this scene is than the sensor image.
    scene.source_long = sensor_long;
    // Built-in lens corrections (Fujifilm's distortion, fringing and
    // vignetting tables). Lightroom applies them only with Enable Profile
    // Corrections on: with it off (our kits), applying any of them moved 46
    // Fuji photos from 2.58 to 3.7-5.2 dE from its renders. Off until the
    // edit asks for profile corrections (to come with preset import);
    // RAPIDONE_LENS=1 turns them on for testing.
    let lens = fuji_bytes.as_deref().and_then(crate::lens::fujifilm).filter(|_| std::env::var("RAPIDONE_LENS").is_ok_and(|v| v == "1"));
    let scene = match &lens {
        Some(l) => l.apply(&scene),
        None => scene,
    };
    Ok(Scene {
        image: scene,
        camera: format!("{} {}", raw.clean_make, raw.clean_model),
        make: raw.clean_make.clone(),
        wb: white_balance,
        exposure_offset: fuji_bytes.as_deref().map_or(0.0, exposure_offset),
        display_referred: false,
        lens: lens_shot,
        decode_ms,
        develop_ms: started.elapsed().as_millis(),
        subject_mask: std::sync::OnceLock::new(),
        subject_refined: std::sync::OnceLock::new(),
        denoised: std::sync::OnceLock::new(),
    })
}

/// Decodes just enough to inspect a RAW's metadata (developer tools).
pub fn decode_for_info(path: &Path) -> Result<RawImage, String> {
    rawler::decode_file(path).map_err(|e| e.to_string())
}

/// The camera's white balance facts: its two colour matrices (tungsten "A"
/// and daylight "D65"; if only one exists it's used for both) and the
/// as-shot neutral. None if the file has no usable matrix.
pub fn white_balance_of(raw: &RawImage, wb: [f32; 3]) -> Option<crate::whitebalance::WhiteBalance> {
    use rawler::imgop::xyz::Illuminant;
    let as3 = |m: &Vec<f32>| -> Option<[[f32; 3]; 3]> {
        (m.len() >= 9).then(|| [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]])
    };
    let a = raw.color_matrix.get(&Illuminant::A).and_then(as3);
    let d65 = raw
        .color_matrix_find_first([Illuminant::D65, Illuminant::D55, Illuminant::D50, Illuminant::Daylight])
        .and_then(|(_, m)| as3(&m));
    let (a, d65) = match (a, d65) {
        (Some(a), Some(d)) => (a, d),
        (Some(m), None) | (None, Some(m)) => (m, m),
        (None, None) => return None,
    };
    // Adobe's per-model CameraCalibration scales the matrices' camera rows,
    // which is what puts our Kelvin/Tint on Lightroom's scale.
    let cc = crate::adobe::camera_calibration(&format!("{} {}", raw.clean_make, raw.clean_model)).unwrap_or([1.0; 3]);
    let cal = |m: [[f32; 3]; 3]| [0, 1, 2].map(|i| m[i].map(|v| v * cc[i]));
    let neutral = [1.0 / wb[0], 1.0, 1.0 / wb[2]];
    let mut balance = crate::whitebalance::WhiteBalance::new(crate::whitebalance::CameraColour { a: cal(a), d65: cal(d65) }, neutral);
    balance.calibration = cc;
    Some(balance)
}

/// A photo's own starting-brightness offset from its metadata. Fujifilm RAFs
/// record the exposure bias the camera applied (usually -0.72 stops; DR modes
/// and some bodies differ), which Lightroom compensates for: measured on 25
/// Fuji samples, the brightness Lightroom gives matches -(bias + 0.72) within
/// ~0.1 stop (GFX 100 II -1.72 -> +1.0, X-T30 +0.28 -> -1.0).
fn exposure_offset(raf: &[u8]) -> f32 {
    crate::fuji::exposure_info(raf).and_then(|e| e.raw_exposure_bias).map(|b| -(b + 0.72)).unwrap_or(0.0)
}
