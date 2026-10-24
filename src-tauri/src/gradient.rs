//! Linear and radial gradients, and the subject mask: local adjustments that
//! fade across the photo, Lightroom-style. A subject mask is a recipe (its
//! sliders, Feather and Invert), not a shape: each photo's own subject (the
//! subject model, `mask`) is found for it, so it pastes onto a batch as
//! Lightroom's Select Subject does. RapidOne's own formulas (not matched to
//! Lightroom's local sliders yet). Positions are fractions of the upright,
//! uncropped photo, like removals, so a crop doesn't move them.
//!
//! Exposure, Temperature and Tint act on scene light (before the tone
//! controls, as the global sliders do); Contrast, Highlights, Shadows and
//! Saturation on the rendered colour.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Linear,
    Radial,
    /// The photo's main subject (its subject mask), softened by `feather`.
    Subject,
    /// Painted by hand (`strokes`).
    Brush,
}

/// A part of a combined mask: another shape added to the mask (where either is) or
/// subtracted from it (taken out where the part is), as Lightroom's Add / Subtract.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Part {
    pub subtract: bool,
    /// Its shape (kind and placement; its sliders aren't used).
    pub shape: Gradient,
}

/// One stroke of a brush mask: its path (fractions of the photo), radius (a
/// fraction of the photo's width), 0..100 feather (how soft its edge is) and
/// whether it paints the mask or erases it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub size: f32,
    pub feather: f32,
    pub erase: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Gradient {
    pub kind: Kind,
    /// Its effect switched off (the layer's eye in the masks panel), the mask kept.
    pub hidden: bool,
    /// Linear: full effect at `full`, none at `zero` (fractions of the photo).
    pub full: [f32; 2],
    pub zero: [f32; 2],
    /// Radial: centre, and radii as fractions of the photo's width / height.
    pub centre: [f32; 2],
    pub rx: f32,
    pub ry: f32,
    /// Radial: degrees, clockwise.
    pub angle: f32,
    /// Radial: 0..100, how wide the fade at the oval's edge is. Subject: how
    /// soft the mask's edge is (0: the model's own edge).
    pub feather: f32,
    /// Radial: the effect outside the oval instead of inside. Subject: on the
    /// background instead of the subject.
    pub invert: bool,
    /// Brush: the strokes painted, in order.
    pub strokes: Vec<Stroke>,
    /// Other shapes added to or subtracted from this one (a combined mask).
    pub parts: Vec<Part>,
    /// Subject: the subject mask as found on the photo it was copied from (a PNG, base64),
    /// used as it is instead of finding the subject again: a combined mask with a subject in
    /// it is pasted as a fixed shape. None: the photo's own subject.
    pub frozen: Option<String>,
    /// Stops.
    pub exposure: f32,
    /// -100..100 each.
    pub temperature: f32,
    pub tint: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    /// -100..100 each, as the global sliders' numbers. Whites and Blacks on
    /// the rendered lightness; Texture, Clarity and Dehaze are the global
    /// tools faded in by the gradient's weight (`definition_amounts`).
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub saturation: f32,
}

