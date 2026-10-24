//! Auto Style: white balance and exposure as the photographer would set them,
//! learned from their own Lightroom edits (`autostyle-<name>.json`, never in
//! git: a style is personal). For each RAW: the engine's neutral render
//! (as-shot white balance, exposure 0, Shinobi Standard, long edge 256) as a
//! log-chroma histogram and brightness percentiles, plus the as-shot Kelvin
//! and tint and the EXIF exposure (ISO, shutter, aperture, flash). Two
//! answers from those measurements, averaged (better than either alone on a
//! shoot neither has seen, 15 Oct):
//! - a small network (159 -> 128 -> 64 -> 4) giving the change from as-shot
//!   (mired, tint), the Exposure and the Contrast (a style trained before
//!   23 Oct has three outputs: no Contrast, which then comes from its look);
//! - a lookup: the corrections the photographer gave the 15 most similar
//!   edited photos (the style keeps every training photo's measurements).
//! The style also carries its look (every setting the edits share, as a
//! Lightroom sidecar), applied first; masks are never part of it.
//! Train Style (`train`) builds a style from folders of edited RAWs; Increase
//! training adds folders to one and retrains on everything.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Log-chroma histogram bins per axis, and its range (log ratio).
const HIST_BINS: usize = 12;
const HIST_RANGE: f32 = 1.5;
/// Targets (mired, tint, stops, Contrast) are divided by these to train at about unit size.
const SCALE: [f32; 4] = [30.0, 10.0, 1.0, 10.0];
/// Most answers a style gives (with Contrast).
const OUTS: usize = 4;
/// Most similar edited photos the lookup averages.
const LOOKUP_K: usize = 15;
/// Fewest edited photos a style can be trained from.
pub const MIN_PHOTOS: usize = 20;

#[derive(Serialize, Deserialize, Clone)]
struct Layer {
    w: Vec<Vec<f32>>,
    b: Vec<f32>,
}

/// One edited photo the style learned from.
#[derive(Serialize, Deserialize, Clone)]
pub struct Sample {
    /// The RAW (forward slashes).
    pub raw: String,
    /// The folder it came from (one shoot).
    pub folder: String,
    /// Its measurements (`features`).
    pub f: Vec<f32>,
    /// The photographer's correction: change in mired from as-shot, change
    /// in tint, Exposure, and Contrast (missing in a style trained before 23 Oct).
    pub y: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    hist_bins: usize,
    hist_range: f32,
    mu: Vec<f32>,
    sd: Vec<f32>,
    scale: Vec<f32>,
    layers: Vec<Layer>,
    /// The style's look: one of the training sidecars, masks and crop removed,
    /// holding every setting the edits share (applied before the per-photo
    /// white balance, tint and exposure). Empty: white balance and exposure only.
    #[serde(default)]
    pub look_xmp: String,
    /// Or the look from RapidOne's own edits (most training photos edited in RapidOne): one photo's
    /// `.rpd` edit, as `look_xmp` is one photo's sidecar, without what belongs to one photo
    /// (`rpd_look`). The app applies it as it does a preset saved from an edit.
    #[serde(default)]
    pub look_rpd: Option<serde_json::Value>,
    /// The folders it was trained on.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Every training photo, for the lookup and for retraining. A style made
    /// before 15 Oct has none (network only).
    #[serde(default)]
    pub samples: Vec<Sample>,
}

/// The settings Auto Style gives a photo, in Lightroom's terms.
#[derive(serde::Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    pub temperature: f32,
    pub tint: f32,
    pub exposure: f32,
    /// None: a style trained before Contrast was learned (its look's Contrast stands).
    pub contrast: Option<f32>,
}

/// The installed styles: `autostyle-*.json` in the models folders and in
/// `extra` (the app's own styles folder).
pub fn styles(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::mask::model_dirs()
        .into_iter()
        .chain(extra.iter().cloned())
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("autostyle-") && n.ends_with(".json")))
        .collect();
    out.sort();
    out
}

pub fn load(path: &Path) -> Result<Style, String> {
    serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn save(style: &Style, path: &Path) -> Result<(), String> {
    let text = serde_json::to_string(style).map_err(|e| e.to_string())?;
    // Written beside, then moved over, so a failed write never loses a style.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The style file name for a style name ("Brownies" -> autostyle-brownies.json).
pub fn file_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("autostyle-{}.json", if slug.is_empty() { "style" } else { slug })
}

/// What the network needs to know about the photo besides its pixels.
pub struct Exif {
    pub shot_kelvin: f32,
    pub shot_tint: f32,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub flash: f32,
    pub baseline: f32,
}

/// numpy's default percentile (linear between the closest ranks) of sorted values.
fn percentile(sorted: &[f32], p: f32) -> f32 {
    let pos = p / 100.0 * (sorted.len() - 1) as f32;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f32)
}

