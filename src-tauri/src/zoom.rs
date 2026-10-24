//! Zoomed-in detail for Edit: the part of the photo on screen, rendered from
//! the full-resolution photo with the current settings, matching the whole
//! photo's render (and so the export).
//!
//! Rendering the whole photo at full size for each slider move is far too
//! slow (24 MP: 1.4-4 s, 100 MP: 6-26 s), so only the area in view is
//! rendered, at the view's resolution. What depends on the whole photo - the
//! tone map, its brightness statistics, Clarity's and Dehaze's measures,
//! where the vignette, gradients and removals sit - comes from the whole
//! photo's preview render with the same settings (`pipeline::Measured`,
//! `pipeline::Frame`), so the part comes out as it would in the whole.
//!
//! Geometry: the view's rectangle is on the photo as shown (quarter turns,
//! keystone, straighten and crop applied); each of its pixels is traced back
//! to the full-resolution photo, the area those land in (plus a margin for
//! the tools that look at neighbours) is rendered, and the view is sampled
//! from it, as `crop::apply` samples the whole picture.

use crate::develop::Scene;
use crate::pipeline::{Frame, Measured, Region, RenderSettings, SceneImage};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The photo at full resolution, for the photo being zoomed into (one).
#[derive(Default)]
pub struct FullScene {
    entry: Mutex<Option<(String, Arc<Scene>)>>,
}

impl FullScene {
    pub fn get_or_develop(&self, path: &str) -> Result<Arc<Scene>, String> {
        if let Some((p, s)) = self.entry.lock().unwrap().as_ref() {
            if p == path {
                return Ok(s.clone());
            }
        }
        // Drop the last one first: at 100 MP it is over a gigabyte.
        *self.entry.lock().unwrap() = None;
        let scene = Arc::new(crate::develop::develop_scene_export(Path::new(path), 1 << 20)?);
        *self.entry.lock().unwrap() = Some((path.to_string(), scene.clone()));
        Ok(scene)
    }
}

/// What the latest whole-photo renders measured, by photo and settings.
#[derive(Default)]
pub struct MeasuredCache {
    entries: Mutex<Vec<(String, Arc<Measured>)>>,
}

const MEASURED_KEPT: usize = 8;

/// The key a render's settings are kept under (the region fields are never set here).
pub fn settings_key(path: &str, settings: &RenderSettings) -> String {
    format!("{path}\u{1}{settings:?}")
}

impl MeasuredCache {
    pub fn put(&self, key: String, m: Measured) {
        let mut e = self.entries.lock().unwrap();
        e.retain(|(k, _)| *k != key);
        e.push((key, Arc::new(m)));
        if e.len() > MEASURED_KEPT {
            e.remove(0);
        }
    }

    pub fn get(&self, key: &str) -> Option<Arc<Measured>> {
        self.entries.lock().unwrap().iter().find(|(k, _)| k == key).map(|(_, m)| m.clone())
    }

    /// The newest measures for a photo, whatever its settings were (while a
    /// slider moves: close enough, and no whole-photo render to wait for).
    pub fn latest_for(&self, path: &str) -> Option<Arc<Measured>> {
        let prefix = format!("{path}\u{1}");
        self.entries.lock().unwrap().iter().rev().find(|(k, _)| k.starts_with(&prefix)).map(|(_, m)| m.clone())
    }
}

/// The shown photo's size (after quarter turns and crop) for a photo w x h,
/// as `crop::apply` makes it.
fn shown_size(crop: &crate::crop::Crop, w: usize, h: usize) -> (f32, f32) {
    let (wq, hq) = if crop.quarter % 2 == 1 { (h as f32, w as f32) } else { (w as f32, h as f32) };
    if crop.is_cut_free() {
        return (wq, hq);
    }
    let (x0, x1) = (crop.x0.clamp(0.0, 1.0), crop.x1.clamp(0.0, 1.0));
    let (y0, y1) = (crop.y0.clamp(0.0, 1.0), crop.y1.clamp(0.0, 1.0));
    (((x1 - x0) * wq).round().max(1.0), ((y1 - y0) * hq).round().max(1.0))
}