/// The gradients' combined amounts at one pixel.
#[derive(Clone, Copy, Default)]
pub struct Local {
    ev: f32,
    temp: f32,
    tint: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    saturation: f32,
    /// Texture, Clarity, Dehaze (-100..100 scale).
    pub definition: [f32; 3],
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Gradient {
    /// 0..1: how much of the effect at (x, y), fractions of a photo `aspect` (w / h) wide.
    pub fn weight(&self, x: f32, y: f32, aspect: f32) -> f32 {
        // Square units: x scaled by the aspect.
        match self.kind {
            Kind::Linear => {
                let (fx, fy) = (self.full[0] * aspect, self.full[1]);
                let (dx, dy) = (self.zero[0] * aspect - fx, self.zero[1] - fy);
                let len2 = (dx * dx + dy * dy).max(1e-8);
                let t = ((x * aspect - fx) * dx + (y - fy) * dy) / len2;
                // 100% at the full line to 0% at the zero line, evenly, as a Photoshop /
                // Affinity gradient. (Chris, 18 Oct: not Lightroom's S-curve.)
                1.0 - t.clamp(0.0, 1.0)
            }
            Kind::Radial => {
                let (px, py) = ((x - self.centre[0]) * aspect, y - self.centre[1]);
                let (s, c) = (-self.angle.to_radians()).sin_cos();
                let (ru, rv) = (px * c - py * s, px * s + py * c);
                let (rx, ry) = ((self.rx * aspect).max(1e-4), self.ry.max(1e-4));
                let d = ((ru / rx).powi(2) + (rv / ry).powi(2)).sqrt();
                // The fade starts inside the oval and runs past its edge, as
                // Lightroom's does: our formula fitted to the gradient kit
                // (17 Oct, 12 photos, feather 25/50/75/100, within ~2%):
                // from 1 - 1.2 f^0.8 to 1 + 0.32 f^0.35 of the radius.
                let f = (self.feather / 100.0).clamp(0.0, 1.0);
                let w = if f <= 1e-3 {
                    if d <= 1.0 { 1.0 } else { 0.0 }
                } else {
                    let (a, b) = (1.0 - 1.2 * f.powf(0.8), 1.0 + 0.32 * f.powf(0.35));
                    1.0 - smooth((d - a) / (b - a))
                };
                if self.invert { 1.0 - w } else { w }
            }
            // Need the photo's mask, or the painted one: see `Prepared`.
            Kind::Subject | Kind::Brush => 0.0,
        }
    }
}

/// The subject mask softened for a Feather of 0..100: a blur up to SUBJECT_SOFTEST
/// of the mask's width (two box passes, near a Gaussian).
const SUBJECT_SOFTEST: f32 = 0.025;

/// A brush mask painted: 0..1 per pixel of a grid over the photo, its long side
/// BRUSH_GRID pixels (enough for a soft-edged local adjustment).
const BRUSH_GRID: usize = 1024;

/// A mask over the whole photo, 0..1 per pixel of a `w` x `h` grid (the photo's shape):
/// a brush mask painted, or a subject mask refined (`refine_subject`).
#[derive(Debug)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Grid {
    /// The strokes painted on a grid over a photo `aspect` (w / h) wide: each a run of
    /// soft round dabs, full inside (1 - feather) of the radius and fading to its edge;
    /// a stroke paints where it's stronger, an erasing one takes away.
    pub fn painted(strokes: &[Stroke], aspect: f32) -> Grid {
        let (w, h) = if aspect >= 1.0 {
            (BRUSH_GRID, ((BRUSH_GRID as f32 / aspect).round() as usize).max(1))
        } else {
            (((BRUSH_GRID as f32 * aspect).round() as usize).max(1), BRUSH_GRID)
        };
        let mut data = vec![0.0f32; w * h];
        for st in strokes {
            let r = (st.size * w as f32).max(0.5);
            let hard = 1.0 - (st.feather / 100.0).clamp(0.0, 1.0);
            let mut dab = |cx: f32, cy: f32| {
                if cx + r < 0.0 || cy + r < 0.0 || cx - r > w as f32 || cy - r > h as f32 {
                    return;
                }
                let (x0, x1) = ((cx - r).floor().max(0.0) as usize, ((cx + r).ceil() as usize).min(w - 1));
                let (y0, y1) = ((cy - r).floor().max(0.0) as usize, ((cy + r).ceil() as usize).min(h - 1));
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() / r;
                        if d >= 1.0 {
                            continue;
                        }
                        let k = if d <= hard { 1.0 } else { 1.0 - smooth((d - hard) / (1.0 - hard).max(1e-4)) };
                        let v = &mut data[y * w + x];
                        *v = if st.erase { *v * (1.0 - k) } else { v.max(k) };
                    }
                }
            };
            // Dabs a quarter of the radius apart along the path.
            let pts: Vec<(f32, f32)> = st.points.iter().map(|p| (p[0] * w as f32, p[1] * h as f32)).collect();
            if let Some(&(x, y)) = pts.first() {
                dab(x, y);
            }
            for pair in pts.windows(2) {
                let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
                let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                let n = (len / (r * 0.25).max(0.5)).ceil().max(1.0) as usize;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    dab(ax + (bx - ax) * t, ay + (by - ay) * t);
                }
            }
        }
        Grid { w, h, data }
    }

    /// The mask at (x, y), fractions of the photo, bilinear.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let fx = (x * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (y * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * self.w + x];
        let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
        let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// A box blur of `data` (`w` x `h`) over 2r + 1 pixels, rows then columns, clamped at the