/// The 159 features, as the training script computes them, from an 8-bit
/// sRGB thumbnail.
pub fn features(rgb: &[[u8; 3]], exif: &Exif, bins: usize, range: f32) -> Vec<f32> {
    let dec = |c: u8| {
        let v = c as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let lin: Vec<[f32; 3]> = rgb.iter().map(|p| p.map(dec)).collect();
    let y: Vec<f32> = lin.iter().map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).collect();
    // Log-chroma histogram (np.histogram2d: last edge inclusive), weighted by sqrt(Y).
    let mut hist = vec![0.0f32; bins * bins];
    let width = 2.0 * range / bins as f32;
    let bin = |v: f32| -> Option<usize> {
        if !(-range..=range).contains(&v) {
            return None;
        }
        Some((((v + range) / width) as usize).min(bins - 1))
    };
    for (c, &yy) in lin.iter().zip(&y) {
        let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        if mx >= 0.98 || mn <= 0.004 {
            continue;
        }
        let (u, v) = ((c[1] / c[0]).ln(), (c[1] / c[2]).ln());
        if let (Some(i), Some(j)) = (bin(u), bin(v)) {
            hist[i * bins + j] += yy.sqrt();
        }
    }
    let total: f32 = hist.iter().sum::<f32>().max(1e-6);
    let mut f: Vec<f32> = hist.iter().map(|h| h / total * 10.0).collect();
    // Brightness percentiles (log2), and the colour of the brightest 5%.
    let mut ly: Vec<f32> = y.iter().map(|v| v.max(1e-4).log2()).collect();
    ly.sort_by(|a, b| a.total_cmp(b));
    for p in [2.0, 10.0, 25.0, 50.0, 75.0, 90.0, 98.0] {
        f.push(percentile(&ly, p) / 5.0);
    }
    let mut ys = y.clone();
    ys.sort_by(|a, b| a.total_cmp(b));
    let cut = percentile(&ys, 95.0);
    let (mut sum, mut n) = ([0.0f64; 3], 0usize);
    for (c, &yy) in lin.iter().zip(&y) {
        if yy >= cut {
            for k in 0..3 {
                sum[k] += c[k] as f64;
            }
            n += 1;
        }
    }
    let bright = sum.map(|s| (s / n.max(1) as f64) as f32);
    let ev100 = if exif.iso > 0.0 && exif.shutter > 0.0 && exif.aperture > 0.0 {
        (exif.aperture.max(0.7).powi(2) / exif.shutter.max(1e-5)).log2() - (exif.iso.max(50.0) / 100.0).log2()
    } else {
        0.0
    };
    f.extend([
        1e6 / exif.shot_kelvin / 100.0,
        exif.shot_tint / 10.0,
        exif.iso.max(50.0).log2() / 10.0,
        ev100 / 10.0,
        exif.flash,
        (bright[0] / bright[1].max(1e-6)).ln(),
        (bright[2] / bright[1].max(1e-6)).ln(),
        exif.baseline,
    ]);
    f
}

/// A RAW's measurements and the camera's as-shot white balance.
pub struct Measured {
    pub f: Vec<f32>,
    pub shot_kelvin: f32,
    pub shot_tint: f32,
}

/// Measures a RAW: the engine's neutral 256 px render and its EXIF.
pub fn measure(raw: &Path) -> Result<Measured, String> {
    let settings = crate::pipeline::RenderSettings { profile: crate::pipeline::Profile::ShinobiStandard, ..Default::default() };
    let scene = crate::develop::develop_scene(raw, 256)?;
    let wb = scene.wb.as_ref().ok_or("no colour data for this camera")?;
    let img = crate::commands::render_image(&scene, &settings).to_rgb8();
    let rgb: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
    let meta = rawler::rawsource::RawSource::new(raw).ok().and_then(|src| {
        let dec = rawler::get_decoder(&src).ok()?;
        dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default()).ok()
    });
    let rat = |r: Option<rawler::formats::tiff::Rational>| r.map(|r| r.n as f32 / r.d.max(1) as f32).unwrap_or(0.0);
    let e = meta.map(|m| m.exif);
    let exif = Exif {
        shot_kelvin: wb.shot_kelvin,
        shot_tint: wb.shot_tint,
        iso: e.as_ref().and_then(|e| e.iso_speed_ratings.map(u32::from).or(e.iso_speed).or(e.recommended_exposure_index)).unwrap_or(0) as f32,
        shutter: rat(e.as_ref().and_then(|e| e.exposure_time)),
        aperture: rat(e.as_ref().and_then(|e| e.fnumber)),
        flash: e.as_ref().and_then(|e| e.flash).map_or(0.0, |f| (f & 1) as f32),
        baseline: settings.profile.baseline_for(&scene.make, &scene.camera) + scene.exposure_offset,
    };
    Ok(Measured { f: features(&rgb, &exif, HIST_BINS, HIST_RANGE), shot_kelvin: wb.shot_kelvin, shot_tint: wb.shot_tint })
}

/// PyTorch's GELU (the exact, erf form).
fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + erf(x / std::f32::consts::SQRT_2))
}

/// GELU's slope.
fn gelu_slope(x: f32) -> f32 {
    0.5 * (1.0 + erf(x / std::f32::consts::SQRT_2)) + x * (-0.5 * x * x).exp() / (2.0 * std::f32::consts::PI).sqrt()
}

