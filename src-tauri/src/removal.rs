//! Object removal, non-destructive (like Lightroom's Remove): each removal is
//! a patch, the model's picture of the area without the object, blended into
//! the scene's light before white balance, exposure and every other tool, so
//! the sliders still work on it.
//!
//! Making one (`create`): the photo is developed at full size; a crop around
//! the chosen area is turned into an ordinary-looking picture (the scene light
//! at its normal brightness, sRGB), the area is marked with a red rectangle
//! (padded, so an object on a wall reads as an object on a wall) and FLUX.2
//! Klein (`klein`) is asked to remove what's marked. Its answer is stored as a
//! PNG in `sidecars/removals/`.
//!
//! Using one (`apply`): the PNG is turned back into scene light the same way,
//! scaled to the photo's size, and blended in over the area (grown a little,
//! soft-edged outside it so none of the object shows through); everywhere
//! else the photo is untouched.
//!
//! Tested 15-16 Oct on two weddings: signs on
//! brick, a sign behind a bride's head, fences, at 24 and 100 MP.

use image::{Rgb, RgbImage, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One entry of a photo's removals list: a removal (an object taken out by the model), or a patch
/// (`patch`: an area given another's texture or copy). Told apart by their fields (a patch has
/// `"kind": "patch"`), so a list saved before patches existed reads as it always did.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Spot {
    Patch(crate::patch::Patch),
    Removal(Removal),
}

impl Spot {
    pub fn removal(&self) -> Option<&Removal> {
        match self {
            Spot::Removal(r) => Some(r),
            Spot::Patch(_) => None,
        }
    }
    pub fn patch(&self) -> Option<&crate::patch::Patch> {
        match self {
            Spot::Patch(p) => Some(p),
            Spot::Removal(_) => None,
        }
    }
}

/// One removal, as the photo's edit stores it. Rectangles are fractions of
/// the upright, uncropped photo: [left, top, right, bottom].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Removal {
    /// The patch (PNG), covering `crop`.
    pub file: String,
    /// The part of the photo the patch covers.
    pub crop: [f32; 4],
    /// The object's area (what's replaced).
    pub area: [f32; 4],
    /// Stops the scene light was brightened by to make the picture the model saw.
    pub ev: f32,
}

/// Model working size, long edge (a multiple of 16).
const SIDE: u32 = 1024;
/// The crop around the area: this many times its size, at least MIN_CROP pixels.
const GROW: f32 = 2.2;
const MIN_CROP: f32 = 640.0;

// Linear Rec.2020 <-> linear sRGB (D65).
const TO_SRGB: [[f32; 3]; 3] = [[1.6605, -0.5876, -0.0728], [-0.1246, 1.1329, -0.0083], [-0.0182, -0.1006, 1.1187]];
const TO_2020: [[f32; 3]; 3] = [[0.6274, 0.3293, 0.0433], [0.0691, 0.9195, 0.0114], [0.0164, 0.0880, 0.8956]];

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn encode(px: [f32; 3], gain: f32) -> [u8; 3] {
    let s = mul(&TO_SRGB, px.map(|v| v * gain));
    s.map(|v| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (e * 255.0 + 0.5) as u8
    })
}

fn decode(rgb: [f32; 3], gain: f32) -> [f32; 3] {
    let lin = rgb.map(|v| {
        let v = v / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    });
    mul(&TO_2020, lin).map(|v| v / gain)
}

/// The crop around an area (pixels), `GROW` times its size, inside the photo.
fn crop_around(area: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let (cx, cy) = ((area[0] + area[2]) / 2.0, (area[1] + area[3]) / 2.0);
    // RAPIDONE_REMOVAL_CROP_GROW: another growth (testing).
    let grow = std::env::var("RAPIDONE_REMOVAL_CROP_GROW").ok().and_then(|v| v.parse().ok()).unwrap_or(GROW);
    let cw = ((area[2] - area[0]) * grow).max(MIN_CROP).min(w);
    let ch = ((area[3] - area[1]) * grow).max(MIN_CROP).min(h);
    let left = (cx - cw / 2.0).clamp(0.0, w - cw);
    let top = (cy - ch / 2.0).clamp(0.0, h - ch);
    [left, top, left + cw, top + ch]
}

/// How much of the patch to use at (x, y), pixels: 1 over the area grown a
/// little, fading to 0 over `fade` pixels outside it, and 0 at the crop's edge.
fn weight(x: f32, y: f32, grown: [f32; 4], fade: f32, crop: [f32; 4]) -> f32 {
    let dx = (grown[0] - x).max(x - grown[2]).max(0.0);
    let dy = (grown[1] - y).max(y - grown[3]).max(0.0);
    let d = (dx * dx + dy * dy).sqrt();
    let t = (1.0 - d / fade).clamp(0.0, 1.0);
    let inside = t * t * (3.0 - 2.0 * t);
    // Never reach the crop's own edge (the patch ends there).
    let edge = (x - crop[0]).min(crop[2] - x).min(y - crop[1]).min(crop[3] - y);
    inside * (edge / 2.0).clamp(0.0, 1.0)
}

/// 0 at the crop's edge rising to 1 two pixels in (the patch ends there).
fn edge_guard(x: f32, y: f32, crop: [f32; 4]) -> f32 {
    let edge = (x - crop[0]).min(crop[2] - x).min(y - crop[1]).min(crop[3] - y);
    (edge / 2.0).clamp(0.0, 1.0)
}

/// The area grown, and the fade width, both in pixels, for a crop and area in pixels.
fn blend_shape(crop: [f32; 4], area: [f32; 4]) -> ([f32; 4], f32) {
    let fade = ((crop[2] - crop[0]).max(crop[3] - crop[1]) * 0.05).max(2.0);
    let (gx, gy) = ((area[2] - area[0]) * 0.12, (area[3] - area[1]) * 0.12);
    ([area[0] - gx, area[1] - gy, area[2] + gx, area[3] + gy], fade)
}

/// Patches, decoded once and kept while they're in use.
/// RGBA: the alpha says where the fill is (see `lite::composite`); patches saved before 21 Nov
/// are RGB and read as opaque, as they were made.
fn patch(file: &str) -> Option<Arc<RgbaImage>> {
    static CACHE: Mutex<Vec<(String, Arc<RgbaImage>)>> = Mutex::new(Vec::new());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = cache.iter().find(|(f, _)| f == file) {
        return Some(p.clone());
    }
    let img = Arc::new(image::open(file).ok()?.to_rgba8());
    cache.push((file.to_string(), img.clone()));
    if cache.len() > 32 {
        cache.remove(0);
    }
    Some(img)
}

/// Bilinear sample of the patch at (u, v), its own pixels: colour 0..255 and alpha 0..1. The
/// colour is weighted by alpha, so a fill's edge doesn't take in the colour beyond it.
fn sample(p: &RgbaImage, u: f32, v: f32) -> ([f32; 3], f32) {
    let (w, h) = (p.width() as f32, p.height() as f32);
    let (u, v) = (u.clamp(0.0, w - 1.0), v.clamp(0.0, h - 1.0));
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(p.width() - 1), (y0 + 1).min(p.height() - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let px = |x, y| p.get_pixel(x, y).0.map(|c| c as f32);
    let ws = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
    let ps = [px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1)];
    let alpha: f32 = ps.iter().zip(ws).map(|(p, w)| p[3] * w).sum();
    if alpha <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    (std::array::from_fn(|k| ps.iter().zip(ws).map(|(p, w)| p[k] * p[3] * w).sum::<f32>() / alpha), alpha / 255.0)
}