/// edges: running sums, so the cost doesn't grow with r.
fn box_blur(data: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return data.to_vec();
    }
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    let line = |src: &[f32], dst: &mut [f32], n: usize, at: &dyn Fn(usize) -> usize| {
        // Prefix sums along the line, then each window's mean.
        let mut pre = vec![0.0f64; n + 1];
        for i in 0..n {
            pre[i + 1] = pre[i] + src[at(i)] as f64;
        }
        for i in 0..n {
            let (lo, hi) = (i.saturating_sub(r), (i + r).min(n - 1));
            dst[at(i)] = ((pre[hi + 1] - pre[lo]) / (hi - lo + 1) as f64) as f32;
        }
    };
    for y in 0..h {
        line(data, &mut tmp, w, &|x| y * w + x);
    }
    for x in 0..w {
        line(&tmp, &mut out, h, &|y| y * w + x);
    }
    out
}

impl Grid {
    /// Softened for a Feather of 0..100: a blur up to SUBJECT_SOFTEST of the width (two
    /// box passes, near a Gaussian).
    pub fn feathered(&self, feather: f32) -> Grid {
        let r = ((feather / 100.0).clamp(0.0, 1.0) * SUBJECT_SOFTEST * self.w as f32 / 2.0).round() as usize;
        let once = box_blur(&self.data, self.w, self.h, r);
        Grid { w: self.w, h: self.h, data: box_blur(&once, self.w, self.h, r) }
    }
}

impl Grid {
    /// The grid as a greyscale PNG, base64, its long side at most `long` (frozen subject masks).
    pub fn to_png_base64(&self, long: usize) -> Option<String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let s = (long as f32 / self.w.max(self.h) as f32).min(1.0);
        let (w, h) = (((self.w as f32 * s).round() as u32).max(1), ((self.h as f32 * s).round() as u32).max(1));
        let img = image::GrayImage::from_fn(w, h, |x, y| {
            let v = self.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            image::Luma([(v.clamp(0.0, 1.0) * 255.0).round() as u8])
        });
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).ok()?;
        Some(STANDARD.encode(bytes))
    }

    /// A grid from `to_png_base64`.
    pub fn from_png_base64(data: &str) -> Option<Grid> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let bytes = STANDARD.decode(data.split_once("base64,").map_or(data, |(_, d)| d)).ok()?;
        let img = image::load_from_memory(&bytes).ok()?.to_luma8();
        Some(Grid { w: img.width() as usize, h: img.height() as usize, data: img.pixels().map(|p| p[0] as f32 / 255.0).collect() })
    }
}

/// The long side of a refined subject mask.
const REFINED_LONG: usize = 2048;

