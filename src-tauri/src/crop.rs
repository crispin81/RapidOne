//! Crop, straighten and quarter turns, on the finished picture (after the
//! detail tools), so the preview and the export are cut the same way. The
//! same geometry as RapidRetouch's crop: the photo is first turned by quarter
//! turns, then by `angle` degrees (positive clockwise) about its centre on a
//! canvas the same size, then x0..x1, y0..y1 (fractions of that canvas) kept.

use crate::detail::Channel;
use image::{ImageBuffer, Rgb};
#[cfg(test)]
use image::RgbImage;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Crop {
    /// Quarter turns clockwise, 0-3 (for a photo that opened sideways).
    pub quarter: u8,
    /// Straighten, degrees, -45..45 (positive clockwise).
    pub angle: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Self { quarter: 0, angle: 0.0, x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }
    }
}

impl Crop {
    /// No straighten and no cut (quarter turns aside).
    pub fn is_cut_free(&self) -> bool {
        self.angle == 0.0 && self.x0 <= 0.0 && self.y0 <= 0.0 && self.x1 >= 1.0 && self.y1 >= 1.0
    }

    pub fn is_none(&self) -> bool {
        self.quarter % 4 == 0 && self.angle == 0.0 && self.x0 <= 0.0 && self.y0 <= 0.0 && self.x1 >= 1.0 && self.y1 >= 1.0
    }
}

/// Bilinear sample of `img` at (x, y) in pixels; outside it is black.
pub fn sample<T: Channel>(img: &ImageBuffer<Rgb<T>, Vec<T>>, x: f32, y: f32) -> Rgb<T>
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let px = |xx: i64, yy: i64| -> [f32; 3] {
        let (cx, cy) = (xx.clamp(0, w - 1), yy.clamp(0, h - 1));
        let p = img.get_pixel(cx as u32, cy as u32);
        [p[0].to_f32(), p[1].to_f32(), p[2].to_f32()]
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut out = [T::from_f32(0.0); 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        out[k] = T::from_f32(top + (bottom - top) * ty);
    }
    Rgb(out)
}

/// The picture turned, straightened and cut as `crop` says.
pub fn apply<T: Channel>(img: ImageBuffer<Rgb<T>, Vec<T>>, crop: &Crop, keystone: &crate::keystone::Keystone) -> ImageBuffer<Rgb<T>, Vec<T>>
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    let img = match crop.quarter % 4 {
        1 => image::imageops::rotate90(&img),
        2 => image::imageops::rotate180(&img),
        3 => image::imageops::rotate270(&img),
        _ => img,
    };
    // Keystone on the upright photo, before the straighten and cut (Lightroom's order).
    let img = crate::keystone::apply(img, keystone);
    let full = crop.angle == 0.0 && crop.x0 <= 0.0 && crop.y0 <= 0.0 && crop.x1 >= 1.0 && crop.y1 >= 1.0;
    if full {
        return img;
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (x0, x1) = (crop.x0.clamp(0.0, 1.0), crop.x1.clamp(0.0, 1.0));
    let (y0, y1) = (crop.y0.clamp(0.0, 1.0), crop.y1.clamp(0.0, 1.0));
    let ow = ((x1 - x0) * w).round().max(1.0) as u32;
    let oh = ((y1 - y0) * h).round().max(1.0) as u32;
    // Each output pixel's place on the turned canvas, then back to where it
    // came from on the photo (turned the other way about the centre).
    let t = (-crop.angle).to_radians();
    let (s, c) = t.sin_cos();
    // Row by row on all cores (one core took ~0.6 s a 24 MP export, 24 Oct); the same sums per pixel.
    use rayon::prelude::*;
    let mut out = vec![T::from_f32(0.0); ow as usize * oh as usize * 3];
    out.par_chunks_mut(ow as usize * 3).enumerate().for_each(|(oy, row)| {
        for (ox, px) in row.chunks_exact_mut(3).enumerate() {
            let x = x0 * w + ox as f32 + 0.5 - w / 2.0;
            let y = y0 * h + oy as f32 + 0.5 - h / 2.0;
            px.copy_from_slice(&sample(&img, x * c - y * s + w / 2.0, x * s + y * c + h / 2.0).0);
        }
    });
    ImageBuffer::from_raw(ow, oh, out).expect("one value per channel")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_crop_keeps_the_pixels_it_covers() {
        let img = RgbImage::from_fn(10, 8, |x, y| Rgb([x as u8 * 20, y as u8 * 30, 0]));
        let out = apply(img.clone(), &Crop { x0: 0.2, y0: 0.25, x1: 0.7, y1: 0.75, ..Crop::default() }, &Default::default());
        assert_eq!((out.width(), out.height()), (5, 4));
        assert_eq!(out.get_pixel(0, 0), img.get_pixel(2, 2));
        assert_eq!(out.get_pixel(4, 3), img.get_pixel(6, 5));
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides() {
        let img = RgbImage::new(10, 8);
        let out = apply(img, &Crop { quarter: 1, ..Crop::default() }, &Default::default());
        assert_eq!((out.width(), out.height()), (8, 10));
    }
}