/// The scene with its removals blended in; None when there are none (or no
/// patch could be read).
/// Patches and removals in the order they were made, each on the result of those before it: a patch
/// drawn over a removed object patches the removal, and one copying from there copies the removal
/// (before 24 Oct every patch went first, so a removal made earlier covered a later patch). Runs of
/// the same kind go together. `sample` reads the patches' sources (fractions of the whole photo),
/// told the index of the run's first spot (every spot before it is in the picture it should read);
/// without one, they're read from the picture as it stands at that point.
pub fn apply_in_order(
    scene: &crate::pipeline::SceneImage,
    spots: &[Spot],
    frame: Option<&crate::pipeline::Frame>,
    sample: Option<&(dyn Fn(usize, f32, f32) -> [f32; 3] + Sync)>,
) -> Option<crate::pipeline::SceneImage> {
    let mut current: Option<crate::pipeline::SceneImage> = None;
    let mut i = 0;
    while i < spots.len() {
        let is_patch = spots[i].patch().is_some();
        let mut j = i;
        while j < spots.len() && spots[j].patch().is_some() == is_patch {
            j += 1;
        }
        let run = &spots[i..j];
        let base = current.as_ref().unwrap_or(scene);
        let next = if is_patch {
            match sample {
                Some(s) => {
                    let at = move |x: f32, y: f32| s(i, x, y);
                    crate::patch::apply(base, run, frame, &at)
                }
                None => {
                    let (w, h) = (base.width, base.height);
                    let px = &base.px;
                    let s = move |x: f32, y: f32| -> [f32; 3] { crate::commands::bilinear(px, w, h, x * w as f32 - 0.5, y * h as f32 - 0.5) };
                    crate::patch::apply(base, run, frame, &s)
                }
            }
        } else {
            apply(base, run, frame)
        };
        if let Some(n) = next {
            current = Some(n);
        }
        i = j;
    }
    current
}

pub fn apply(scene: &crate::pipeline::SceneImage, spots: &[Spot], frame: Option<&crate::pipeline::Frame>) -> Option<crate::pipeline::SceneImage> {
    let removals: Vec<&Removal> = spots.iter().filter_map(Spot::removal).collect();
    if removals.is_empty() {
        return None;
    }
    let mut out = scene.clone();
    let (w, h) = (scene.width as f32, scene.height as f32);
    // Fractions of the whole photo; a part of it (`frame`) sits within that.
    let (fw, fh, ox, oy) = match frame {
        Some(f) => (f.full_w, f.full_h, f.x0, f.y0),
        None => (w, h, 0.0, 0.0),
    };
    let mut any = false;
    for r in removals {
        let Some(p) = patch(&r.file) else { continue };
        let shaped = p.pixels().any(|q| q[3] < 255);
        any = true;
        let crop = [r.crop[0] * fw - ox, r.crop[1] * fh - oy, r.crop[2] * fw - ox, r.crop[3] * fh - oy];
        let area = [r.area[0] * fw - ox, r.area[1] * fh - oy, r.area[2] * fw - ox, r.area[3] * fh - oy];
        let (grown, fade) = blend_shape(crop, area);
        let gain = r.ev.exp2();
        let (sx, sy) = (p.width() as f32 / (crop[2] - crop[0]), p.height() as f32 / (crop[3] - crop[1]));
        let (y0, y1) = ((crop[1].floor().max(0.0)) as usize, (crop[3].ceil().min(h)) as usize);
        let (x0, x1) = ((crop[0].floor().max(0.0)) as usize, (crop[2].ceil().min(w)) as usize);
        let width = scene.width;
        if y0 >= y1 || x0 >= x1 {
            continue; // outside this part of the photo
        }
        // How much of the patch each pixel of the crop takes.
        let rw = x1 - x0;
        let amount: Vec<f32> = (0..rw * (y1 - y0))
            .into_par_iter()
            .map(|i| {
                let (xf, y) = ((x0 + i % rw) as f32 + 0.5, (y0 + i / rw) as f32 + 0.5);
                // A patch with its own alpha (from 21 Nov) says where the fill goes; the box
                // round the area is for older, opaque ones (it clipped a fill's edge straight).
                let a = if shaped { edge_guard(xf, y, crop) } else { weight(xf, y, grown, fade, crop) };
                if a <= 0.0 {
                    return 0.0;
                }
                a * sample(&p, (xf - crop[0]) * sx - 0.5, (y - crop[1]) * sy - 0.5).1
            })
            .collect();
        out.px[y0 * width..y1 * width].par_chunks_mut(width).enumerate().for_each(|(i, row)| {
            let y = (y0 + i) as f32 + 0.5;
            for (x, px) in row.iter_mut().enumerate().take(x1).skip(x0) {
                let a = amount[i * rw + x - x0];
                if a <= 0.0 {
                    continue;
                }
                let xf = x as f32 + 0.5;
                let (rgb, _) = sample(&p, (xf - crop[0]) * sx - 0.5, (y - crop[1]) * sy - 0.5);
                let new = decode(rgb, gain);
                *px = std::array::from_fn(|k| px[k] * (1.0 - a) + new[k] * a);
            }
        });
        // GPU (Klein) fills carry the photo's own grain already.
        if !r.file.ends_with("-max.png") {
            add_grain(&mut out, [x0, y0, x1, y1], &amount, &r.file);
        }
    }
    any.then_some(out)
}

/// The photo's own grain, put back on a fill. The models paint smooth (RORem at 512 px, LaMa
/// at 512, both enlarged), so on a noisy photo a fill showed as a smooth patch with an edge
/// where the grain stopped (21 Nov, P1368513, a night street). The grain is taken from the
/// photo itself, not made up from its ISO: the fine detail (each pixel less the mean of its
/// 3x3) of flat, untouched 32 px tiles of the crop, so it matches the camera, the ISO, any
/// noise reduction and the size it's shown at. It's copied tile by tile onto the fill, each
/// tile from one of those picked by a hash (the same every render), scaled for brightness
/// (shot noise: by the square root), and by sqrt(a(2-a)) where the fill is blended with the
/// photo (a), so the photo's grain and the copy add up to the same amount across the fade.
fn add_grain(out: &mut crate::pipeline::SceneImage, rect: [usize; 4], amount: &[f32], file: &str) {
    const T: usize = 32;
    let [x0, y0, x1, y1] = rect;
    let (rw, rh) = (x1 - x0, y1 - y0);
    let (w, h) = (out.width, out.height);
    if rw < 3 * T || rh < 3 * T {
        return;
    }
    let lum = |p: [f32; 3]| (p[0] + p[1] + p[2]) / 3.0;
    let img = &out.px;
    // The fine detail at (x, y) of the photo: the pixel less the mean of its 3x3.
    let detail = |x: usize, y: usize| -> [f32; 3] {
        let (xa, xb, ya, yb) = (x.saturating_sub(1), (x + 1).min(w - 1), y.saturating_sub(1), (y + 1).min(h - 1));
        let mut m = [0.0f32; 3];
        for yy in ya..=yb {
            for xx in xa..=xb {
                let q = img[yy * w + xx];
                (0..3).for_each(|k| m[k] += q[k]);
            }
        }
        let n = ((yb - ya + 1) * (xb - xa + 1)) as f32;
        let p = img[y * w + x];
        std::array::from_fn(|k| p[k] - m[k] / n)
    };
    // Untouched tiles (none of the fill in them or next to them), scored by their fine detail
    // for their brightness: the flattest 40% are grain, not the edges of things.
    let tiles: Vec<(usize, usize)> = (0..rh / T).flat_map(|ty| (0..rw / T).map(move |tx| (tx, ty))).collect();
    let mut flat: Vec<(f32, usize, usize)> = tiles
        .par_iter()
        .filter_map(|&(tx, ty)| {
            let (ox, oy) = (tx * T, ty * T);
            let (ya, yb, xa, xb) = (oy.saturating_sub(1), (oy + T + 1).min(rh), ox.saturating_sub(1), (ox + T + 1).min(rw));
            if (ya..yb).any(|y| (xa..xb).any(|x| amount[y * rw + x] > 0.0)) {
                return None;
            }
            let (mut d, mut l) = (0.0f32, 0.0f32);
            for y in oy..oy + T {
                for x in ox..ox + T {
                    let (ax, ay) = (x0 + x, y0 + y);
                    d += lum(detail(ax, ay)).abs();
                    l += lum(img[ay * w + ax]);
                }
            }
            (l > 0.0).then(|| (d / l.sqrt(), x0 + ox, y0 + oy))
        })
        .collect();
    if flat.len() < 4 {
        return; // too little of the photo round the fill to take grain from
    }
    flat.sort_by(|a, b| a.0.total_cmp(&b.0));
    flat.truncate((flat.len() * 2 / 5).max(4));
    // Each source tile's detail and smoothed brightness, copied out (the fill is written below).
    let sources: Vec<(Vec<[f32; 3]>, Vec<f32>)> = flat
        .par_iter()
        .map(|&(_, sx, sy)| {
            let mut g = Vec::with_capacity(T * T);
            let mut b = Vec::with_capacity(T * T);
            for y in sy..sy + T {
                for x in sx..sx + T {
                    let d = detail(x, y);
                    g.push(d);
                    b.push(lum(img[y * w + x]) - lum(d));
                }
            }
            (g, b)
        })
        .collect();
    let seed = file.bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193));
    let pick = |tx: usize, ty: usize| -> usize {
        let mut v = seed ^ (tx as u32).wrapping_mul(0x9e3779b1) ^ (ty as u32).wrapping_mul(0x85ebca77);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        v as usize % sources.len()
    };
    out.px[y0 * w..y1 * w].par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        for x in x0..x1 {
            let a = amount[i * rw + x - x0];
            if a <= 0.0 {
                continue;
            }
            let (lx, ly) = (x - x0, i);
            let (g, base) = &sources[pick(lx / T, ly / T)];
            let j = (ly % T) * T + lx % T;
            let here = lum(row[x]);
            let scale = if base[j] > 1e-6 && here > 0.0 { (here / base[j]).sqrt().clamp(0.25, 4.0) } else { 0.0 };
            let k = (a * (2.0 - a)).sqrt() * scale;
            row[x] = std::array::from_fn(|c| (row[x][c] + g[j][c] * k).max(0.0));
        }
    });
}