/// The subject mask (the model's, 1024 x 1024 over the frame) refined against the
/// photo itself: scaled up to REFINED_LONG on the photo's long side, its edge snapped
/// to the photo's own edges (hair, shoulders, clothes) with a guided filter on the
/// photo's lightness (He et al., "Guided Image Filtering"), so it holds up zoomed in.
pub fn refine_subject(mask: &crate::mask::Mask, image: &crate::pipeline::SceneImage) -> Grid {
    let small = image.downscaled(REFINED_LONG);
    let (w, h) = (small.width, small.height);
    // The guide: perceptual lightness (cube root of luminance), 0..1-ish.
    let peak = small.px.iter().map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).fold(1e-4f32, f32::max);
    let guide: Vec<f32> = small.px.iter().map(|p| ((0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0) / peak).cbrt()).collect();
    let rough: Vec<f32> = (0..w * h).map(|i| mask.at(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)).collect();
    // Radius: about one rough-mask pixel's worth either side, a little more; eps: how
    // strong an edge in the photo has to be to be followed.
    let r = (w.max(h) / 256).max(2);
    let eps = 1e-3f32;
    let n = w * h;
    let ip: Vec<f32> = (0..n).map(|i| guide[i] * rough[i]).collect();
    let ii: Vec<f32> = (0..n).map(|i| guide[i] * guide[i]).collect();
    let (m_i, m_p, m_ip, m_ii) = (box_blur(&guide, w, h, r), box_blur(&rough, w, h, r), box_blur(&ip, w, h, r), box_blur(&ii, w, h, r));
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for i in 0..n {
        let var = m_ii[i] - m_i[i] * m_i[i];
        let cov = m_ip[i] - m_i[i] * m_p[i];
        a[i] = cov / (var + eps);
        b[i] = m_p[i] - a[i] * m_i[i];
    }
    let (m_a, m_b) = (box_blur(&a, w, h, r), box_blur(&b, w, h, r));
    let data = (0..n).map(|i| (m_a[i] * guide[i] + m_b[i]).clamp(0.0, 1.0)).collect();
    Grid { w, h, data }
}


/// One shape of a mask, ready to sample: a gradient's formula, or a mask grid (a subject
/// mask softened by its Feather, or brush strokes painted).
enum Comp {
    Formula(Gradient),
    Map(Grid),
    Nothing,
}

impl Comp {
    fn new(shape: &Gradient, aspect: f32, subject: Option<&Grid>) -> Comp {
        match shape.kind {
            Kind::Subject => {
                let frozen = shape.frozen.as_deref().and_then(Grid::from_png_base64);
                match frozen.as_ref().or(subject) {
                    Some(m) => Comp::Map(m.feathered(shape.feather)),
                    None => Comp::Nothing,
                }
            }
            Kind::Brush => Comp::Map(Grid::painted(&shape.strokes, aspect)),
            // Its own Invert is applied by the mask (see `MaskShape::at`).
            _ => Comp::Formula(Gradient { invert: false, ..shape.clone() }),
        }
    }

    fn at(&self, x: f32, y: f32, aspect: f32) -> f32 {
        match self {
            Comp::Formula(g) => g.weight(x, y, aspect),
            Comp::Map(m) => m.at(x, y).clamp(0.0, 1.0),
            Comp::Nothing => 0.0,
        }
    }
}

/// A whole mask ready to sample: its own shape, then its parts added (the stronger of the
/// two) or subtracted (what's there times one minus the part), each part with its own
/// Invert; the mask's Invert last, on the whole.
pub struct MaskShape {
    base: Comp,
    parts: Vec<(bool, bool, Comp)>,
    invert: bool,
    aspect: f32,
}

impl MaskShape {
    /// `subject`: the photo's subject mask, for a subject shape not frozen (without it, none).
    pub fn new(g: &Gradient, aspect: f32, subject: Option<&Grid>) -> MaskShape {
        MaskShape {
            base: Comp::new(g, aspect, subject),
            parts: g.parts.iter().map(|p| (p.subtract, p.shape.invert, Comp::new(&p.shape, aspect, subject))).collect(),
            invert: g.invert,
            aspect,
        }
    }

