//! Keystone (Transform): Vertical, Horizontal, Rotate and Scale, as a
//! perspective warp of the upright photo (after quarter turns, before the
//! crop's straighten and cut, as Lightroom orders them). The model is the
//! preview's (`src/transform.ts`): the photo is a plane seen by a camera whose
//! focal length is its long edge; Vertical / Horizontal tilt that plane about
//! its middle, then Rotate turns it and Scale sizes it. The picture keeps its
//! size; what the warp uncovers is white, as in Lightroom.

use crate::detail::Channel;
use image::{ImageBuffer, Rgb};

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Keystone {
    /// -100..100, Lightroom's Vertical / Horizontal.
    pub vertical: f32,
    pub horizontal: f32,
    /// Degrees, positive clockwise.
    pub rotate: f32,
    /// Percent, 100 = unchanged.
    pub scale: f32,
}

impl Default for Keystone {
    fn default() -> Self {
        Self { vertical: 0.0, horizontal: 0.0, rotate: 0.0, scale: 100.0 }
    }
}

/// Tilt per slider unit, degrees (the preview's `TILT_DEG_PER_UNIT`).
const TILT_DEG_PER_UNIT: f32 = 0.4;
/// The camera's focal length as a fraction of the long edge.
const FOCAL: f32 = 1.0;

type Mat3 = [[f32; 3]; 3];

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

impl Keystone {
    pub fn is_none(&self) -> bool {
        self.vertical == 0.0 && self.horizontal == 0.0 && self.rotate == 0.0 && (self.scale - 100.0).abs() < 1e-3
    }

    /// Where each source point (pixels from the middle) lands, as a
    /// homography, for a picture whose long edge is `long` pixels.
    fn forward(&self, long: f32) -> Mat3 {
        let p = FOCAL * long;
        // As CSS: rotateX(-vertical) rotateY(-horizontal), y down, z towards the viewer.
        let (ax, ay) = ((-self.vertical * TILT_DEG_PER_UNIT).to_radians(), (-self.horizontal * TILT_DEG_PER_UNIT).to_radians());
        let rx: Mat3 = [[1.0, 0.0, 0.0], [0.0, ax.cos(), -ax.sin()], [0.0, ax.sin(), ax.cos()]];
        let ry: Mat3 = [[ay.cos(), 0.0, ay.sin()], [0.0, 1.0, 0.0], [-ay.sin(), 0.0, ay.cos()]];
        let m = mul(&rx, &ry);
        // The plane's points (x, y, 0) through m, then seen from distance p.
        let project: Mat3 = [[p * m[0][0], p * m[0][1], 0.0], [p * m[1][0], p * m[1][1], 0.0], [-m[2][0], -m[2][1], p]];
        let (r, k) = (self.rotate.to_radians(), self.scale / 100.0);
        let turn_scale: Mat3 = [[k * r.cos(), -k * r.sin(), 0.0], [k * r.sin(), k * r.cos(), 0.0], [0.0, 0.0, 1.0]];
        mul(&project, &turn_scale)
    }
}

impl Keystone {
    /// Where a point (x, y) of the warped picture (pixels from its top left,
    /// a w x h picture) comes from in the picture before the warp, as `apply`
    /// samples it. None for a point the warp leaves uncovered.
    pub fn source_point(&self, w: f32, h: f32) -> impl Fn(f32, f32) -> Option<(f32, f32)> {
        let none = self.is_none();
        let inv = crate::whitebalance::invert3(&self.forward(w.max(h)));
        let (cx, cy) = (w / 2.0, h / 2.0);
        move |x, y| {
            if none {
                return Some((x, y));
            }
            let (x, y) = (x - cx, y - cy);
            let q = [inv[0][0] * x + inv[0][1] * y + inv[0][2], inv[1][0] * x + inv[1][1] * y + inv[1][2], inv[2][0] * x + inv[2][1] * y + inv[2][2]];
            (q[2] > 1e-6).then(|| (q[0] / q[2] + cx, q[1] / q[2] + cy))
        }
    }
}