/// Where a photo's removal patches go: `sidecars/removals/` beside it.
fn folder(photo: &Path) -> Result<PathBuf, String> {
    let dir = photo.parent().ok_or("no folder")?.join("sidecars").join("removals");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// The instruction for Klein. Describing the physical object (and its
/// fixings) and the background behind it works better than naming it by any
/// words printed on it, which Klein can take as "remove the lettering".
/// Klein otherwise sometimes fills a bigger space with something new (a
/// plant, a person, a sign), so it's told plainly not to add anything.
const NOTHING_NEW: &str = "Do not add any new objects, people, plants, signs or text: only continue the background around it.";

fn prompt(what: &str) -> String {
    let what = what.trim().trim_end_matches('.');
    if what.is_empty() {
        // No words: the area has been greyed out (`boxed`), so the grey shape
        // is the thing to remove.
        return format!(
            "Remove the flat grey shape from the image. Show the plain background behind it, matching the surroundings. {NOTHING_NEW} Keep everything else exactly the same."
        );
    }
    format!(
        "Remove the whole {what} inside the red rectangle, including anything attached to it, and remove the red rectangle. {} {NOTHING_NEW} {}",
        "Fill the space with the plain background behind it, matching the surroundings.",
        "Keep everything else exactly the same."
    )
}

/// The model's working size (multiples of 16, long edge SIDE) for a w x h picture.
fn model_size(w: f32, h: f32) -> (u32, u32) {
    model_size_side(w, h, SIDE)
}

/// `model_size` for another long edge.
fn model_size_side(w: f32, h: f32, side: u32) -> (u32, u32) {
    let s = side as f32 / w.max(h);
    (((w * s / 16.0).round() * 16.0).max(16.0) as u32, ((h * s / 16.0).round() * 16.0).max(16.0) as u32)
}

/// The hole in the model's picture (mw x mh) of `crop`: the outline's pixels, or without one the
/// area's box grown 12%.
fn hole_in(mw: u32, mh: u32, outline: Option<&image::GrayImage>, crop: [f32; 4], area_px: [f32; 4], w: f32, h: f32) -> Vec<bool> {
    match outline {
        Some(mask) => outline_hole(mw, mh, mask, crop, w, h),
        None => {
            let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
            let (sx, sy) = (mw as f32 / cw, mh as f32 / ch);
            let (aw, ah) = (area_px[2] - area_px[0], area_px[3] - area_px[1]);
            let g = [
                (area_px[0] - aw * 0.12 - crop[0]) * sx,
                (area_px[1] - ah * 0.12 - crop[1]) * sy,
                (area_px[2] + aw * 0.12 - crop[0]) * sx,
                (area_px[3] + ah * 0.12 - crop[1]) * sy,
            ];
            (0..(mw * mh) as usize)
                .map(|i| {
                    let (x, y) = ((i % mw as usize) as f32 + 0.5, (i / mw as usize) as f32 + 0.5);
                    x >= g[0] && x <= g[2] && y >= g[1] && y <= g[3]
                })
                .collect()
        }
    }
}

/// How much structure there is round the hole, in `pic` (8-bit): the mean absolute difference
/// between a 5x5 and a 21x21 neighbourhood's mean (structure, with the grain averaged out, so a
/// dark noisy backdrop still reads plain), over a ring from 1% to 6% of the long edge outside the
/// hole, as a share of the ring's brightness. A plain studio backdrop or clear sky reads near 0;
/// grass, paving or foliage far higher (see `plain_backdrop`).
pub(crate) fn ring_detail(pic: &RgbImage, hole: &[bool]) -> f32 {
    let (w, h) = (pic.width() as usize, pic.height() as usize);
    let long = w.max(h) as f32;
    let outer = dilate(hole, w, h, (long * 0.06).ceil() as usize);
    let inner = dilate(hole, w, h, (long * 0.01).ceil() as usize + 1);
    let lum: Vec<f32> = pic.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
    let mut ii = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += lum[y * w + x] as f64;
            ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
        }
    }
    let mean = |x: usize, y: usize, r: usize| -> f64 {
        let (x0, y0, x1, y1) = (x.saturating_sub(r), y.saturating_sub(r), (x + r + 1).min(w), (y + r + 1).min(h));
        (ii[y1 * (w + 1) + x1] - ii[y0 * (w + 1) + x1] - ii[y1 * (w + 1) + x0] + ii[y0 * (w + 1) + x0]) / ((x1 - x0) * (y1 - y0)) as f64
    };
    let (mut dsum, mut lsum, mut n) = (0f64, 0f64, 0f64);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !outer[i] || inner[i] {
                continue;
            }
            dsum += (mean(x, y, 2) - mean(x, y, 10)).abs();
            lsum += lum[i] as f64;
            n += 1.0;
        }
    }
    if n < 1.0 || lsum <= 0.0 {
        return f32::MAX;
    }
    (dsum / lsum) as f32
}

/// Below this `ring_detail` the background round the hole is a plain backdrop (21 Nov: studio
/// light 0.0007, studio dark 0.0028; bluebells 0.020, moor 0.037, night street 0.086). Was 0.008
/// until 24 Nov: a windsurfer's sail against a soft out-of-focus sea (_DSC3995) read 0.0075 and was
/// smeared by the smooth fill instead of going to the AI; 0.0045 sits about 1.6x from both.
const PLAIN: f32 = 0.0045;