/// erf to about 1e-7 (Abramowitz & Stegun 7.1.26 is 1.5e-7).
fn erf(x: f32) -> f32 {
    let x = x as f64;
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * (-x * x).exp();
    (if x >= 0.0 { y } else { -y }) as f32
}

/// How much say each measurement gets in the lookup: the colour map, the
/// brightness percentiles and the camera's numbers count equally as groups
/// (the 144 colour bins would otherwise drown the rest).
fn lookup_weight(i: usize, n: usize, bins: usize) -> f32 {
    let hist = bins * bins;
    let group = if i < hist { hist } else if i < hist + 7 { 7 } else { n - hist - 7 };
    1.0 / (group as f32).sqrt()
}

impl Style {
    /// The network's raw answer: (change in mired, change in tint, exposure[, Contrast]).
    pub fn run(&self, features: &[f32]) -> Vec<f32> {
        let mut x: Vec<f32> = features.iter().zip(self.mu.iter().zip(&self.sd)).map(|(v, (m, s))| (v - m) / s).collect();
        for (i, layer) in self.layers.iter().enumerate() {
            x = layer.w.iter().zip(&layer.b).map(|(row, b)| row.iter().zip(&x).map(|(w, v)| w * v).sum::<f32>() + b).collect();
            if i + 1 < self.layers.len() {
                x = x.into_iter().map(gelu).collect();
            }
        }
        x.iter().zip(&self.scale).map(|(v, k)| v * k).collect()
    }

    /// How many answers it gives: 4 with Contrast, 3 for a style trained before it.
    fn outs(&self) -> usize {
        self.layers.last().map_or(3, |l| l.b.len())
    }

    /// The lookup's answer: the corrections of the most similar training
    /// photos, nearer ones counting more. None for a style without samples.
    pub fn lookup(&self, features: &[f32]) -> Option<Vec<f32>> {
        if self.samples.is_empty() {
            return None;
        }
        let n = features.len();
        let z = |f: &[f32]| -> Vec<f32> {
            (0..n).map(|i| (f[i] - self.mu[i]) / self.sd[i] * lookup_weight(i, n, self.hist_bins)).collect()
        };
        let q = z(features);
        let mut d: Vec<(f32, usize)> = self
            .samples
            .iter()
            .enumerate()
            .filter(|(_, s)| s.f.len() == n && s.y.len() >= self.outs())
            .map(|(k, s)| (z(&s.f).iter().zip(&q).map(|(a, b)| (a - b) * (a - b)).sum::<f32>(), k))
            .collect();
        if d.is_empty() {
            return None;
        }
        let k = LOOKUP_K.min(d.len());
        d.select_nth_unstable_by(k - 1, |a, b| a.0.total_cmp(&b.0));
        let outs = self.outs();
        let (mut sum, mut wsum) = (vec![0.0f32; outs], 0.0f32);
        for &(dist, i) in &d[..k] {
            let w = 1.0 / (dist.max(0.0).sqrt() + 1e-3);
            for c in 0..outs {
                sum[c] += w * self.samples[i].y[c];
            }
            wsum += w;
        }
        Some(sum.into_iter().map(|s| s / wsum).collect())
    }

    /// Auto Style for one RAW: network and lookup averaged (network alone
    /// for a style without samples).
    pub fn predict(&self, raw: &Path) -> Result<Prediction, String> {
        let m = measure(raw)?;
        let net = self.run(&m.f);
        let a = match self.lookup(&m.f) {
            Some(near) => net.iter().zip(&near).map(|(x, y)| (x + y) / 2.0).collect(),
            None => net,
        };
        let (dm, dt, ev) = (a[0], a[1], a[2]);
        let mired = (1e6 / m.shot_kelvin + dm).max(1e6 / 50000.0);
        Ok(Prediction {
            temperature: (1e6 / mired).clamp(2000.0, 50000.0),
            tint: (m.shot_tint + dt).clamp(-150.0, 150.0),
            exposure: ev.clamp(-5.0, 5.0),
            contrast: a.get(3).map(|c| c.clamp(-100.0, 100.0)),
        })
    }

    /// Number of different folders (shoots) it learned from.
    pub fn shoots(&self) -> usize {
        self.samples.iter().map(|s| s.folder.as_str()).collect::<HashSet<_>>().len()
    }
}

// ---- Train Style ------------------------------------------------------------

/// A sidecar's main settings block: the first rdf:Description's own crs:
/// attributes (not its snapshots, history or masks), in order.
fn main_attrs(xmp: &str) -> Vec<(String, String)> {
    let Some(start) = xmp.find("<rdf:Description") else { return Vec::new() };
    let Some(len) = xmp[start..].find('>') else { return Vec::new() };
    let tag = &xmp[start..start + len];
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(at) = rest.find("crs:") {
        rest = &rest[at + 4..];
        let key_len = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
        let key = &rest[..key_len];
        let after = &rest[key_len..];
        if let Some(v) = after.strip_prefix("=\"") {
            if let Some(end) = v.find('"') {
                out.push((key.to_string(), v[..end].to_string()));
                rest = &v[end + 1..];
                continue;
            }
        }
        rest = after;
    }
    out
}

