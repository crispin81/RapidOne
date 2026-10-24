//! Lens profile corrections from Adobe's lens profiles (.lcp), read from the
//! user's own Camera Raw / Lightroom install like the Adobe Color profiles
//! (`adobe.rs`); nothing of Adobe's is shipped. Vignetting only for now.
//!
//! An .lcp holds one entry per focal length, aperture and focus distance,
//! each with a vignette model: light kept = 1 + p1 r^2 + p2 r^4 + p3 r^6,
//! r the distance from the centre in units of the focal length (so a 24 mm
//! lens on full frame reaches r = 0.9 in the corners), unless the entry
//! gives its own `FocalLengthX` (then in units of that times the long edge).
//! Matched to the photo by the lens name in its EXIF (`stCamera:Lens`).

use crate::pipeline::SceneImage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// The lens and shot from a RAW's EXIF.
#[derive(Clone, Debug, Default)]
pub struct LensShot {
    pub make: String,
    pub lens: String,
    pub focal: f32,
    pub fnumber: f32,
}

#[derive(Clone, Debug)]
struct Entry {
    focal: f32,
    /// APEX aperture value: 2 log2(f-number).
    av: f32,
    focus: f32,
    format_factor: f32,
    focal_x: Option<f32>,
    centre: (f32, f32),
    vignette: [f32; 3],
}

#[derive(Debug)]
pub struct LensProfile {
    entries: Vec<Entry>,
}

fn attr(s: &str, name: &str) -> Option<f32> {
    let key = format!("stCamera:{name}=\"");
    let at = s.find(&key)? + key.len();
    s[at..].split('"').next()?.trim().parse().ok()
}

fn attr_str<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("stCamera:{name}=\"");
    let at = s.find(&key)? + key.len();
    s[at..].split('"').next()
}

impl LensProfile {
    /// The RAW-file entries of an .lcp that carry a vignette model.
    fn parse(text: &str) -> LensProfile {
        let mut entries = Vec::new();
        // Each top-level entry is an <rdf:li> holding one <rdf:Description>.
        for chunk in text.split("<rdf:li>").skip(1) {
            let Some(focal) = attr(chunk, "FocalLength") else { continue };
            if attr_str(chunk, "CameraRawProfile").is_some_and(|v| !v.eq_ignore_ascii_case("true")) {
                continue;
            }
            let (Some(p1), Some(p2), Some(p3)) = (attr(chunk, "VignetteModelParam1"), attr(chunk, "VignetteModelParam2"), attr(chunk, "VignetteModelParam3")) else { continue };
            entries.push(Entry {
                focal,
                av: attr(chunk, "ApertureValue").unwrap_or(0.0),
                focus: attr(chunk, "FocusDistance").unwrap_or(10000.0),
                format_factor: attr(chunk, "SensorFormatFactor").unwrap_or(1.0),
                focal_x: attr(chunk, "FocalLengthX"),
                centre: (attr(chunk, "ImageXCenter").unwrap_or(0.5), attr(chunk, "ImageYCenter").unwrap_or(0.5)),
                vignette: [p1, p2, p3],
            });
        }
        LensProfile { entries }
    }

    /// The vignette entry for this shot: focused far (EXIF rarely gives the
    /// distance), the nearest focal lengths and apertures, interpolated
    /// (held at the ends).
    fn vignette_at(&self, focal: f32, fnumber: f32) -> Option<Entry> {
        let far = self.entries.iter().map(|e| e.focus).fold(f32::MIN, f32::max);
        let pool: Vec<&Entry> = self.entries.iter().filter(|e| e.focus == far).collect();
        let av = 2.0 * fnumber.max(0.5).log2();
        // Between the two nearest focal lengths, each between its two nearest apertures.
        let mut focals: Vec<f32> = pool.iter().map(|e| e.focal).collect();
        focals.sort_by(f32::total_cmp);
        focals.dedup();
        let bracket = |xs: &[f32], x: f32| -> Option<(f32, f32, f32)> {
            let first = *xs.first()?;
            let last = *xs.last()?;
            if x <= first {
                return Some((first, first, 0.0));
            }
            if x >= last {
                return Some((last, last, 0.0));
            }
            let i = xs.iter().position(|&v| v >= x)?;
            let (a, b) = (xs[i - 1], xs[i]);
            Some((a, b, (x - a) / (b - a)))
        };
        let at_focal = |f: f32| -> Option<Entry> {
            let mut row: Vec<&Entry> = pool.iter().copied().filter(|e| e.focal == f).collect();
            row.sort_by(|a, b| a.av.total_cmp(&b.av));
            let avs: Vec<f32> = row.iter().map(|e| e.av).collect();
            let (a, b, t) = bracket(&avs, av)?;
            let ea = row.iter().find(|e| e.av == a)?;
            let eb = row.iter().find(|e| e.av == b)?;
            Some(blend(ea, eb, t))
        };
        let (fa, fb, t) = bracket(&focals, focal)?;
        Some(blend(&at_focal(fa)?, &at_focal(fb)?, t))
    }
}

fn blend(a: &Entry, b: &Entry, t: f32) -> Entry {
    let mix = |x: f32, y: f32| x + (y - x) * t;
    Entry {
        focal: mix(a.focal, b.focal),
        av: mix(a.av, b.av),
        focus: a.focus,
        format_factor: a.format_factor,
        focal_x: a.focal_x.zip(b.focal_x).map(|(x, y)| mix(x, y)),
        centre: (mix(a.centre.0, b.centre.0), mix(a.centre.1, b.centre.1)),
        vignette: std::array::from_fn(|i| mix(a.vignette[i], b.vignette[i])),
    }
}