/// A plain backdrop filled with no model: its light carried smoothly in from all round the hole
/// (pull-push: each coarser level the mean of what's known, then filled back down, then smoothed),
/// so a studio backdrop's gradient continues. The photo's grain is added when the patch is applied
/// (`add_grain`). Every AI fill failed here: none would empty the frame of a studio subject (Klein
/// kept the grey shape or the person, LaMa smeared a column of hair; Chris, 21 Nov).
fn smooth_fill(pic: &RgbImage, hole: &[bool]) -> RgbImage {
    let (w, h) = (pic.width() as usize, pic.height() as usize);
    let mut img: Vec<[f32; 3]> = pic.pixels().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    let known: Vec<f32> = hole.iter().map(|&b| if b { 0.0 } else { 1.0 }).collect();
    fn pull_push(img: &[[f32; 3]], known: &[f32], w: usize, h: usize) -> Vec<[f32; 3]> {
        if w <= 2 || h <= 2 {
            let (mut s, mut n) = ([0f32; 3], 0f32);
            for (p, &k) in img.iter().zip(known) {
                (0..3).for_each(|c| s[c] += p[c] * k);
                n += k;
            }
            let m = if n > 0.0 { s.map(|v| v / n) } else { [128.0; 3] };
            return vec![m; w * h];
        }
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut ci = vec![[0f32; 3]; cw * ch];
        let mut ck = vec![0f32; cw * ch];
        for y in 0..h {
            for x in 0..w {
                let (i, j) = (y * w + x, (y / 2) * cw + x / 2);
                (0..3).for_each(|c| ci[j][c] += img[i][c] * known[i]);
                ck[j] += known[i];
            }
        }
        for j in 0..cw * ch {
            if ck[j] > 0.0 {
                ci[j] = ci[j].map(|v| v / ck[j]);
            }
            ck[j] = ck[j].min(1.0);
        }
        let coarse = pull_push(&ci, &ck, cw, ch);
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                // Bilinear from the coarser level.
                let (fx, fy) = ((x as f32 + 0.5) / 2.0 - 0.5, (y as f32 + 0.5) / 2.0 - 0.5);
                let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
                let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
                let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
                let up: [f32; 3] = std::array::from_fn(|c| {
                    let a = coarse[y0 * cw + x0][c] * (1.0 - tx) + coarse[y0 * cw + x1][c] * tx;
                    let b = coarse[y1 * cw + x0][c] * (1.0 - tx) + coarse[y1 * cw + x1][c] * tx;
                    a * (1.0 - ty) + b * ty
                });
                let k = known[i];
                std::array::from_fn(|c| img[i][c] * k + up[c] * (1.0 - k))
            })
            .collect()
    }
    img = pull_push(&img, &known, w, h);
    // Smoothed inside the hole (a box of 1.5% of the long edge, a few passes), the rest as shot.
    let r = ((w.max(h) as f32) * 0.015).ceil() as usize;
    for _ in 0..3 {
        let prev = img.clone();
        let blurred: Vec<[f32; 3]> = {
            let pass = |src: &[[f32; 3]], horiz: bool| -> Vec<[f32; 3]> {
                let (n, m) = if horiz { (h, w) } else { (w, h) };
                let mut out = vec![[0f32; 3]; w * h];
                for line in 0..n {
                    let at = |k: usize| if horiz { line * w + k } else { k * w + line };
                    let mut pre = vec![[0f64; 3]; m + 1];
                    for k in 0..m {
                        pre[k + 1] = std::array::from_fn(|c| pre[k][c] + src[at(k)][c] as f64);
                    }
                    for k in 0..m {
                        let (a, b) = (k.saturating_sub(r), (k + r + 1).min(m));
                        out[at(k)] = std::array::from_fn(|c| ((pre[b][c] - pre[a][c]) / (b - a) as f64) as f32);
                    }
                }
                out
            };
            pass(&pass(&prev, true), false)
        };
        for i in 0..w * h {
            if hole[i] {
                img[i] = blurred[i];
            }
        }
    }
    RgbImage::from_fn(w as u32, h as u32, |x, y| Rgb(img[y as usize * w + x as usize].map(|v| v.round().clamp(0.0, 255.0) as u8)))
}

/// GPU's fill of `crop`, blended (RGBA patch), at `gpumax::work_size` (1408 px, area capped). No
/// retrying smaller after running out of memory: a WebGPU failure leaves ONNX Runtime's graphics
/// device unusable until RapidOne restarts (21 Nov: 1408, 1024 and 768 all failed in turn), so the
/// size is chosen up front. The hole is grown 2% so Klein doesn't see the object's edges; people the
/// outline protects are never greyed.
#[allow(clippy::too_many_arguments)]
fn max_fill(img: &crate::pipeline::SceneImage, crop: [f32; 4], ev: f32, outline: Option<&image::GrayImage>, area_px: [f32; 4], w: f32, h: f32, seed: u32) -> Result<RgbaImage, String> {
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let (mw, mh) = crate::gpumax::work_size(cw, ch);
    let small = picture(img, crop, mw, mh, ev.exp2());
    let hole = hole_in(mw, mh, outline, crop, area_px, w, h);
    let protect = outline.map(|m| outline_where(mw, mh, m, crop, w, h, |v| v == crate::lite::PROTECT)).filter(|p| p.iter().any(|&b| b));
    let grow = ((mw.max(mh) as f32) * 0.02).ceil() as usize + 2;
    let mut grown = dilate(&hole, mw as usize, mh as usize, grow);
    if let Some(p) = &protect {
        grown.iter_mut().zip(p).for_each(|(g, &p)| *g &= !p);
    }
    let out = crate::gpumax::fill(&small, &grown, seed)?;
    Ok(crate::gpumax::composite_changed(&small, &out, &grown, protect.as_deref()))
}

/// The part of the scene in `crop` (pixels) as an ordinary picture (the scene
/// light brightened by `gain`, sRGB) at mw x mh.
fn picture(img: &crate::pipeline::SceneImage, crop: [f32; 4], mw: u32, mh: u32, gain: f32) -> RgbImage {
    let (x0, y0) = (crop[0] as u32, crop[1] as u32);
    let (pw, ph) = (((crop[2] - crop[0]) as u32).max(1), ((crop[3] - crop[1]) as u32).max(1));
    let mut full = RgbImage::new(pw, ph);
    full.enumerate_rows_mut().par_bridge().for_each(|(_, row)| {
        for (x, y, p) in row {
            let i = (y0 + y) as usize * img.width + (x0 + x) as usize;
            *p = Rgb(encode(img.px[i.min(img.px.len() - 1)], gain));
        }
    });
    if (pw, ph) == (mw, mh) {
        full
    } else {
        image::imageops::resize(&full, mw, mh, image::imageops::FilterType::Lanczos3)
    }
}

/// Files for one model run: (input, output) in the photo's removals folder.
fn run_files(photo: &Path) -> Result<(PathBuf, PathBuf), String> {
    let dir = folder(photo)?;
    let stem = photo.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Ok((dir.join(format!("{stem}-{id}-input.png")), dir.join(format!("{stem}-{id}.png"))))
}

/// The photo's normal brightness, in stops, for the model's picture.
fn brightness(scene: &crate::develop::Scene) -> f32 {
    crate::pipeline::Profile::ShinobiColour.baseline_for(&scene.make, &scene.camera) + scene.exposure_offset
}

/// Removes what's in `area` (fractions of the upright, uncropped photo) from
/// the photo, returning the removal to add to its edit. The whole photo as
/// the area (Ctrl+E) means "find it from `what`": see `find_and_remove`.
/// `seed` picks the model's version (0 first; Try again asks for the next).
pub fn create(photo: &Path, area: [f32; 4], what: &str, outline: Option<&Path>, seed: u32) -> Result<Removal, String> {
    let whole = area[0] <= 0.001 && area[1] <= 0.001 && area[2] >= 0.999 && area[3] >= 0.999;
    if whole && outline.is_none() && lite_tools() {
        return Err(LITE_WORDS.into());
    }
    let scene = crate::develop::develop_scene_export(photo, 100_000)?;
    if whole && outline.is_none() {
        find_and_remove(photo, &scene, what, seed)
    } else {
        boxed(photo, &scene, area, what, outline, seed)
    }
}