/// The style's look from its training photos: the one whose settings match the ones most edits share
/// best, with what belongs to one photo removed (masks, local adjustments, crop; white balance and
/// exposure, learned per photo). Each photo's edit is the one Edit opens it with (`photo_edit`): an
/// .xmp, compared in RapidOne's slider names and numbers (`preset::parse`; RapidOne's are modelled
/// on Lightroom's), or RapidOne's own. The look is kept in the form it came in: an .xmp (`look_xmp`)
/// or a RapidOne edit (`look_rpd`).
fn build_look(raws: &[PathBuf]) -> (String, Option<serde_json::Value>) {
    enum Source {
        Xmp(PathBuf),
        Rpd(serde_json::Map<String, serde_json::Value>),
    }
    let looks: Vec<(Source, HashMap<String, String>)> = raws
        .par_iter()
        .filter_map(|raw| {
            let e = photo_edit(raw)?;
            let mut flat = HashMap::new();
            let source = match e.xmp {
                Some(x) => {
                    let lr = crate::preset::parse(&std::fs::read_to_string(&x).ok()?);
                    for (k, v) in &lr.sliders {
                        flat.insert(format!("sliders.{k}"), format!("{v:.2}"));
                    }
                    if let Some(p) = &lr.profile {
                        flat.insert("profile".into(), p.clone());
                    }
                    Source::Xmp(x)
                }
                None => {
                    let look = rpd_look(raw)?;
                    for (k, v) in look.get("sliders").and_then(|s| s.as_object()).into_iter().flatten() {
                        flat.insert(format!("sliders.{k}"), v.as_f64().map_or_else(|| v.to_string(), |n| format!("{n:.2}")));
                    }
                    if let Some(p) = look.get("profile").and_then(|p| p.as_str()) {
                        flat.insert("profile".into(), p.to_string());
                    }
                    Source::Rpd(look)
                }
            };
            // Per photo, never compared (as never part of the look).
            flat.retain(|k, _| !k.strip_prefix("sliders.").is_some_and(|s| RPD_PER_PHOTO_SLIDERS.contains(&s) || s == "noise"));
            Some((source, flat))
        })
        .collect();
    if looks.is_empty() {
        return (String::new(), None);
    }
    // A slider a photo doesn't set is at 0 (its neutral), as Lightroom leaves it out.
    let keys: HashSet<&String> = looks.iter().flat_map(|(_, a)| a.keys()).collect();
    let value = |a: &HashMap<String, String>, k: &String| -> Option<String> {
        a.get(k).cloned().or_else(|| k.starts_with("sliders.").then(|| "0.00".to_string()))
    };
    let mode: Vec<(&String, Option<String>)> = keys
        .into_iter()
        .map(|k| {
            let mut counts: HashMap<Option<String>, usize> = HashMap::new();
            for (_, a) in &looks {
                *counts.entry(value(a, k)).or_default() += 1;
            }
            (k, counts.into_iter().max_by_key(|(v, c)| (*c, v.clone())).and_then(|(v, _)| v))
        })
        .collect();
    let best = looks.iter().max_by_key(|(_, a)| mode.iter().filter(|(k, v)| value(a, k) == *v).count()).expect("not empty");
    match &best.0 {
        Source::Xmp(x) => (xmp_look(x), None),
        Source::Rpd(look) => (String::new(), Some(serde_json::Value::Object(look.clone()))),
    }
}

/// An .xmp as a look: masks, local adjustments and crop removed.
fn xmp_look(best: &Path) -> String {
    let Ok(mut look) = std::fs::read_to_string(best) else { return String::new() };
    for tag in ["MaskGroupBasedCorrections", "GradientBasedCorrections", "CircularGradientBasedCorrections", "PaintBasedCorrections", "RetouchAreas", "RedEyeInfo"] {
        let (open, close) = (format!("<crs:{tag}>"), format!("</crs:{tag}>"));
        while let Some(a) = look.find(&open) {
            let Some(b) = look[a..].find(&close) else { break };
            look.replace_range(a..a + b + close.len(), "");
        }
    }
    strip_crop(&look)
}

/// RapidOne's per-photo settings, never part of a look (as `PER_PHOTO` for an .xmp): white balance and
/// exposure (learned per photo), straighten and keystone; and the edit's crop, masks and removals.
const RPD_PER_PHOTO_SLIDERS: [&str; 8] = ["temperature", "tint", "exposure", "autoTransform", "rotate", "vertical", "horizontal", "scale"];
const RPD_PER_PHOTO: [&str; 3] = ["crop", "gradients", "removals"];

/// A photo's RapidOne edit as a look: its `.rpd` edit without the per-photo settings.
fn rpd_look(raw: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let v: serde_json::Value = serde_json::from_str(&crate::sidecar::read(raw)?).ok()?;
    let mut edit = v.get("edit")?.as_object()?.clone();
    for k in RPD_PER_PHOTO {
        edit.remove(k);
    }
    if let Some(sliders) = edit.get_mut("sliders").and_then(|s| s.as_object_mut()) {
        sliders.retain(|k, _| !RPD_PER_PHOTO_SLIDERS.contains(&k.as_str()));
    }
    Some(edit)
}