/// Every .lcp under Camera Raw's lens profile folders, by the lens name it
/// is for: (camera make it was made with, path).
fn index() -> &'static HashMap<String, Vec<(String, PathBuf)>> {
    static INDEX: OnceLock<HashMap<String, Vec<(String, PathBuf)>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
        let mut stack: Vec<PathBuf> = crate::adobe::camera_raw_dirs().into_iter().map(|d| d.join("LensProfiles")).filter(|d| d.is_dir()).collect();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lcp")) {
                    // The lens name and make sit in the first entry's attributes.
                    let Ok(text) = std::fs::read_to_string(&p) else { continue };
                    let head = &text[..text.len().min(4000)];
                    if let Some(lens) = attr_str(head, "Lens") {
                        let make = attr_str(head, "Make").unwrap_or("").to_string();
                        map.entry(lens.trim().to_string()).or_default().push((make, p));
                    }
                }
            }
        }
        map
    })
}

/// Adobe's profile for this lens, if installed: the one made with the
/// photo's camera make when there are several, else a RAW profile for it.
pub fn for_lens(shot: &LensShot) -> Option<Arc<LensProfile>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<LensProfile>>>>> = OnceLock::new();
    let key = format!("{}|{}", shot.make, shot.lens);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let found = (|| {
        let candidates = index().get(shot.lens.trim())?;
        let make = shot.make.to_ascii_lowercase();
        let mut ordered: Vec<&(String, PathBuf)> = candidates.iter().collect();
        // Same make first, then RAW profiles, then the newer "v2" ones.
        ordered.sort_by_key(|(m, p)| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            (!m.to_ascii_lowercase().contains(&make) as u8, !name.contains("raw") as u8, !name.contains(" v2") as u8)
        });
        ordered.iter().find_map(|(_, p)| {
            let prof = LensProfile::parse(&std::fs::read_to_string(p).ok()?);
            (!prof.entries.is_empty()).then(|| Arc::new(prof))
        })
    })();
    cache.lock().unwrap().insert(key, found.clone());
    found
}

/// Light lost to vignetting divided back out, `amount` of the way (Lightroom's
/// Vignetting slider under Lens Corrections / 100). Returns None when no
/// profile applies.
pub fn correct_vignetting(img: &SceneImage, shot: &LensShot, amount: f32) -> Option<SceneImage> {
    if amount <= 0.0 || shot.focal <= 0.0 {
        return None;
    }
    let e = for_lens(shot)?.vignette_at(shot.focal, shot.fnumber)?;
    let (w, h) = (img.width as f32, img.height as f32);
    let (cx, cy) = (e.centre.0 * w, e.centre.1 * h);
    // Pixels per unit of r.
    let unit = match e.focal_x {
        Some(fx) => fx * w.max(h),
        // Half diagonal in mm (full frame 21.63, smaller for crop sensors).
        None => (0.5 * (w * w + h * h).sqrt()) / (21.633 / e.format_factor.max(0.1)) * e.focal,
    };
    let [p1, p2, p3] = e.vignette;
    let mut out = img.clone();
    use rayon::prelude::*;
    let width = img.width;
    out.px.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let dy = (y as f32 + 0.5 - cy) / unit;
        for (x, c) in row.iter_mut().enumerate() {
            let dx = (x as f32 + 0.5 - cx) / unit;
            let r2 = dx * dx + dy * dy;
            let kept = (1.0 + r2 * (p1 + r2 * (p2 + r2 * p3))).clamp(0.05, 1.0);
            let gain = kept.powf(-amount);
            *c = c.map(|v| v * gain);
        }
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_interpolates_vignette_entries() {
        let lcp = r#"<rdf:li><rdf:Description stCamera:CameraRawProfile="True" stCamera:Lens="X" stCamera:FocalLength="24" stCamera:FocusDistance="10000" stCamera:ApertureValue="2" stCamera:SensorFormatFactor="1"><stCamera:VignetteModel><rdf:Description stCamera:VignetteModelParam1="-1" stCamera:VignetteModelParam2="0" stCamera:VignetteModelParam3="0"/></stCamera:VignetteModel></rdf:Description></rdf:li>
<rdf:li><rdf:Description stCamera:CameraRawProfile="True" stCamera:Lens="X" stCamera:FocalLength="24" stCamera:FocusDistance="10000" stCamera:ApertureValue="4" stCamera:SensorFormatFactor="1"><stCamera:VignetteModel><rdf:Description stCamera:VignetteModelParam1="-0.5" stCamera:VignetteModelParam2="0" stCamera:VignetteModelParam3="0"/></stCamera:VignetteModel></rdf:Description></rdf:li>"#;
        let p = LensProfile::parse(lcp);
        assert_eq!(p.entries.len(), 2);
        // f/2.8 is Av 2.97: about halfway between the two.
        let e = p.vignette_at(24.0, 2.8).unwrap();
        assert!((e.vignette[0] + 0.757).abs() < 0.01, "{:?}", e.vignette);
        // Beyond the widest aperture: held.
        assert_eq!(p.vignette_at(24.0, 1.0).unwrap().vignette[0], -1.0);
    }
}