    /// 0..1: how much of the mask at (x, y), fractions of the photo.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let mut w = self.base.at(x, y, self.aspect);
        for (subtract, invert, c) in &self.parts {
            let p = c.at(x, y, self.aspect);
            let p = if *invert { 1.0 - p } else { p };
            w = if *subtract { w * (1.0 - p) } else { w.max(p) };
        }
        if self.invert { 1.0 - w } else { w }
    }
}

/// Whether a mask needs the photo's own subject (a subject shape in it, not frozen).
pub fn needs_subject(g: &Gradient) -> bool {
    let live = |s: &Gradient| s.kind == Kind::Subject && s.frozen.is_none();
    live(g) || g.parts.iter().any(|p| live(&p.shape))
}

/// The photo's gradients, ready to render.
pub struct Prepared<'a> {
    list: &'a [Gradient],
    masks: Vec<MaskShape>,
}

impl<'a> Prepared<'a> {
    /// `subject`: the photo's subject mask, for subject masks (without it they do nothing).
    pub fn new(list: &'a [Gradient], width: usize, height: usize, subject: Option<&Grid>) -> Option<Self> {
        let used = |g: &Gradient| {
            [g.exposure, g.temperature, g.tint, g.contrast, g.highlights, g.shadows, g.whites, g.blacks, g.texture, g.clarity, g.dehaze, g.saturation]
                .iter()
                .any(|&v| v != 0.0)
        };
        let aspect = width as f32 / height.max(1) as f32;
        list.iter().any(used).then(|| Prepared { list, masks: list.iter().map(|g| MaskShape::new(g, aspect, subject)).collect() })
    }

    /// Amounts at pixel `i` of a `w` x `h` image.
    pub fn at(&self, i: usize, w: usize, h: usize) -> Local {
        self.at_frac(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32)
    }

    /// Amounts at (x, y), fractions of the whole photo.
    pub fn at_frac(&self, x: f32, y: f32) -> Local {
        let mut l = Local::default();
        for (gi, g) in self.list.iter().enumerate() {
            if g.hidden {
                continue;
            }
            let k = self.masks[gi].at(x, y);
            if k <= 0.0 {
                continue;
            }
            l.ev += k * g.exposure;
            l.temp += k * g.temperature / 100.0;
            l.tint += k * g.tint / 100.0;
            l.contrast += k * g.contrast / 100.0;
            l.highlights += k * g.highlights / 100.0;
            l.shadows += k * g.shadows / 100.0;
            l.whites += k * g.whites / 100.0;
            l.blacks += k * g.blacks / 100.0;
            l.saturation += k * g.saturation / 100.0;
            l.definition[0] += k * g.texture;
            l.definition[1] += k * g.clarity;
            l.definition[2] += k * g.dehaze;
        }
        l
    }

    /// Whether any gradient uses Texture, Clarity or Dehaze.
    pub fn has_definition(&self) -> bool {
        self.list.iter().any(|g| g.texture != 0.0 || g.clarity != 0.0 || g.dehaze != 0.0)
    }
}