/// Where a point of the shown photo (pixels) comes from in the photo (w x h,
/// upright as developed): the crop's straighten and cut undone, then the
/// keystone, then the quarter turns, in `crop::apply`'s order.
fn source_of(settings: &RenderSettings, w: usize, h: usize) -> impl Fn(f32, f32) -> Option<(f32, f32)> {
    let crop = settings.crop;
    let q = crop.quarter % 4;
    let (wq, hq) = if q % 2 == 1 { (h as f32, w as f32) } else { (w as f32, h as f32) };
    let keystone = settings.keystone.source_point(wq, hq);
    let cut_free = crop.is_cut_free();
    let (x0, y0) = (crop.x0.clamp(0.0, 1.0), crop.y0.clamp(0.0, 1.0));
    let (s, c) = (-crop.angle).to_radians().sin_cos();
    let (wf, hf) = (w as f32, h as f32);
    move |ox, oy| {
        // On the turned (and keystoned) canvas.
        let (kx, ky) = if cut_free {
            (ox, oy)
        } else {
            let x = x0 * wq + ox - wq / 2.0;
            let y = y0 * hq + oy - hq / 2.0;
            (x * c - y * s + wq / 2.0, x * s + y * c + hq / 2.0)
        };
        let (tx, ty) = keystone(kx, ky)?;
        // Undo the quarter turns (image rotate90 / 180 / 270, clockwise).
        Some(match q {
            1 => (ty, hf - tx),
            2 => (wf - tx, hf - ty),
            3 => (wf - ty, tx),
            _ => (tx, ty),
        })
    }
}