/// Brush: removes what was painted. `painted` is a white-on-black PNG (any
/// size, covering the whole upright, uncropped photo); the painted shape is
/// exactly what's greyed for Klein, so no words are needed.
pub fn remove_painted(photo: &Path, painted_png: &[u8], what: &str, seed: u32) -> Result<Removal, String> {
    let mask = image::load_from_memory(painted_png).map_err(|e| e.to_string())?.to_luma8();
    let (w, h) = (mask.width() as f32, mask.height() as f32);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, p) in mask.enumerate_pixels() {
        if p.0[0] > 127 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return Err("Paint over what to remove first.".into());
    }
    let dir = folder(photo)?;
    let (_, output) = run_files(photo)?;
    let file = dir.join(format!("{}-painted.png", output.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
    mask.save(&file).map_err(|e| e.to_string())?;
    let area = [x0 as f32 / w, y0 as f32 / h, (x1 + 1) as f32 / w, (y1 + 1) as f32 / h];
    create(photo, area, what, Some(&file), seed)
}

/// An object found by SAM 3 (`find`), for the Remove tool to show and pick
/// from: its area (fractions of the upright, uncropped photo), its outline
/// file (handed back to `create`), confidence, and a picture of the outline
/// (a cyan-tinted PNG data URL, ~1024 px) to lay over the photo.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Outline {
    pub area: [f32; 4],
    pub outline: String,
    pub score: f32,
    pub overlay: String,
}

/// The removers (the Remove bar's Large / Medium / Lite switch):
/// - Large: FLUX.2 Klein 4B (an editing model told to remove), SAM 3 masks;
/// - Medium: RORem (an SDXL inpainter trained only to remove; mask in, no
///   words), SAM 3 masks - both need a graphics card and Python;
/// - Lite: LaMa + MobileSAM + Grounding DINO on the CPU (`lite.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Large,
    Medium,
    Lite,
}

/// The one chosen; the best installed when none is.
static MODE_CHOSEN: Mutex<Option<Mode>> = Mutex::new(None);

const LITE_WORDS: &str = "Finding things by name needs Grounding DINO (Lite) or SAM 3. Draw a box round it, or paint over it.";

/// Whether RORem may be used: off since 21 Nov (Chris: GPU is Klein 4B, `gpumax`; RORem's code is kept).
/// RAPIDONE_ROREM=1 turns it back on (testing).
pub fn rorem_enabled() -> bool {
    std::env::var("RAPIDONE_ROREM").as_deref() == Ok("1")
}

/// Which removers are installed: (Large, Medium, Lite). Medium is GPU (Smarter): Klein 4B.
pub fn installed() -> (bool, bool, bool) {
    let rorem = rorem_enabled() && (crate::rorem::available() || crate::klein::rorem_available());
    (crate::klein::available(), crate::gpumax::available() || rorem, crate::lite::available())
}

/// The remover in use: the one chosen if installed, else Lite (it ships with
/// the app; Chris, 17 Oct), else Medium, else Large.
pub fn mode() -> Mode {
    let (large, medium, lite) = installed();
    let ok = |m: Mode| match m {
        Mode::Large => large,
        Mode::Medium => medium,
        Mode::Lite => lite,
    };
    let chosen = *MODE_CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    chosen.filter(|&m| ok(m)).or_else(|| [Mode::Lite, Mode::Medium, Mode::Large].into_iter().find(|&m| ok(m))).unwrap_or(Mode::Lite)
}

/// The Remove bar's switch (CPU = Lite, GPU = Medium). Leaving GPU frees
/// RORem's graphics memory (~3.6 GB at 1024 px) for Denoise, masks and the renderer.
pub fn set_mode(m: Mode) {
    *MODE_CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(m);
    eprintln!("removal: mode {m:?}");
    if m != Mode::Medium {
        crate::rorem::unload();
        crate::gpumax::unload();
    } else if rorem_enabled() && crate::rorem::held() {
        crate::rorem::preload();
    }
}

/// Whether Lite is the remover in use.
pub fn lite() -> bool {
    mode() == Mode::Lite
}

/// Whether outlines and finding by words use Lite's ONNX tools (MobileSAM,
/// Grounding DINO): in Lite ("CPU"), and in Medium ("GPU") when RORem runs
/// through ONNX Runtime, which needs no Python and so has no SAM 3.
fn lite_tools() -> bool {
    lite() || (mode() == Mode::Medium && (crate::gpumax::available() || (rorem_enabled() && crate::rorem::available())))
}

/// Lite: MobileSAM's outline of what's in `bbox` (pixels of the find
/// picture), written beside the photo's removals: (file, its bounding box).
fn lite_outline(photo: &Path, picture: &Path, bbox: [f32; 4], n: usize) -> Result<Option<(PathBuf, [f32; 4])>, String> {
    let Some((mask, bb)) = crate::lite::outline(picture, bbox)? else { return Ok(None) };
    let (run, _) = run_files(photo)?;
    let file = folder(photo)?.join(format!("{}-mask{n}.png", run.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
    mask.save(&file).map_err(|e| e.to_string())?;
    Ok(Some((file, bb)))
}

/// Long edge of the picture SAM 3 is given.
const FIND_SIDE: usize = 2048;

/// The photo as SAM 3 sees it (FIND_SIDE long edge), made once per photo and
/// kept beside its removals as `<stem>-find.jpg`: (photo, picture, width,
/// height). The worker keeps SAM 3's analysis of it too, so after `prepare`
/// an outline only costs the box itself.
static PREPARED: Mutex<Option<(PathBuf, PathBuf, f32, f32)>> = Mutex::new(None);

fn find_picture(photo: &Path) -> Result<(PathBuf, f32, f32), String> {
    let mut held = PREPARED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, pic, w, h)) = held.as_ref() {
        if p == photo && pic.is_file() {
            return Ok((pic.clone(), *w, *h));
        }
    }
    let scene = crate::develop::develop_scene(photo, FIND_SIDE)?;
    let img = &scene.image;
    let (w, h) = (img.width as f32, img.height as f32);
    let pic = picture(img, [0.0, 0.0, w, h], img.width as u32, img.height as u32, brightness(&scene).exp2());
    let stem = photo.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let file = folder(photo)?.join(format!("{stem}-find.jpg"));
    pic.save(&file).map_err(|e| e.to_string())?;
    *held = Some((photo.to_path_buf(), file.clone(), w, h));
    Ok((file, w, h))
}

/// Gets SAM 3 ready for this photo (the Remove tool opening on it), so
/// drawing a box outlines what's in it almost at once.
pub fn prepare(photo: &Path) -> Result<(), String> {
    if !can_find() {
        if lite() {
            // No MobileSAM: still load LaMa ahead of the first removal.
            let _ = crate::lite::available() && crate::lite::fill(&RgbImage::new(8, 8), &[false; 64], None, 0).is_ok();
        }
        return Ok(());
    }
    let (pic, _, _) = find_picture(photo)?;
    if lite_tools() {
        return crate::lite::prepare(&pic);
    }
    crate::klein::prepare(&pic)
}

/// Whether drawn boxes are outlined first (SAM 3; Lite: MobileSAM), and with
/// Full, objects found by words.
pub fn can_find() -> bool {
    if lite_tools() {
        crate::lite::can_outline()
    } else {
        crate::klein::sam_available()
    }
}

/// SAM 3: every object in the photo matching `words`, most confident first;
/// or, given `area` (fractions of the upright, uncropped photo) and no words,
/// the object in that box - shown before it's removed.
pub fn find(photo: &Path, words: &str, area: Option<[f32; 4]>) -> Result<Vec<Outline>, String> {
    let words = words.trim();
    if words.is_empty() && area.is_none() {
        return Err("Say what to find.".into());
    }
    let (input, w, h) = find_picture(photo)?;
    let bbox = area.map(|a| [a[0] * w, a[1] * h, a[2] * w, a[3] * h]);
    if lite_tools() {
        // Lite: a drawn box outlined by MobileSAM; words found by Grounding
        // DINO, each match then outlined by MobileSAM.
        let boxes: Vec<([f32; 4], f32)> = match bbox {
            Some(b) => vec![(b, 1.0)],
            None if crate::lite::can_find_words() => crate::lite::find_words(&input, words)?,
            None => return Err(LITE_WORDS.into()),
        };
        let mut out = Vec::new();
        for (i, (b, score)) in boxes.into_iter().enumerate() {
            if let Some((file, bb)) = lite_outline(photo, &input, b, i)? {
                out.push(Outline {
                    area: [bb[0] / w, bb[1] / h, bb[2] / w, bb[3] / h],
                    overlay: overlay(&file)?,
                    outline: file.to_string_lossy().replace('\\', "/"),
                    score,
                });
            }
        }
        return Ok(out);
    }
    let dir = folder(photo)?;
    let (run, _) = run_files(photo)?;
    let stem = run.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let found = crate::klein::find(&input, (!words.is_empty()).then_some(words), bbox, &dir, &stem);
    found?
        .into_iter()
        .map(|f| {
            Ok(Outline {
                area: [f.bbox[0] / w, f.bbox[1] / h, f.bbox[2] / w, f.bbox[3] / h],
                overlay: overlay(&f.mask)?,
                outline: f.mask.to_string_lossy().replace('\\', "/"),
                score: f.score,
            })
        })
        .collect()
}