/// Lightroom's oval (Top/Left/Bottom/Right box and Angle, fractions of a
/// photo `aspect` wide) as RapidOne's: centre, radii and angle.
///
/// Lightroom turns the oval in 0..1 photo coordinates, not pixels, and its
/// Angle is the direction it shows on the photo: on the gradient kit (17 Oct,
/// a 30 degree oval on 3:2 photos) the oval that fits is the box's turned by
/// atan(aspect x tan(Angle)) in 0..1 coordinates - long axis at 31 degrees,
/// radii 0.38 / 0.18 of the height (free fit: 0.376 / 0.157, rms at the
/// unturned circle's noise floor). A turned oval in 0..1 coordinates is an
/// oval in pixels too, so this is exact for that rule, feather included.
pub fn lightroom_oval(centre: [f32; 2], rx: f32, ry: f32, angle: f32, aspect: f32) -> ([f32; 2], f32, f32, f32) {
    if angle.rem_euclid(180.0).abs() < 1e-4 || (aspect - 1.0).abs() < 1e-6 {
        return (centre, rx, ry, angle);
    }
    let a = angle.to_radians();
    // The turn in 0..1 coordinates (RapidOne's sign: ours = -Lightroom's).
    let t = -(aspect * a.sin()).atan2(a.cos());
    let (s, c) = t.sin_cos();
    // d^2 = p^T Q p; in 0..1 coordinates Q = R^T diag(1/rx^2, 1/ry^2) R,
    // R = [[c, s], [-s, c]] (ru = c px + s py, rv = -s px + c py).
    let (ia, ib) = (1.0 / (rx * rx), 1.0 / (ry * ry));
    let q = [c * c * ia + s * s * ib, c * s * (ia - ib), s * s * ia + c * c * ib];
    // Into square units (x times the aspect): Q_s = D^-1 Q D^-1, D = diag(aspect, 1).
    let qs = [q[0] / (aspect * aspect), q[1] / aspect, q[2]];
    // Its axes: the eigenvectors of [[qs0, qs1], [qs1, qs2]].
    let tr = qs[0] + qs[2];
    let det = qs[0] * qs[2] - qs[1] * qs[1];
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
    // The axis closest to the photo's x axis is RapidOne's rx.
    let v1 = if qs[1].abs() > 1e-12 { (l1 - qs[2], qs[1]) } else if qs[0] >= qs[2] { (1.0, 0.0) } else { (0.0, 1.0) };
    let (vx, vy) = { let n = (v1.0 * v1.0 + v1.1 * v1.1).sqrt().max(1e-12); (v1.0 / n, v1.1 / n) };
    let (lx, ly, ux, uy) = if vx.abs() >= vy.abs() { (l1, l2, vx, vy) } else { (l2, l1, -vy, vx) };
    // RapidOne's angle: ru = px cos(a) + py sin(a) runs along (cos a, sin a).
    let ours = uy.atan2(ux).to_degrees();
    (centre, 1.0 / lx.max(1e-12).sqrt() / aspect, 1.0 / ly.max(1e-12).sqrt(), ours)
}

impl Local {
    /// Exposure and white balance, on scene light (linear Rec.2020).
    pub fn scene(&self, p: [f32; 3]) -> [f32; 3] {
        if self.ev == 0.0 && self.temp == 0.0 && self.tint == 0.0 {
            return p;
        }
        // Temperature: warmer = more red, less blue; Tint: + = magenta (less
        // green). Strengths set to Lightroom's local sliders (gradient kit,
        // 17 Oct, tools/ml/gradient_sliders_fit.py): Temperature x1.35 and
        // Tint x0.82 of the first guesses.
        let gain = [self.ev + 0.47 * self.temp + 0.082 * self.tint, self.ev - 0.246 * self.tint, self.ev - 0.61 * self.temp + 0.082 * self.tint];
        std::array::from_fn(|k| p[k] * gain[k].exp2())
    }