impl Keystone {
    /// The smallest Scale (percent, from 100) at which this warp leaves no white: every corner of
    /// the w x h frame then comes from inside the picture (the picture is convex and the warp
    /// keeps straight lines straight, so the corners are enough). Whole percents, rounded up.
    pub fn cover_scale(&self, w: f32, h: f32) -> f32 {
        let covers = |scale: f32| {
            let f = Keystone { scale, ..*self }.source_point(w, h);
            [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
                .iter()
                .all(|&(x, y)| f(x, y).is_some_and(|(sx, sy)| sx >= -0.5 && sy >= -0.5 && sx <= w + 0.5 && sy <= h + 0.5))
        };
        if covers(100.0) {
            return 100.0;
        }
        let (mut lo, mut hi) = (100.0f32, 400.0f32);
        if !covers(hi) {
            return hi;
        }
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if covers(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi.ceil()
    }
}

/// The picture warped as `k` says (same size).
pub fn apply<T: Channel>(img: ImageBuffer<Rgb<T>, Vec<T>>, k: &Keystone) -> ImageBuffer<Rgb<T>, Vec<T>>
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    if k.is_none() {
        return img;
    }
    let (w, h) = (img.width(), img.height());
    let inv = crate::whitebalance::invert3(&k.forward(w.max(h) as f32));
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let rows: Vec<Vec<Rgb<T>>> = {
        use rayon::prelude::*;
        (0..h)
            .into_par_iter()
            .map(|oy| {
                (0..w)
                    .map(|ox| {
                        let (x, y) = (ox as f32 + 0.5 - cx, oy as f32 + 0.5 - cy);
                        let q = [inv[0][0] * x + inv[0][1] * y + inv[0][2], inv[1][0] * x + inv[1][1] * y + inv[1][2], inv[2][0] * x + inv[2][1] * y + inv[2][2]];
                        if q[2] <= 1e-6 {
                            return Rgb([T::from_f32(T::MAX); 3]);
                        }
                        let (sx, sy) = (q[0] / q[2] + cx, q[1] / q[2] + cy);
                        if sx < 0.0 || sy < 0.0 || sx > w as f32 || sy > h as f32 {
                            return Rgb([T::from_f32(T::MAX); 3]);
                        }
                        crate::crop::sample(&img, sx, sy)
                    })
                    .collect()
            })
            .collect()
    };
    ImageBuffer::from_fn(w, h, |x, y| rows[y as usize][x as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(k: &Keystone, long: f32, x: f32, y: f32) -> (f32, f32) {
        let m = k.forward(long);
        let q = [m[0][0] * x + m[0][1] * y + m[0][2], m[1][0] * x + m[1][1] * y + m[1][2], m[2][0] * x + m[2][1] * y + m[2][2]];
        (q[0] / q[2], q[1] / q[2])
    }

    #[test]
    fn none_is_identity_and_scale_scales() {
        let (x, y) = map(&Keystone::default(), 1000.0, 120.0, -80.0);
        assert!((x - 120.0).abs() < 1e-3 && (y + 80.0).abs() < 1e-3);
        let (x, _) = map(&Keystone { scale: 120.0, ..Default::default() }, 1000.0, 100.0, 0.0);
        assert!((x - 120.0).abs() < 1e-3);
    }

    #[test]
    fn cover_scale_hides_the_corners() {
        assert_eq!(Keystone::default().cover_scale(3000.0, 2000.0), 100.0);
        // A 3:2 photo turned 5 degrees needs cos 5 + 1.5 sin 5 = 1.127: 113%.
        let s = Keystone { rotate: 5.0, ..Default::default() }.cover_scale(3000.0, 2000.0);
        assert_eq!(s, 113.0);
        let k = Keystone { rotate: 5.0, vertical: 20.0, ..Default::default() };
        assert!(k.cover_scale(3000.0, 2000.0) > s);
    }

    #[test]
    fn vertical_makes_top_and_bottom_different_widths() {
        let k = Keystone { vertical: 50.0, ..Default::default() };
        let (top, _) = map(&k, 1000.0, 300.0, -300.0);
        let (bottom, _) = map(&k, 1000.0, 300.0, 300.0);
        assert!((top - bottom).abs() > 10.0, "{top} {bottom}");
    }
}