/// An outline as a cyan, see-through PNG data URL (long edge 1024).
fn overlay(mask: &Path) -> Result<String, String> {
    use base64::Engine;
    let m = image::open(mask).map_err(|e| e.to_string())?.to_luma8();
    let s = 1024.0 / m.width().max(m.height()) as f32;
    let small = image::imageops::resize(&m, ((m.width() as f32 * s) as u32).max(1), ((m.height() as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle);
    let mut rgba = image::RgbaImage::new(small.width(), small.height());
    for (x, y, p) in small.enumerate_pixels() {
        rgba.put_pixel(x, y, image::Rgba([62, 230, 255, (p.0[0] as f32 * 0.45) as u8]));
    }
    let mut png = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// SAM 3 with a box (pixels of the full scene): the outline of the object it's
/// round, written beside the photo's removals; None if SAM 3 isn't set up or
/// finds nothing there.
fn outline_in_box(photo: &Path, scene: &crate::develop::Scene, area_px: [f32; 4]) -> Option<PathBuf> {
    if !can_find() {
        return None;
    }
    let (input, pw, _) = find_picture(photo).ok()?;
    let s = pw / scene.image.width as f32;
    if lite_tools() {
        let b = [area_px[0] * s, area_px[1] * s, area_px[2] * s, area_px[3] * s];
        return lite_outline(photo, &input, b, 0).ok().flatten().map(|(f, _)| f);
    }
    let dir = folder(photo).ok()?;
    let (run, _) = run_files(photo).ok()?;
    let stem = run.file_stem()?.to_string_lossy().to_string();
    let b = [area_px[0] * s, area_px[1] * s, area_px[2] * s, area_px[3] * s];
    let found = crate::klein::find(&input, None, Some(b), &dir, &stem);
    found.ok()?.into_iter().next().map(|f| f.mask)
}

/// Removal of what's in a box: a crop around it, at full resolution, with
/// the box marked in red.
fn boxed(photo: &Path, scene: &crate::develop::Scene, area: [f32; 4], what: &str, outline: Option<&Path>, seed: u32) -> Result<Removal, String> {
    let img = &scene.image;
    let (w, h) = (img.width as f32, img.height as f32);
    let area_px = [area[0] * w, area[1] * h, area[2] * w, area[3] * h];
    if area_px[2] - area_px[0] < 2.0 || area_px[3] - area_px[1] < 2.0 {
        return Err("Draw a box around what to remove first.".into());
    }
    let crop = crop_around(area_px, w, h);
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let (mw, mh) = model_size(cw, ch);
    let ev = brightness(scene);
    let mut small = picture(img, crop, mw, mh, ev.exp2());

    let (sx, sy) = (mw as f32 / cw, mh as f32 / ch);
    let (aw, ah) = (area_px[2] - area_px[0], area_px[3] - area_px[1]);
    // No words: the area (grown 12%) is greyed out and Klein asked to remove
    // "the flat grey shape" - unambiguous, where a bare box isn't (it took a
    // sign's lettering or a no-entry bar for the object). 3/3 on 16 Oct
    // (Stable Block plaque, no-entry sign, register-office sign and posts).
    // Asking it to "fill" the grey left it grey: Klein keeps what it's shown.
    // With no words, SAM 3 (when set up) outlines the object in the box, so
    // only the object is greyed and everything else in the box stays visible
    // (a plant pot, railings): better than greying the whole box (16 Oct).
    let found_outline = if outline.is_none() && what.trim().is_empty() { outline_in_box(photo, scene, area_px) } else { None };
    let outline = outline.map(Path::to_path_buf).or(found_outline);
    if lite() || mode() == Mode::Medium {
        // LaMa or RORem fills the outline (or, without one, the box grown 12%).
        let outline_img = outline.as_ref().and_then(|m| image::open(m).ok()).map(|m| m.to_luma8());
        // Lite's outlines mark people next to the object, which the fill keeps.
        let protect = outline_img.as_ref().map(|m| outline_where(mw, mh, m, crop, w, h, |v| v == crate::lite::PROTECT)).filter(|p| p.iter().any(|&b| b));
        let _ = (sx, sy, aw, ah);
        let hole = hole_in(mw, mh, outline_img.as_ref(), crop, area_px, w, h);
        let (input, output) = run_files(photo)?;
        // A plain backdrop round the hole (studio, clear sky): filled with no model, CPU or GPU.
        let detail = ring_detail(&small, &hole);
        eprintln!("removal: detail round the hole {detail:.4}{}", if detail < PLAIN { " (plain backdrop: smooth fill)" } else { "" });
        if detail < PLAIN && std::env::var("RAPIDONE_NO_PLAIN").is_err() {
            let long = mw.max(mh) as f32;
            let grow = (long * 0.02).ceil() as usize + 2;
            let mut core = hole.clone();
            let mut grown = dilate(&hole, mw as usize, mh as usize, grow);
            if let Some(p) = &protect {
                grown.iter_mut().zip(p).for_each(|(g, &p)| *g &= !p);
                core.iter_mut().zip(p).for_each(|(g, &p)| *g &= !p);
            }
            let out = smooth_fill(&small, &grown);
            let filled = crate::lite::composite_ramp(&small, &out, &core, protect.as_deref(), long * 0.02 + 2.0, long * 0.04 + 2.0);
            filled.save(&output).map_err(|e| e.to_string())?;
            if let Some(m) = outline.as_ref() {
                let _ = std::fs::remove_file(m);
            }
            let _ = &input;
            return Ok(Removal { file: output.to_string_lossy().replace('\\', "/"), crop: [crop[0] / w, crop[1] / h, crop[2] / w, crop[3] / h], area, ev });
        }
        // GPU (21 Nov): FLUX.2 Klein 4B + remove LoRA in this process (`gpumax`), when installed;
        // RORem below is kept but no longer offered (Chris). Its own size (1408 px), its own blend
        // (`gpumax::composite_changed`); patches named "-max" (`apply` adds no grain to them).
        if !lite() && crate::gpumax::available() {
            let filled = max_fill(img, crop, ev, outline_img.as_ref(), area_px, w, h, seed)?;
            let stem = output.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let output = output.with_file_name(format!("{stem}-max.png"));
            filled.save(&output).map_err(|e| e.to_string())?;
            if let Some(m) = outline.as_ref() {
                let _ = std::fs::remove_file(m);
            }
            return Ok(Removal { file: output.to_string_lossy().replace('\\', "/"), crop: [crop[0] / w, crop[1] / h, crop[2] / w, crop[3] / h], area, ev });
        }
        let filled = if lite() {
            crate::lite::fill(&small, &hole, protect.as_deref(), seed)?
        } else if !rorem_enabled() {
            return Err("GPU (Smarter) isn't installed on this computer. Switch Remove to CPU.".into());
        } else if crate::rorem::available() {
            // GPU: RORem through ONNX Runtime, in this process. The hole grown 3.5%
            // of the crop (RAPIDONE_ROREM_GROW, percent, for tests). 20 Nov, P1368513
            // (a couple removed from a night street): at 2% the edges of their dark
            // suits stayed outside the hole and RORem carried them in as a dark ghost
            // with a seam along the outline; at 5% it caught the bollard beside them
            // (half of it went) and left a light streak; 3.5% kept the bollard whole
            // and the ghost mostly gone. Blended over half the growth: no hard edge.
            // 21 Nov: blended by distance from the object, fully out to 2% (the suits' edges)
            // and fading to nothing at 5% (`lite::composite_ramp`), not over the grown hole's
            // edge, where the alpha jumped and left a line on the floor. RAPIDONE_ROREM_FULL /
            // _ZERO (percent) for tests.
            let env = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
            let pct = env("RAPIDONE_ROREM_GROW", 3.5);
            let long = mw.max(mh) as f32;
            let grow = (long * pct / 100.0).ceil() as usize + 2;
            let mut core = hole.clone();
            let mut hole = dilate(&hole, mw as usize, mh as usize, grow);
            if let Some(p) = &protect {
                hole.iter_mut().zip(p).for_each(|(h, &p)| *h &= !p);
                core.iter_mut().zip(p).for_each(|(h, &p)| *h &= !p);
            }
            let out = crate::rorem::fill(&small, &hole, seed, |_, _| {})?;
            let full = long * env("RAPIDONE_ROREM_FULL", 2.0) / 100.0 + 2.0;
            let zero = long * env("RAPIDONE_ROREM_ZERO", 5.0) / 100.0 + 2.0;
            crate::lite::composite_ramp(&small, &out, &core, protect.as_deref(), full, zero)
        } else {
            // RORem: the hole grown ~2% (edges, shadows), as a mask beside the picture.
            let grow = ((mw.max(mh) as f32) * 0.02).ceil() as usize + 2;
            let mut hole = dilate(&hole, mw as usize, mh as usize, grow);
            if let Some(p) = &protect {
                hole.iter_mut().zip(p).for_each(|(h, &p)| *h &= !p);
            }
            let mask_file = input.with_extension("mask.png");
            image::GrayImage::from_fn(mw, mh, |x, y| image::Luma([if hole[(y * mw + x) as usize] { 255 } else { 0 }]))
                .save(&mask_file)
                .map_err(|e| e.to_string())?;
            small.save(&input).map_err(|e| e.to_string())?;
            let r = crate::klein::rorem(&input, &mask_file, &output, mw, mh, seed);
            let _ = std::fs::remove_file(&input);
            let _ = std::fs::remove_file(&mask_file);
            r?;
            let out = image::open(&output).map_err(|e| e.to_string())?.to_rgb8();
            let out = if out.dimensions() == (mw, mh) { out } else { image::imageops::resize(&out, mw, mh, image::imageops::FilterType::CatmullRom) };
            crate::lite::composite(&small, &out, &hole, (grow / 2).max(2))
        };
        filled.save(&output).map_err(|e| e.to_string())?;
        if let Some(m) = outline.as_ref() {
            let _ = std::fs::remove_file(m);
        }
        return Ok(Removal { file: output.to_string_lossy().replace('\\', "/"), crop: [crop[0] / w, crop[1] / h, crop[2] / w, crop[3] / h], area, ev });
    }
    if let Some(mask) = outline.as_ref().and_then(|m| image::open(m).ok()).map(|m| m.to_luma8()) {
        grey_outline(&mut small, &mask, crop, w, h);
        let (input, output) = run_files(photo)?;
        small.save(&input).map_err(|e| e.to_string())?;
        let result = crate::klein::edit(&input, &output, &prompt(""), mw, mh, seed);
        let _ = std::fs::remove_file(&input);
        if let Some(m) = outline.as_ref() {
            let _ = std::fs::remove_file(m);
        }
        result?;
        return Ok(Removal { file: output.to_string_lossy().replace('\\', "/"), crop: [crop[0] / w, crop[1] / h, crop[2] / w, crop[3] / h], area, ev });
    }
    if what.trim().is_empty() {
        let grey = [
            (area_px[0] - aw * 0.12 - crop[0]) * sx,
            (area_px[1] - ah * 0.12 - crop[1]) * sy,
            (area_px[2] + aw * 0.12 - crop[0]) * sx,
            (area_px[3] + ah * 0.12 - crop[1]) * sy,
        ];
        for (x, y, p) in small.enumerate_pixels_mut() {
            let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
            if xf >= grey[0] && xf <= grey[2] && yf >= grey[1] && yf <= grey[3] {
                *p = Rgb([128, 128, 128]);
            }
        }
    } else {
        mark_red(&mut small, area_px, crop, sx, sy, aw, ah, mw, mh);
    }

    let (input, output) = run_files(photo)?;
    small.save(&input).map_err(|e| e.to_string())?;
    let result = crate::klein::edit(&input, &output, &prompt(what), mw, mh, seed);
    let _ = std::fs::remove_file(&input);
    result?;
    Ok(Removal {
        file: output.to_string_lossy().replace('\\', "/"),
        crop: [crop[0] / w, crop[1] / h, crop[2] / w, crop[3] / h],
        area,
        ev,
    })
}

/// Greys the object's outline (`mask`, any size, covering the whole photo)
/// in the model's picture of `crop` (pixels of a w x h photo), grown about
/// 1.2% so its edge is covered too.
fn grey_outline(small: &mut RgbImage, mask: &image::GrayImage, crop: [f32; 4], w: f32, h: f32) {
    let (mw, mh) = (small.width() as usize, small.height() as usize);
    let inside = outline_hole(mw as u32, mh as u32, mask, crop, w, h);
    let grow = ((mw.max(mh) as f32) * 0.012).ceil().max(3.0) as usize;
    let grown = dilate(&inside, mw, mh, grow);
    for (i, p) in small.pixels_mut().enumerate() {
        if grown[i] {
            *p = Rgb([128, 128, 128]);
        }
    }
}

/// Which pixels of the model's mw x mh picture of `crop` (pixels of a w x h
/// photo) are inside the object's outline (`mask`, any size, whole photo).
fn outline_hole(mw: u32, mh: u32, mask: &image::GrayImage, crop: [f32; 4], w: f32, h: f32) -> Vec<bool> {
    outline_where(mw, mh, mask, crop, w, h, |v| v > 127)
}

/// As `outline_hole`, for the pixels whose value in the outline file `pick`s.
fn outline_where(mw: u32, mh: u32, mask: &image::GrayImage, crop: [f32; 4], w: f32, h: f32, pick: impl Fn(u8) -> bool) -> Vec<bool> {
    let (mw, mh) = (mw as usize, mh as usize);
    let (kx, ky) = (mask.width() as f32 / w, mask.height() as f32 / h);
    (0..mw * mh)
        .map(|i| {
            let (x, y) = ((i % mw) as f32 + 0.5, (i / mw) as f32 + 0.5);
            let fx = crop[0] + x / mw as f32 * (crop[2] - crop[0]);
            let fy = crop[1] + y / mh as f32 * (crop[3] - crop[1]);
            let (mx, my) = ((fx * kx) as u32, (fy * ky) as u32);
            mx < mask.width() && my < mask.height() && pick(mask.get_pixel(mx, my).0[0])
        })
        .collect()
}

/// The red rectangle round a named object: the area padded (15% across, 25%
/// up and down), so an object on a wall reads as an object on a wall.
#[allow(clippy::too_many_arguments)]
fn mark_red(small: &mut RgbImage, area_px: [f32; 4], crop: [f32; 4], sx: f32, sy: f32, aw: f32, ah: f32, mw: u32, mh: u32) {
    let mark = [
        (area_px[0] - aw * 0.15 - crop[0]) * sx,
        (area_px[1] - ah * 0.25 - crop[1]) * sy,
        (area_px[2] + aw * 0.15 - crop[0]) * sx,
        (area_px[3] + ah * 0.25 - crop[1]) * sy,
    ];
    let line = (mw.max(mh) as f32 * 0.008).max(4.0);
    for (x, y, p) in small.enumerate_pixels_mut() {
        let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
        let near_edge = (xf - mark[0]).abs() < line || (xf - mark[2]).abs() < line || (yf - mark[1]).abs() < line || (yf - mark[3]).abs() < line;
        let within = xf > mark[0] - line && xf < mark[2] + line && yf > mark[1] - line && yf < mark[3] + line;
        if near_edge && within {
            *p = Rgb([255, 0, 0]);
        }
    }
}

/// Ctrl+E, no box: Klein removes `what` from the whole photo (at its working
/// size); where its picture differs from the photo is where the object was.
/// A big object keeps that result; anything smaller is removed again from a
/// full-resolution crop round it (as if a box had been drawn), so its fill is
/// as sharp as the rest of the photo.
fn find_and_remove(photo: &Path, scene: &crate::develop::Scene, what: &str, seed: u32) -> Result<Removal, String> {
    let what = what.trim();
    if what.is_empty() {
        return Err("Say what to remove (e.g. \"the sign\"), or draw a box round it.".into());
    }
    let img = &scene.image;
    let (w, h) = (img.width as f32, img.height as f32);
    let (mw, mh) = model_size(w, h);
    let ev = brightness(scene);
    let before = picture(img, [0.0, 0.0, w, h], mw, mh, ev.exp2());
    let (input, output) = run_files(photo)?;
    before.save(&input).map_err(|e| e.to_string())?;
    let what_clean = what.trim_start_matches("the ").trim_end_matches('.');
    let result = crate::klein::edit(&input, &output, &format!("Remove the {what_clean} from the image. {NOTHING_NEW} Keep everything else exactly the same."), mw, mh, seed);
    let _ = std::fs::remove_file(&input);
    result?;
    let after = image::open(&output).map_err(|e| e.to_string())?.to_rgb8();
    let Some(found) = changed_area(&before, &after) else {
        let _ = std::fs::remove_file(&output);
        return Err(format!("Couldn't find \"{what}\" to remove. Try other words, or draw a box round it."));
    };
    let area = [found[0] / mw as f32, found[1] / mh as f32, found[2] / mw as f32, found[3] / mh as f32];
    // Big enough that the whole-photo result is sharp enough: keep it.
    if (area[2] - area[0]).max(area[3] - area[1]) > 0.4 {
        return Ok(Removal { file: output.to_string_lossy().replace('\\', "/"), crop: [0.0, 0.0, 1.0, 1.0], area, ev });
    }
    let _ = std::fs::remove_file(&output);
    boxed(photo, scene, area, what_clean, None, seed)
}

/// Where the model changed the picture: the bounding box (pixels) of the
/// biggest clearly changed region, or None. The model redraws everything a
/// little, so "clearly" is well above the typical difference; nearby changed
/// pieces (a sign and its post) are joined first.
fn changed_area(a: &RgbImage, b: &RgbImage) -> Option<[f32; 4]> {
    let (w, h) = (a.width() as usize, a.height() as usize);
    if (b.width() as usize, b.height() as usize) != (w, h) {
        return None;
    }
    let diff: Vec<f32> = a
        .pixels()
        .zip(b.pixels())
        .map(|(p, q)| (0..3).map(|c| (p[c] as f32 - q[c] as f32).abs()).sum::<f32>() / (3.0 * 255.0))
        .collect();
    // Smoothed, so single-pixel redraw noise doesn't count.
    let smooth = box_blur(&diff, w, h, 3);
    let mut sorted = smooth.clone();
    sorted.sort_by(|x, y| x.total_cmp(y));
    let median = sorted[sorted.len() / 2];
    let threshold = (median * 6.0).max(0.06);
    let mask: Vec<bool> = smooth.iter().map(|&v| v > threshold).collect();
    // Join pieces within ~1.5% of the picture of each other.
    let joined = dilate(&mask, w, h, ((w.max(h) as f32) * 0.015).ceil() as usize);
    // Biggest region by how much changed in it (8-connected).
    let mut label = vec![0u32; w * h];
    let mut best: Option<(f32, [usize; 4])> = None;
    let mut next = 0u32;
    for start in 0..w * h {
        if !joined[start] || label[start] != 0 {
            continue;
        }
        next += 1;
        let (mut score, mut bb) = (0.0f32, [usize::MAX, usize::MAX, 0, 0]);
        let mut stack = vec![start];
        label[start] = next;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            if mask[i] {
                score += smooth[i];
                bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x), bb[3].max(y)];
            }
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if joined[j] && label[j] == 0 {
                        label[j] = next;
                        stack.push(j);
                    }
                }
            }
        }
        if bb[0] != usize::MAX && best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, bb));
        }
    }
    let (score, bb) = best?;
    // Too little changed anywhere: nothing was removed.
    if score < 20.0 {
        return None;
    }
    let pad = 2.0;
    Some([
        (bb[0] as f32 - pad).max(0.0),
        (bb[1] as f32 - pad).max(0.0),
        (bb[2] as f32 + 1.0 + pad).min(w as f32),
        (bb[3] as f32 + 1.0 + pad).min(h as f32),
    ])
}