    /// Contrast, Highlights, Shadows and Saturation, on the rendered colour (linear).
    pub fn display(&self, c: [f32; 3]) -> [f32; 3] {
        if self.contrast == 0.0 && self.highlights == 0.0 && self.shadows == 0.0 && self.whites == 0.0 && self.blacks == 0.0 && self.saturation == 0.0 {
            return c;
        }
        let y = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(0.0);
        let mut c = c;
        if y > 1e-6 && (self.contrast != 0.0 || self.highlights != 0.0 || self.shadows != 0.0 || self.whites != 0.0 || self.blacks != 0.0) {
            // On perceptual lightness (about L*/100).
            let l0 = y.powf(1.0 / 2.4).min(1.2);
            let mut l = l0;
            // Strengths set to Lightroom's local sliders (gradient kit, 17 Oct,
            // tools/ml/gradient_sliders_fit.py); Lightroom's aren't symmetric:
            // its + Highlights and - Shadows push harder than the other way.
            if self.contrast != 0.0 {
                // An S round mid-grey, gentler at the ends so nothing jumps to clipping.
                let d = l - 0.46;
                let k = if self.contrast > 0.0 { 0.31 } else { 0.23 };
                l += self.contrast * k * d * (1.0 - (d.abs() * 1.6).min(1.0));
            }
            if self.highlights != 0.0 {
                let w = smooth((l - 0.45) / 0.5);
                let k = if self.highlights > 0.0 { 0.5 } else { 0.12 };
                l += self.highlights * k * w * if self.highlights > 0.0 { (1.0 - l).max(0.0) } else { l };
            }
            if self.shadows != 0.0 {
                let w = 1.0 - smooth((l - 0.05) / 0.5);
                let k = if self.shadows > 0.0 { 0.1 } else { 0.42 };
                l += self.shadows * k * w * if self.shadows > 0.0 { (1.0 - l).max(0.0) } else { l };
            }
            // Whites and Blacks: the shapes the gradient kit measured (17 Oct,
            // first numbers, not refitted): at +-50, Whites moves lightness
            // about in proportion to it (+0.10 at 0.75), easing off near
            // white going up; Blacks in proportion to its darkness (+0.13 at
            // 0.15, nothing at white).
            if self.whites != 0.0 {
                l += if self.whites > 0.0 { self.whites * 0.27 * l * smooth((1.05 - l) / 0.3) } else { self.whites * 0.24 * l };
            }
            if self.blacks != 0.0 {
                l += self.blacks * if self.blacks > 0.0 { 0.31 } else { 0.34 } * (1.0 - l).max(0.0);
            }
            let k = l.max(0.0).powf(2.4) / y;
            c = c.map(|v| v * k);
        }
        if self.saturation != 0.0 {
            let y = 0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2];
            // Lightroom's + Saturation is gentler than its - (kit, 17 Oct).
            let s = (1.0 + self.saturation * if self.saturation > 0.0 { 0.58 } else { 0.95 }).max(0.0);
            c = c.map(|v| y + (v - y) * s);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_fades_from_full_to_zero() {
        let g = Gradient { kind: Kind::Linear, full: [0.5, 0.0], zero: [0.5, 0.5], exposure: 1.0, ..Default::default() };
        assert_eq!(g.weight(0.5, 0.0, 1.5), 1.0);
        // Half way: half, and a quarter of the way: three quarters (even).
        assert!((g.weight(0.2, 0.25, 1.5) - 0.5).abs() < 1e-4);
        assert!((g.weight(0.2, 0.125, 1.5) - 0.75).abs() < 1e-4);
        assert_eq!(g.weight(0.9, 0.9, 1.5), 0.0);
    }

    #[test]
    fn radial_full_inside_none_outside_and_inverts() {
        let mut g = Gradient { kind: Kind::Radial, centre: [0.5, 0.5], rx: 0.2, ry: 0.3, feather: 50.0, ..Default::default() };
        assert_eq!(g.weight(0.5, 0.5, 1.5), 1.0);
        assert_eq!(g.weight(0.95, 0.5, 1.5), 0.0);
        g.invert = true;
        assert_eq!(g.weight(0.95, 0.5, 1.5), 1.0);
    }

    #[test]
    fn lightroom_ovals_turn_in_photo_coordinates() {
        // The oval Lightroom's rule gives (turned by atan(aspect tan A) in 0..1
        // coordinates) and the converted one agree at every point.
        let (aspect, angle) = (1.5f32, 30.0f32);
        let (rx, ry, centre) = (0.15f32, 0.3f32, [0.5f32, 0.5f32]);
        let (c2, rx2, ry2, a2) = lightroom_oval(centre, rx, ry, angle, aspect);
        let g = Gradient { kind: Kind::Radial, centre: c2, rx: rx2, ry: ry2, angle: a2, feather: 50.0, exposure: 1.0, ..Default::default() };
        let t = -(aspect * angle.to_radians().sin()).atan2(angle.to_radians().cos());
        for (x, y) in [(0.5, 0.2), (0.6, 0.6), (0.3, 0.45), (0.55, 0.75), (0.7, 0.4)] {
            let (px, py) = (x - 0.5f32, y - 0.5f32);
            let (ru, rv) = (px * t.cos() + py * t.sin(), -px * t.sin() + py * t.cos());
            let d = ((ru / rx).powi(2) + (rv / ry).powi(2)).sqrt();
            let (a, b) = (1.0 - 1.2 * 0.5f32.powf(0.8), 1.0 + 0.32 * 0.5f32.powf(0.35));
            let want = 1.0 - smooth((d - a) / (b - a));
            assert!((g.weight(x, y, aspect) - want).abs() < 1e-3, "{x},{y}: {} vs {want}", g.weight(x, y, aspect));
        }
    }
}

#[cfg(test)]
mod brush_tests {
    use super::*;