/// Removes every crs:HasCrop / crs:Crop* attribute.
fn strip_crop(xmp: &str) -> String {
    let mut out = String::with_capacity(xmp.len());
    let mut rest = xmp;
    while let Some(at) = rest.find("crs:") {
        let after = &rest[at + 4..];
        let is_crop = after.starts_with("HasCrop=\"") || (after.starts_with("Crop") && after[4..].split('=').next().is_some_and(|k| k.chars().all(|c| c.is_ascii_alphanumeric())));
        let value_end = after.find("=\"").and_then(|q| after[q + 2..].find('"').map(|e| at + 4 + q + 2 + e + 1));
        match (is_crop, value_end) {
            (true, Some(end)) => {
                out.push_str(rest[..at].trim_end());
                rest = &rest[end..];
            }
            _ => {
                out.push_str(&rest[..at + 4]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The photographer's Temperature, Tint, Exposure and Contrast from a sidecar
/// (Contrast 0 when the sidecar leaves it out, as Lightroom does at 0).
fn edit_of(xmp: &Path) -> Option<[f32; 4]> {
    let text = std::fs::read_to_string(xmp).ok()?;
    let a: HashMap<String, String> = main_attrs(&text).into_iter().collect();
    let num = |k: &str| a.get(k)?.trim().parse::<f32>().ok();
    Some([num("Temperature")?, num("Tint")?, num("Exposure2012")?, num("Contrast2012").unwrap_or(0.0)])
}

/// A photo's edit to learn from, the one Edit opens it with (`sidecar::lightroom_applies`): its
/// Lightroom / Aftershoot `.xmp` (`photo.xmp` or `photo.ext.xmp`), or RapidOne's own edit in its
/// `.rpd` (on the same scales: Kelvin, tint, stops, Contrast). None: no edit.
struct PhotoEdit {
    /// None: as shot (RapidOne's edit left white balance alone).
    temperature: Option<f32>,
    tint: Option<f32>,
    exposure: f32,
    contrast: f32,
    /// The `.xmp` it came from, for the style's look; None for a RapidOne edit.
    xmp: Option<PathBuf>,
}

fn photo_edit(raw: &Path) -> Option<PhotoEdit> {
    if crate::sidecar::lightroom_applies(raw) {
        let mut xmps = vec![crate::xmp::sidecar_path_for(raw)];
        if let Some(name) = raw.file_name() {
            xmps.push(raw.with_file_name(format!("{}.xmp", name.to_string_lossy())));
        }
        return xmps.into_iter().find_map(|x| {
            let [t, tint, exposure, contrast] = edit_of(&x)?;
            Some(PhotoEdit { temperature: Some(t), tint: Some(tint), exposure, contrast, xmp: Some(x) })
        });
    }
    if !crate::sidecar::has_edit(raw) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&crate::sidecar::read(raw)?).ok()?;
    let sliders = v.get("edit")?.get("sliders");
    let num = |k: &str| sliders.and_then(|s| s.get(k)).and_then(serde_json::Value::as_f64).map(|n| n as f32);
    Some(PhotoEdit {
        temperature: num("temperature").filter(|t| *t > 0.0),
        tint: num("tint"),
        exposure: num("exposure").unwrap_or(0.0),
        contrast: num("contrast").unwrap_or(0.0),
        xmp: None,
    })
}

/// Where Train Style is: "reading" (photos measured / to measure) or
/// "training" (passes done / total).
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

/// Trains a style from folders of edited RAWs (only RAWs with a .xmp edit),
/// adding to `base` (Increase training) when given: its photos are kept,
/// photos it already has are skipped, and it is retrained on all of them.
pub fn train(name: &str, folders: &[String], base: Option<Style>, progress: Progress) -> Result<Style, String> {
    let mut samples = base.as_ref().map(|s| s.samples.clone()).unwrap_or_default();
    // Photos learned before Contrast was (Increase training on an older style): their Contrast
    // from their sidecars, so the retrained style learns it from all of them.
    samples.par_iter_mut().filter(|s| s.y.len() < OUTS).for_each(|s| {
        if let Some(e) = photo_edit(Path::new(&s.raw)) {
            s.y.truncate(3);
            s.y.push(e.contrast);
        }
    });
    let known: HashSet<String> = samples.iter().map(|s| s.raw.clone()).collect();
    let mut todo: Vec<(PathBuf, String, PhotoEdit)> = Vec::new();
    for folder in folders {
        let folder = folder.replace('\\', "/");
        for photo in crate::library::find_photos(Path::new(&folder))? {
            let raw = photo.raw_path;
            let is_jpeg = raw.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
            if is_jpeg || known.contains(&raw.to_string_lossy().replace('\\', "/")) {
                continue;
            }
            if let Some(edit) = photo_edit(&raw) {
                todo.push((raw, folder.clone(), edit));
            }
        }
    }
    let total = todo.len();
    let done = AtomicUsize::new(0);
    progress("reading", 0, total);
    let fresh: Vec<Sample> = todo
        .par_iter()
        .filter_map(|(raw, folder, e)| {
            let m = measure(raw).ok();
            progress("reading", done.fetch_add(1, Ordering::Relaxed) + 1, total);
            let m = m?;
            // White balance left as shot (a RapidOne edit that didn't touch it): no change.
            let mired = e.temperature.map_or(0.0, |t| 1e6 / t - 1e6 / m.shot_kelvin);
            let tint = e.tint.map_or(0.0, |t| t - m.shot_tint);
            Some(Sample {
                raw: raw.to_string_lossy().replace('\\', "/"),
                folder: folder.clone(),
                f: m.f,
                y: vec![mired, tint, e.exposure, e.contrast],
            })
        })
        .collect();
    if base.is_some() && fresh.is_empty() {
        return Err("No new edited photos in that folder (only RAWs with an edit count: a Lightroom / Aftershoot .xmp or RapidOne's own; ones already learned are skipped).".into());
    }
    samples.extend(fresh);
    if samples.len() < MIN_PHOTOS {
        return Err(format!(
            "Found {} edited photo{}; a style needs at least {MIN_PHOTOS} (RAWs with an edit: a Lightroom / Aftershoot .xmp or RapidOne's own).",
            samples.len(),
            if samples.len() == 1 { "" } else { "s" }
        ));
    }
    // Contrast learned per photo only where it beats the look's one Contrast on shoots it
    // hasn't seen (Chris's photos vary it by a point or two, so there it doesn't, 23 Oct).
    progress("checking", 0, 1);
    let with_contrast = contrast_helps(&samples);
    let (mu, sd, layers) = fit(&samples, 0, with_contrast, progress);
    let outs = layers.last().map_or(3, |l| l.b.len());
    // The look (every other setting): from the photos trained on now (a new style's, or the shoot
    // added by Increase training), so a look tweaked on the latest job carries on; any of them, from
    // an .xmp or RapidOne's own edit alike (`build_look`).
    let added: Vec<String> = folders.iter().map(|f| f.replace('\\', "/")).collect();
    let recent: Vec<PathBuf> = samples.iter().filter(|s| added.contains(&s.folder)).map(|s| PathBuf::from(&s.raw)).collect();
    let pool = if recent.is_empty() { samples.iter().map(|s| PathBuf::from(&s.raw)).collect() } else { recent };
    let (look_xmp, look_rpd) = build_look(&pool);
    let mut all_folders = base.as_ref().map(|s| s.folders.clone()).unwrap_or_default();
    for f in folders {
        let f = f.replace('\\', "/");
        if !all_folders.contains(&f) {
            all_folders.push(f);
        }
    }
    Ok(Style {
        name: name.trim().to_string(),
        hist_bins: HIST_BINS,
        hist_range: HIST_RANGE,
        mu,
        sd,
        scale: SCALE[..outs].to_vec(),
        layers,
        look_xmp,
        look_rpd,
        folders: all_folders,
        samples,
    })
}

/// Small deterministic random numbers (SplitMix64) for training.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Network sizes: inputs -> 128 -> 64 -> 4 (3 before Contrast), as `wbexp_train.py` (plus Contrast).
const H1: usize = 128;
const H2: usize = 64;
const EPOCHS: usize = 400;
const BATCH: usize = 64;
const DROPOUT: f32 = 0.2;
const LR: f32 = 2e-3;
const WEIGHT_DECAY: f32 = 1e-2;

/// Where each layer's weights and biases sit in the flat parameter list.
struct Shape {
    d: usize,
    w1: usize,
    b1: usize,
    w2: usize,
    b2: usize,
    w3: usize,
    b3: usize,
    len: usize,
    /// Outputs: 4 (with Contrast), or 3.
    o: usize,
}

impl Shape {
    fn new(d: usize, o: usize) -> Self {
        let w1 = 0;
        let b1 = w1 + H1 * d;
        let w2 = b1 + H1;
        let b2 = w2 + H2 * H1;
        let w3 = b2 + H2;
        let b3 = w3 + o * H2;
        Shape { d, w1, b1, w2, b2, w3, b3, len: b3 + o, o }
    }
}

/// One photo through the network and back: adds its loss slopes to `g`.
fn backprop(p: &[f32], s: &Shape, x: &[f32], t: &[f32], keep: &[f32], scale: f32, g: &mut [f32]) {
    let (d, w1, w2, w3) = (s.d, &p[s.w1..s.b1], &p[s.w2..s.b2], &p[s.w3..s.b3]);
    let mut h1 = [0.0f32; H1];
    let mut a1 = [0.0f32; H1];
    for j in 0..H1 {
        h1[j] = p[s.b1 + j] + w1[j * d..(j + 1) * d].iter().zip(x).map(|(w, v)| w * v).sum::<f32>();
        a1[j] = gelu(h1[j]) * keep[j];
    }
    let mut h2 = [0.0f32; H2];
    let mut a2 = [0.0f32; H2];
    for j in 0..H2 {
        h2[j] = p[s.b2 + j] + w2[j * H1..(j + 1) * H1].iter().zip(&a1).map(|(w, v)| w * v).sum::<f32>();
        a2[j] = gelu(h2[j]);
    }
    let mut dout = [0.0f32; OUTS];
    for c in 0..s.o {
        let o = p[s.b3 + c] + w3[c * H2..(c + 1) * H2].iter().zip(&a2).map(|(w, v)| w * v).sum::<f32>();
        // Smooth L1 (beta 1), averaged over the batch and the targets.
        dout[c] = (o - t[c]).clamp(-1.0, 1.0) * scale;
    }
    let mut dh2 = [0.0f32; H2];
    for c in 0..s.o {
        g[s.b3 + c] += dout[c];
        for j in 0..H2 {
            g[s.w3 + c * H2 + j] += dout[c] * a2[j];
            dh2[j] += dout[c] * w3[c * H2 + j];
        }
    }
    let mut dh1 = [0.0f32; H1];
    for j in 0..H2 {
        dh2[j] *= gelu_slope(h2[j]);
        g[s.b2 + j] += dh2[j];
        let row = &w2[j * H1..(j + 1) * H1];
        let grow = &mut g[s.w2 + j * H1..s.w2 + (j + 1) * H1];
        for i in 0..H1 {
            grow[i] += dh2[j] * a1[i];
            dh1[i] += dh2[j] * row[i];
        }
    }
    for j in 0..H1 {
        let dj = dh1[j] * keep[j] * gelu_slope(h1[j]);
        if dj == 0.0 {
            continue;
        }
        g[s.b1 + j] += dj;
        for (gw, v) in g[s.w1 + j * d..s.w1 + (j + 1) * d].iter_mut().zip(x) {
            *gw += dj * v;
        }
    }
}

/// Trains the network (as `wbexp_train.py`: AdamW, smooth L1, dropout 0.2
/// after the first layer, 400 passes in batches of 64). Returns the feature
/// scaling and the layers.
fn fit(samples: &[Sample], seed: u64, with_contrast: bool, progress: Progress) -> (Vec<f32>, Vec<f32>, Vec<Layer>) {
    let n = samples.len();
    let d = samples[0].f.len();
    let mut mu = vec![0.0f32; d];
    let mut sd = vec![0.0f32; d];
    for s in samples {
        for i in 0..d {
            mu[i] += s.f[i] / n as f32;
        }
    }
    for s in samples {
        for i in 0..d {
            sd[i] += (s.f[i] - mu[i]).powi(2) / n as f32;
        }
    }
    for v in &mut sd {
        *v = v.sqrt() + 1e-6;
    }
    let xs: Vec<Vec<f32>> = samples.iter().map(|s| (0..d).map(|i| (s.f[i] - mu[i]) / sd[i]).collect()).collect();
    // Contrast learned too when every photo has it (a style's older photos without a readable
    // sidecar leave it to the look).
    let o = if with_contrast && samples.iter().all(|s| s.y.len() >= OUTS) { OUTS } else { 3 };
    let ts: Vec<Vec<f32>> = samples.iter().map(|s| (0..o).map(|c| s.y[c] / SCALE[c]).collect()).collect();

    // PyTorch's default Linear start: uniform within 1/sqrt(inputs).
    let shape = Shape::new(d, o);
    let mut rng = Rng(seed ^ 0x5EED);
    let mut p = vec![0.0f32; shape.len];
    for (from, to, fan_in) in [(shape.w1, shape.w2, d), (shape.w2, shape.w3, H1), (shape.w3, shape.len, H2)] {
        let bound = 1.0 / (fan_in as f32).sqrt();
        for v in &mut p[from..to] {
            *v = (rng.unit() * 2.0 - 1.0) * bound;
        }
    }
    let (mut m, mut v) = (vec![0.0f32; shape.len], vec![0.0f32; shape.len]);
    let (beta1, beta2, eps) = (0.9f32, 0.999f32, 1e-8f32);
    let threads = rayon::current_num_threads().clamp(1, 8);
    let mut bufs = vec![vec![0.0f32; shape.len]; threads];
    let mut order: Vec<usize> = (0..n).collect();
    let mut step = 0i32;
    for epoch in 0..EPOCHS {
        for i in (1..n).rev() {
            order.swap(i, (rng.next() % (i as u64 + 1)) as usize);
        }
        for batch in order.chunks(BATCH) {
            step += 1;
            let scale = 1.0 / (batch.len() * o) as f32;
            let per = batch.len().div_ceil(threads);
            let step_seed = rng.next();
            batch.par_chunks(per).zip(bufs.par_iter_mut()).for_each(|(part, g)| {
                g.iter_mut().for_each(|x| *x = 0.0);
                for &k in part {
                    let mut drop = Rng(step_seed ^ (k as u64).wrapping_mul(0x9E37_79B9));
                    let keep: Vec<f32> = (0..H1).map(|_| if drop.unit() < DROPOUT { 0.0 } else { 1.0 / (1.0 - DROPOUT) }).collect();
                    backprop(&p, &shape, &xs[k], &ts[k], &keep, scale, g);
                }
            });
            let used = batch.len().div_ceil(per);
            let (c1, c2) = (1.0 - beta1.powi(step), 1.0 - beta2.powi(step));
            for i in 0..shape.len {
                let gi: f32 = bufs[..used].iter().map(|b| b[i]).sum();
                p[i] -= LR * WEIGHT_DECAY * p[i];
                m[i] = beta1 * m[i] + (1.0 - beta1) * gi;
                v[i] = beta2 * v[i] + (1.0 - beta2) * gi * gi;
                p[i] -= LR * (m[i] / c1) / ((v[i] / c2).sqrt() + eps);
            }
        }
        progress("training", epoch + 1, EPOCHS);
    }
    let rows = |at: usize, outs: usize, ins: usize| (0..outs).map(|j| p[at + j * ins..at + (j + 1) * ins].to_vec()).collect::<Vec<_>>();
    let layers = vec![
        Layer { w: rows(shape.w1, H1, d), b: p[shape.b1..shape.w2].to_vec() },
        Layer { w: rows(shape.w2, H2, H1), b: p[shape.b2..shape.w3].to_vec() },
        Layer { w: rows(shape.w3, o, H2), b: p[shape.b3..shape.len].to_vec() },
    ];
    (mu, sd, layers)
}

/// Whether predicting Contrast per photo beats giving every photo the same one (what the look
/// does): held-out photos' Contrast, network + lookup, against the training photos' average.
/// Held out by shoot (the four largest, each tested on a style trained on the rest), or with one
/// shoot in five parts. Kept only when its error is clearly smaller (by a tenth).
fn contrast_helps(samples: &[Sample]) -> bool {
    if samples.iter().any(|s| s.y.len() < OUTS) {
        return false;
    }
    let mut shoots: Vec<(&str, usize)> = Vec::new();
    for s in samples {
        match shoots.iter_mut().find(|f| f.0 == s.folder) {
            Some(f) => f.1 += 1,
            None => shoots.push((&s.folder, 1)),
        }
    }
    shoots.sort_by(|a, b| b.1.cmp(&a.1));
    let folds: Vec<Box<dyn Fn(usize, &Sample) -> bool + Sync>> = if shoots.len() >= 2 {
        shoots.iter().take(4).map(|(f, _)| Box::new(move |_: usize, s: &Sample| s.folder == *f) as Box<dyn Fn(usize, &Sample) -> bool + Sync>).collect()
    } else {
        (0..5).map(|k| Box::new(move |i: usize, _: &Sample| i % 5 == k) as Box<dyn Fn(usize, &Sample) -> bool + Sync>).collect()
    };
    let (mut predicted, mut flat) = (0.0f32, 0.0f32);
    for held in &folds {
        let train: Vec<Sample> = samples.iter().enumerate().filter(|(i, s)| !held(*i, s)).map(|(_, s)| s.clone()).collect();
        let test: Vec<&Sample> = samples.iter().enumerate().filter(|(i, s)| held(*i, s)).map(|(_, s)| s).collect();
        if train.len() < MIN_PHOTOS || test.is_empty() {
            continue;
        }
        let m = fit_for_test("check", &train);
        let mean = train.iter().map(|s| s.y[3]).sum::<f32>() / train.len() as f32;
        for s in test {
            let (net, near) = (m.run(&s.f), m.lookup(&s.f));
            let c = near.map_or(net[3], |n| (net[3] + n[3]) / 2.0);
            predicted += (c - s.y[3]).abs();
            flat += (mean - s.y[3]).abs();
        }
    }
    flat > 0.0 && predicted < 0.9 * flat
}

/// For checking the trainer: trains on `train` only and returns a style
/// (no look) to predict the rest with.
pub fn fit_for_test(name: &str, train: &[Sample]) -> Style {
    let (mu, sd, layers) = fit(train, 0, true, &|_, _, _| {});
    let outs = layers.last().map_or(3, |l| l.b.len());
    Style {
        name: name.into(),
        hist_bins: HIST_BINS,
        hist_range: HIST_RANGE,
        mu,
        sd,
        scale: SCALE[..outs].to_vec(),
        layers,
        look_xmp: String::new(),
        look_rpd: None,
        folders: Vec::new(),
        samples: train.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_file_names() {
        assert_eq!(file_name("Brownies"), "autostyle-brownies.json");
        assert_eq!(file_name("  Sam's  Brownies! "), "autostyle-sam-s-brownies.json");
        assert_eq!(file_name("***"), "autostyle-style.json");
    }

    #[test]
    fn main_block_and_crop() {
        let x = r#"<rdf:Description rdf:about="" crs:Temperature="3628" crs:Tint="+19" crs:Exposure2012="+1.68" crs:HasCrop="True" crs:CropTop="0.1" crs:Contrast2012="+24">
 <crs:Look crs:Name="Adobe Color"/></rdf:Description>"#;
        let a = main_attrs(x);
        assert_eq!(a[0], ("Temperature".to_string(), "3628".to_string()));
        assert_eq!(a.len(), 6);
        let s = strip_crop(x);
        assert!(!s.contains("Crop") && s.contains(r#"crs:Exposure2012="+1.68" crs:Contrast2012="+24""#) && s.contains("crs:Look"));
    }
}