/// Box blur of radius r (rows then columns).
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = src[y * w + a..=y * w + b].iter().sum::<f32>() / (b - a + 1) as f32;
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    out
}

/// A mask grown by r pixels (square).
/// `dilate` for the examples (testing).
pub fn dilate_pub(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    dilate(mask, w, h, r)
}

pub(crate) fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut rows = vec![false; mask.len()];
    for y in 0..h {
        let mut last: Option<usize> = None;
        for x in 0..w {
            if mask[y * w + x] {
                last = Some(x);
            }
            if last.is_some_and(|l| x - l <= r) {
                rows[y * w + x] = true;
            }
        }
        let mut last: Option<usize> = None;
        for x in (0..w).rev() {
            if mask[y * w + x] {
                last = Some(x);
            }
            if last.is_some_and(|l| l - x <= r) {
                rows[y * w + x] = true;
            }
        }
    }
    let mut out = vec![false; mask.len()];
    for x in 0..w {
        let mut last: Option<usize> = None;
        for y in 0..h {
            if rows[y * w + x] {
                last = Some(y);
            }
            if last.is_some_and(|l| y - l <= r) {
                out[y * w + x] = true;
            }
        }
        let mut last: Option<usize> = None;
        for y in (0..h).rev() {
            if rows[y * w + x] {
                last = Some(y);
            }
            if last.is_some_and(|l| l - y <= r) {
                out[y * w + x] = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_changed_region() {
        let a = RgbImage::from_pixel(200, 100, Rgb([100, 120, 90]));
        let mut b = a.clone();
        // A faint change everywhere (the model's redraw), a clear one at 120..150 x 20..70.
        for (x, y, p) in b.enumerate_pixels_mut() {
            p.0[0] = if (x + y) % 2 == 0 { 101 } else { 99 };
            if (120..150).contains(&x) && (20..70).contains(&y) {
                *p = Rgb([30, 160, 40]);
            }
        }
        let f = changed_area(&a, &b).expect("found");
        assert!((f[0] - 118.0).abs() < 4.0 && (f[2] - 152.0).abs() < 4.0, "{f:?}");
        assert!((f[1] - 18.0).abs() < 4.0 && (f[3] - 72.0).abs() < 4.0, "{f:?}");
        assert!(changed_area(&a, &a).is_none());
    }

    #[test]
    fn encoding_round_trips() {
        for px in [[0.18f32, 0.18, 0.18], [0.05, 0.1, 0.02], [0.4, 0.3, 0.2]] {
            let e = encode(px, 2.0);
            let d = decode(e.map(|c| c as f32), 2.0);
            for k in 0..3 {
                assert!((d[k] - px[k]).abs() < 0.01 + px[k] * 0.03, "{px:?} -> {e:?} -> {d:?}");
            }
        }
    }

    #[test]
    fn weight_is_full_over_the_area_and_zero_far_away() {
        let crop = [0.0, 0.0, 1000.0, 1000.0];
        let (grown, fade) = blend_shape(crop, [400.0, 400.0, 600.0, 600.0]);
        assert_eq!(weight(500.0, 500.0, grown, fade, crop), 1.0);
        assert_eq!(weight(390.0, 500.0, grown, fade, crop), 1.0);
        assert_eq!(weight(100.0, 500.0, grown, fade, crop), 0.0);
    }
}