    #[test]
    fn a_stroke_paints_its_path_softly_and_an_erasing_one_takes_it_away() {
        let paint = Stroke { points: vec![[0.2, 0.5], [0.8, 0.5]], size: 0.05, feather: 50.0, erase: false };
        let p = Grid::painted(&[paint.clone()], 1.5);
        assert!(p.at(0.5, 0.5) > 0.99, "full on the path");
        assert!(p.at(0.5, 0.1) < 0.01, "nothing far from it");
        let edge = p.at(0.5, 0.5 + 0.04 * 1.5);
        assert!(edge > 0.0 && edge < 1.0, "soft near the edge: {edge}");
        let erase = Stroke { points: vec![[0.5, 0.3], [0.5, 0.7]], size: 0.05, feather: 0.0, erase: true };
        let p = Grid::painted(&[paint, erase], 1.5);
        assert!(p.at(0.5, 0.5) < 0.01, "erased where crossed");
        assert!(p.at(0.3, 0.5) > 0.99, "kept elsewhere");
    }
}

#[cfg(test)]
mod combine_tests {
    use super::*;

    #[test]
    fn parts_add_and_subtract_and_invert_applies_to_the_whole() {
        let left = Gradient { kind: Kind::Linear, full: [0.0, 0.5], zero: [0.4, 0.5], ..Default::default() };
        let circle = Gradient { kind: Kind::Radial, centre: [0.8, 0.5], rx: 0.1, ry: 0.15, feather: 0.0, ..Default::default() };
        let hole = Gradient { kind: Kind::Radial, centre: [0.05, 0.5], rx: 0.04, ry: 0.06, feather: 0.0, ..Default::default() };
        let g = Gradient { parts: vec![Part { subtract: false, shape: circle }, Part { subtract: true, shape: hole }], ..left };
        let m = MaskShape::new(&g, 1.5, None);
        assert!(m.at(0.8, 0.5) > 0.99, "the added circle");
        assert!(m.at(0.05, 0.5) < 0.01, "the subtracted hole");
        assert!(m.at(0.15, 0.2) > 0.5, "the linear part");
        assert!(m.at(0.6, 0.9) < 0.01, "neither");
        let inv = MaskShape::new(&Gradient { invert: true, ..g }, 1.5, None);
        assert!(inv.at(0.6, 0.9) > 0.99 && inv.at(0.8, 0.5) < 0.01, "inverted as a whole");
    }

    #[test]
    fn a_frozen_subject_comes_back_from_its_png() {
        let grid = Grid { w: 4, h: 2, data: vec![0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0] };
        let back = Grid::from_png_base64(&grid.to_png_base64(1024).unwrap()).unwrap();
        assert_eq!((back.w, back.h), (4, 2));
        assert!((back.at(0.375, 0.5) - 1.0).abs() < 0.01 && back.at(0.0, 0.5) < 0.01);
    }
}