/// The finished part of the photo for a rectangle of it as shown (`rect`:
/// left, top, right, bottom as fractions of the shown photo), `out_w` x
/// `out_h` pixels (at most the photo's own pixels).
pub fn render_part(
    full: &Scene,
    preview: &Scene,
    measured: Arc<Measured>,
    settings: &RenderSettings,
    rect: [f32; 4],
    out_w: u32,
    out_h: u32,
) -> Result<image::RgbImage, String> {
    let (w, h) = (full.image.width, full.image.height);
    let (sw, sh) = shown_size(&settings.crop, w, h);
    let (dx0, dy0) = (rect[0].clamp(0.0, 1.0) * sw, rect[1].clamp(0.0, 1.0) * sh);
    let (dx1, dy1) = (rect[2].clamp(0.0, 1.0) * sw, rect[3].clamp(0.0, 1.0) * sh);
    if dx1 - dx0 < 1.0 || dy1 - dy0 < 1.0 {
        return Err("empty area".into());
    }
    // Never more pixels than the photo has there.
    let out_w = (out_w as f32).min(dx1 - dx0).round().max(1.0) as u32;
    let out_h = (out_h as f32).min(dy1 - dy0).round().max(1.0) as u32;
    let sc = out_w as f32 / (dx1 - dx0);
    // Render at a whole-number fraction of full size when zoomed out (cheaper).
    let k = ((1.0 / sc).floor() as usize).max(1);
    let map = source_of(settings, w, h);

    // The area of the photo the view comes from: its border traced back.
    let (mut sx0, mut sy0, mut sx1, mut sy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..=32 {
        let t = i as f32 / 32.0;
        for (x, y) in [(dx0 + t * (dx1 - dx0), dy0), (dx0 + t * (dx1 - dx0), dy1), (dx0, dy0 + t * (dy1 - dy0)), (dx1, dy0 + t * (dy1 - dy0))] {
            if let Some((px, py)) = map(x, y) {
                sx0 = sx0.min(px);
                sy0 = sy0.min(py);
                sx1 = sx1.max(px);
                sy1 = sy1.max(py);
            }
        }
    }
    if sx0 > sx1 || sy0 > sy1 {
        return Err("outside the photo".into());
    }
    // A margin for the tools that look at neighbours (Texture, sharpening, noise).
    let margin = 64.0 * k as f32;
    let (ax0, ay0) = (((sx0 - margin).floor().max(0.0) as usize) / k * k, ((sy0 - margin).floor().max(0.0) as usize) / k * k);
    let (ax1, ay1) = (((sx1 + margin).ceil() as usize).min(w), ((sy1 + margin).ceil() as usize).min(h));
    if ax1 <= ax0 + k || ay1 <= ay0 + k {
        return Err("outside the photo".into());
    }
    // The area, at 1/k size (block averages).
    let (rw, rh) = ((ax1 - ax0) / k, (ay1 - ay0) / k);
    let mut px = vec![[0.0f32; 3]; rw * rh];
    // Lens corrections on: the part is taken from the full-size photo through the correction
    // (each pixel from where the lens put it, per channel, with its vignetting gain), so it
    // matches the corrected whole photo.
    if let Some(lens) = crate::commands::lens_correction(full, settings) {
        use rayon::prelude::*;
        let img = &full.image;
        let at = |ch: usize, x: f32, y: f32| -> f32 {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let p = |xx: usize, yy: usize| img.px[yy * w + xx][ch];
            let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * fx;
            let bottom = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * fx;
            top + (bottom - top) * fy
        };
        let half = (k as f32 - 1.0) / 2.0;
        px.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
            for (rx, o) in row.iter_mut().enumerate() {
                let (x, y) = ((ax0 + rx * k) as f32 + half, (ay0 + ry * k) as f32 + half);
                let (src, gain) = lens.source_at(x, y, w, h);
                *o = std::array::from_fn(|ch| at(ch, src[ch].0, src[ch].1) * gain);
            }
        });
    } else {
        use rayon::prelude::*;
        let inv = 1.0 / (k * k) as f32;
        px.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
            for (rx, o) in row.iter_mut().enumerate() {
                let mut acc = [0.0f32; 3];
                for yy in 0..k {
                    let line = (ay0 + ry * k + yy) * w;
                    for xx in 0..k {
                        let p = full.image.px[line + ax0 + rx * k + xx];
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                    }
                }
                *o = acc.map(|v| v * inv);
            }
        });
    }
    let mut part_image = SceneImage { width: rw, height: rh, px, source_long: full.image.source_long };
    // Patches (Heal / Clone), whose sources can be anywhere in the photo: read from the full-size
    // photo (through the lens correction, as the part is), at the part's 1/k scale.
    {
        let lens = crate::commands::lens_correction(full, settings);
        let img = &full.image;
        let sample = |fx: f32, fy: f32| -> [f32; 3] {
            let (x, y) = (fx * w as f32 - 0.5, fy * h as f32 - 0.5);
            match &lens {
                Some(l) => {
                    let (src, gain) = l.source_at(x, y, w, h);
                    std::array::from_fn(|ch| crate::commands::bilinear(&img.px, w, h, src[ch].0, src[ch].1)[ch] * gain)
                }
                None => crate::commands::bilinear(&img.px, w, h, x, y),
            }
        };
        let frame = crate::pipeline::Frame { x0: ax0 as f32 / k as f32, y0: ay0 as f32 / k as f32, full_w: w as f32 / k as f32, full_h: h as f32 / k as f32 };
        // With the removals, in the order they were all made (`removal::apply_in_order`).
        if let Some(p) = crate::removal::apply_in_order(&part_image, &settings.removals, Some(&frame), Some(&sample)) {
            part_image = p;
        }
    }
    // In-between sizes (e.g. 80%, 64% while a slider moves): the rest of the
    // way down from 1/k by area averaging. `ps`: part pixels per photo pixel.
    let rest = sc * k as f32;
    if rest < 0.95 {
        part_image = part_image.downscaled(((rw.max(rh) as f32) * rest).round().max(1.0) as usize);
    }
    let ps = part_image.width as f32 / rw as f32 / k as f32;
    let part = Scene {
        image: part_image,
        camera: full.camera.clone(),
        make: full.make.clone(),
        wb: full.wb,
        exposure_offset: full.exposure_offset,
        display_referred: full.display_referred,
        lens: full.lens.clone(),
        decode_ms: 0,
        develop_ms: 0,
        subject_mask: Default::default(),
        subject_refined: Default::default(),
        denoised: Default::default(),
    };
    // The part's settings: placed in the whole photo, with its measures; no
    // geometry (done below); the subject vignette's weights from the whole photo.
    let mut s = settings.clone();
    s.region = Some(Arc::new(Region {
        frame: Frame { x0: ax0 as f32 * ps, y0: ay0 as f32 * ps, full_w: w as f32 * ps, full_h: h as f32 * ps },
        measured,
    }));
    s.measure_into = None;
    // Patches and removals are done above, in order (render_image would do the removals again).
    s.removals = Vec::new();
    s.crop = crate::crop::Crop::default();
    s.keystone = crate::keystone::Keystone::default();
    if s.tools.adaptive_vignette != 0.0 && s.tools.vignette_map.is_none() {
        let mask = preview.subject_mask.get_or_init(|| crate::mask::subject(&preview.image).map(Arc::new));
        s.tools.vignette_map = mask.as_ref().and_then(|m| crate::subject::from_mask(m, preview.image.width, preview.image.height)).map(Arc::new);
    }
    if s.subject_mask.is_none() && s.gradients.iter().any(crate::gradient::needs_subject) {
        s.subject_mask = preview.subject_refined();
    }
    let img = crate::commands::render_image(&part, &s).to_rgb8();

    // The view, sampled from the rendered area as crop::apply samples.
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let out = image::RgbImage::from_fn(out_w, out_h, |ox, oy| {
        let x = dx0 + (ox as f32 + 0.5) / sc;
        let y = dy0 + (oy as f32 + 0.5) / sc;
        match map(x, y) {
            Some((px, py)) => {
                let (rx, ry) = ((px - ax0 as f32) * ps, (py - ay0 as f32) * ps);
                if rx < 0.0 || ry < 0.0 || rx > iw || ry > ih {
                    image::Rgb([255, 255, 255])
                } else {
                    sample(&img, rx, ry)
                }
            }
            None => image::Rgb([255, 255, 255]),
        }
    });
    Ok(out)
}

/// Bilinear sample at (x, y) in pixels (pixel centres at +0.5).
fn sample(img: &image::RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let px = |xx: i64, yy: i64| img.get_pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32).0.map(|v| v as f32);
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    image::Rgb(std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bot = c[i] + (d[i] - c[i]) * tx;
        (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8
    }))
}
