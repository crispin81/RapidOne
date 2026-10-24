//! Lens corrections from the Lensfun database (lensfun.github.io; its data is
//! CC BY-SA 3.0, credited on the About page): distortion, lateral colour
//! fringing (TCA) and vignetting for about 1,500 lenses. The database ships
//! with the app (`data/lensfun`, compiled in); newer copies of its files in
//! the app's data folder, and the user's own Lensfun-format files in
//! Documents/RapidOne/Lens profiles, are read over it by file name.
//!
//! Our own code from Lensfun's published models (not its LGPL library):
//! - distortion maps the corrected radius r_u to the photo's r_d:
//!   ptlens r_d = r_u (a r_u^3 + b r_u^2 + c r_u + 1 - a - b - c),
//!   poly3 r_d = r_u (1 - k1 + k1 r_u^2), poly5 r_d = r_u (1 + k1 r_u^2 + k2 r_u^4),
//!   r in units of half the shorter side of the calibration sensor
//!   (its crop factor and aspect ratio);
//! - TCA scales red and blue's radius: poly3 v + c r + b r^2, linear k;
//! - vignetting (pa) keeps 1 + k1 r^2 + k2 r^4 + k3 r^6 of the light, r in
//!   units of half the calibration sensor's diagonal.
//!
//! Distances on the sensor (mm) carry over between sensor sizes for the same
//! lens, so a calibration on one body corrects another of a different size.
//! The result is a `lens::LensCorrection`, applied by the engine.

use crate::lens::{LensCorrection, Radial};
use crate::lensprofile::LensShot;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// The database files compiled into the app (`data/lensfun`).
const BUILT_IN: &[(&str, &str)] = &[
    ("6x6.xml", include_str!("../../data/lensfun/6x6.xml")),
    ("actioncams.xml", include_str!("../../data/lensfun/actioncams.xml")),
    ("compact-canon.xml", include_str!("../../data/lensfun/compact-canon.xml")),
    ("compact-casio.xml", include_str!("../../data/lensfun/compact-casio.xml")),
    ("compact-fujifilm.xml", include_str!("../../data/lensfun/compact-fujifilm.xml")),
    ("compact-kodak.xml", include_str!("../../data/lensfun/compact-kodak.xml")),
    ("compact-konica-minolta.xml", include_str!("../../data/lensfun/compact-konica-minolta.xml")),
    ("compact-leica.xml", include_str!("../../data/lensfun/compact-leica.xml")),
    ("compact-nikon.xml", include_str!("../../data/lensfun/compact-nikon.xml")),
    ("compact-olympus.xml", include_str!("../../data/lensfun/compact-olympus.xml")),
    ("compact-panasonic.xml", include_str!("../../data/lensfun/compact-panasonic.xml")),
    ("compact-pentax.xml", include_str!("../../data/lensfun/compact-pentax.xml")),
    ("compact-ricoh.xml", include_str!("../../data/lensfun/compact-ricoh.xml")),
    ("compact-samsung.xml", include_str!("../../data/lensfun/compact-samsung.xml")),
    ("compact-sigma.xml", include_str!("../../data/lensfun/compact-sigma.xml")),
    ("compact-sony.xml", include_str!("../../data/lensfun/compact-sony.xml")),
    ("contax.xml", include_str!("../../data/lensfun/contax.xml")),
    ("generic.xml", include_str!("../../data/lensfun/generic.xml")),
    ("mil-canon.xml", include_str!("../../data/lensfun/mil-canon.xml")),
    ("mil-fujifilm.xml", include_str!("../../data/lensfun/mil-fujifilm.xml")),
    ("mil-hasselblad.xml", include_str!("../../data/lensfun/mil-hasselblad.xml")),
    ("mil-leica.xml", include_str!("../../data/lensfun/mil-leica.xml")),
    ("mil-nikon.xml", include_str!("../../data/lensfun/mil-nikon.xml")),
    ("mil-olympus.xml", include_str!("../../data/lensfun/mil-olympus.xml")),
    ("mil-panasonic.xml", include_str!("../../data/lensfun/mil-panasonic.xml")),
    ("mil-pentax.xml", include_str!("../../data/lensfun/mil-pentax.xml")),
    ("mil-samsung.xml", include_str!("../../data/lensfun/mil-samsung.xml")),
    ("mil-samyang.xml", include_str!("../../data/lensfun/mil-samyang.xml")),
    ("mil-sigma.xml", include_str!("../../data/lensfun/mil-sigma.xml")),
    ("mil-sony.xml", include_str!("../../data/lensfun/mil-sony.xml")),
    ("mil-tamron.xml", include_str!("../../data/lensfun/mil-tamron.xml")),
    ("mil-tokina.xml", include_str!("../../data/lensfun/mil-tokina.xml")),
    ("mil-zeiss.xml", include_str!("../../data/lensfun/mil-zeiss.xml")),
    ("misc.xml", include_str!("../../data/lensfun/misc.xml")),
    ("om-system.xml", include_str!("../../data/lensfun/om-system.xml")),
    ("rf-leica.xml", include_str!("../../data/lensfun/rf-leica.xml")),
    ("slr-canon.xml", include_str!("../../data/lensfun/slr-canon.xml")),
    ("slr-hasselblad.xml", include_str!("../../data/lensfun/slr-hasselblad.xml")),
    ("slr-konica-minolta.xml", include_str!("../../data/lensfun/slr-konica-minolta.xml")),
    ("slr-leica.xml", include_str!("../../data/lensfun/slr-leica.xml")),
    ("slr-nikon.xml", include_str!("../../data/lensfun/slr-nikon.xml")),
    ("slr-olympus.xml", include_str!("../../data/lensfun/slr-olympus.xml")),
    ("slr-panasonic.xml", include_str!("../../data/lensfun/slr-panasonic.xml")),
    ("slr-pentax.xml", include_str!("../../data/lensfun/slr-pentax.xml")),
    ("slr-ricoh.xml", include_str!("../../data/lensfun/slr-ricoh.xml")),
    ("slr-samsung.xml", include_str!("../../data/lensfun/slr-samsung.xml")),
    ("slr-samyang.xml", include_str!("../../data/lensfun/slr-samyang.xml")),
    ("slr-schneider.xml", include_str!("../../data/lensfun/slr-schneider.xml")),
    ("slr-sigma.xml", include_str!("../../data/lensfun/slr-sigma.xml")),
    ("slr-soligor.xml", include_str!("../../data/lensfun/slr-soligor.xml")),
    ("slr-sony.xml", include_str!("../../data/lensfun/slr-sony.xml")),
    ("slr-tamron.xml", include_str!("../../data/lensfun/slr-tamron.xml")),
    ("slr-tokina.xml", include_str!("../../data/lensfun/slr-tokina.xml")),
    ("slr-ussr.xml", include_str!("../../data/lensfun/slr-ussr.xml")),
    ("slr-vivitar.xml", include_str!("../../data/lensfun/slr-vivitar.xml")),
    ("slr-zeiss.xml", include_str!("../../data/lensfun/slr-zeiss.xml")),
];

/// Half a full-frame (36 x 24 mm) sensor's diagonal, mm.
const FULL_FRAME_HALF_DIAGONAL: f32 = 21.633_308;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Distortion {
    PtLens([f32; 3]),
    Poly3(f32),
    Poly5([f32; 2]),
}

impl Distortion {
    /// r_d / r_u at r_u = r.
    fn ratio(&self, r: f32) -> f32 {
        match *self {
            Distortion::PtLens([a, b, c]) => a * r * r * r + b * r * r + c * r + 1.0 - a - b - c,
            Distortion::Poly3(k1) => 1.0 - k1 + k1 * r * r,
            Distortion::Poly5([k1, k2]) => 1.0 + k1 * r * r + k2 * r * r * r * r,
        }
    }
    fn mix(&self, other: &Distortion, t: f32) -> Distortion {
        let m = |a: f32, b: f32| a + (b - a) * t;
        match (*self, *other) {
            (Distortion::PtLens(a), Distortion::PtLens(b)) => Distortion::PtLens(std::array::from_fn(|i| m(a[i], b[i]))),
            (Distortion::Poly3(a), Distortion::Poly3(b)) => Distortion::Poly3(m(a, b)),
            (Distortion::Poly5(a), Distortion::Poly5(b)) => Distortion::Poly5(std::array::from_fn(|i| m(a[i], b[i]))),
            // Different models at neighbouring focal lengths: the nearer one.
            _ => {
                if t < 0.5 {
                    *self
                } else {
                    *other
                }
            }
        }
    }
}

/// Red and blue's radius scale: poly3 [v, c, b] (v + c r + b r^2); linear as [k, 0, 0].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tca {
    red: [f32; 3],
    blue: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Lens {
    pub maker: String,
    /// Its names (the first is the database's own).
    pub models: Vec<String>,
    mounts: Vec<String>,
    /// The calibration sensor's crop factor and aspect ratio (width / height).
    crop: f32,
    aspect: f32,
    /// A fisheye: its distortion is a projection, left alone (vignetting and fringing still corrected).
    fisheye: bool,
    distortion: Vec<(f32, Distortion)>,
    tca: Vec<(f32, Tca)>,
    /// (focal, f-number, distance, k1..k3)
    vignetting: Vec<(f32, f32, f32, [f32; 3])>,
}

impl Lens {
    /// "Maker Model", as the app shows it (the maker left out when the model already starts with it).
    pub fn name(&self) -> String {
        let model = self.models.first().cloned().unwrap_or_default();
        if squash(&model).starts_with(&squash(&self.maker)) {
            model
        } else {
            format!("{} {}", self.maker, model)
        }
    }
    /// Its id: a number from its maker and model, stable across database updates.
    pub fn id(&self) -> u32 {
        fnv1a(&format!("{}|{}", self.maker, self.models.first().map(String::as_str).unwrap_or("")))
    }
    fn has_data(&self) -> bool {
        !self.distortion.is_empty() || !self.tca.is_empty() || !self.vignetting.is_empty()
    }
}

#[derive(Clone, Debug)]
struct Camera {
    maker: String,
    models: Vec<String>,
    mount: String,
    crop: f32,
}

#[derive(Default)]
pub struct Db {
    cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// Mounts a mount takes lenses of (its own and its `compat` ones).
    compat: HashMap<String, Vec<String>>,
}

/// FNV-1a (32-bit): the same number the app's lens choice is saved as.
pub fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

fn num(n: roxmltree::Node, attr: &str) -> Option<f32> {
    n.attribute(attr)?.trim().parse().ok()
}

fn aspect_of(text: &str) -> Option<f32> {
    let t = text.trim();
    if let Some((a, b)) = t.split_once(':') {
        let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        return (b > 0.0).then(|| (a / b).max(b / a));
    }
    t.parse::<f32>().ok().map(|v| v.max(1.0 / v))
}

impl Db {
    /// Reads one database file into this one (lenses and cameras added; nothing replaced).
    fn read(&mut self, xml: &str) {
        // The files start with a DOCTYPE (Lensfun's DTD), which the reader must be told to accept.
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let Ok(doc) = roxmltree::Document::parse_with_options(xml, opts) else { return };
        let child_text = |n: roxmltree::Node, tag: &str| n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).map(|t| t.trim().to_string());
        let all_text = |n: roxmltree::Node, tag: &str| -> Vec<String> {
            n.children().filter(|c| c.has_tag_name(tag)).filter_map(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
        };
        for n in doc.root_element().children().filter(|c| c.is_element()) {
            match n.tag_name().name() {
                "mount" => {
                    if let Some(name) = child_text(n, "name") {
                        self.compat.entry(name).or_default().extend(all_text(n, "compat"));
                    }
                }
                "camera" => {
                    let (Some(maker), Some(mount)) = (child_text(n, "maker"), child_text(n, "mount")) else { continue };
                    let crop = child_text(n, "cropfactor").and_then(|c| c.parse().ok()).unwrap_or(1.0);
                    self.cameras.push(Camera { maker, models: all_text(n, "model"), mount, crop });
                }
                "lens" => {
                    let Some(maker) = child_text(n, "maker") else { continue };
                    let mut lens = Lens {
                        maker,
                        models: all_text(n, "model"),
                        mounts: all_text(n, "mount"),
                        crop: child_text(n, "cropfactor").and_then(|c| c.parse().ok()).unwrap_or(1.0),
                        aspect: child_text(n, "aspect-ratio").and_then(|a| aspect_of(&a)).unwrap_or(1.5),
                        fisheye: child_text(n, "type").is_some_and(|t| t != "rectilinear"),
                        distortion: Vec::new(),
                        tca: Vec::new(),
                        vignetting: Vec::new(),
                    };
                    for cal in n.children().filter(|c| c.has_tag_name("calibration")) {
                        for e in cal.children().filter(|c| c.is_element()) {
                            let Some(focal) = num(e, "focal") else { continue };
                            match (e.tag_name().name(), e.attribute("model").unwrap_or("")) {
                                ("distortion", "ptlens") => lens.distortion.push((focal, Distortion::PtLens([num(e, "a").unwrap_or(0.0), num(e, "b").unwrap_or(0.0), num(e, "c").unwrap_or(0.0)]))),
                                ("distortion", "poly3") => lens.distortion.push((focal, Distortion::Poly3(num(e, "k1").unwrap_or(0.0)))),
                                ("distortion", "poly5") => lens.distortion.push((focal, Distortion::Poly5([num(e, "k1").unwrap_or(0.0), num(e, "k2").unwrap_or(0.0)]))),
                                ("tca", "poly3") => lens.tca.push((
                                    focal,
                                    Tca {
                                        red: [num(e, "vr").unwrap_or(1.0), num(e, "cr").unwrap_or(0.0), num(e, "br").unwrap_or(0.0)],
                                        blue: [num(e, "vb").unwrap_or(1.0), num(e, "cb").unwrap_or(0.0), num(e, "bb").unwrap_or(0.0)],
                                    },
                                )),
                                ("tca", "linear") => lens.tca.push((focal, Tca { red: [num(e, "kr").unwrap_or(1.0), 0.0, 0.0], blue: [num(e, "kb").unwrap_or(1.0), 0.0, 0.0] })),
                                ("vignetting", "pa") => {
                                    let (Some(f), Some(d)) = (num(e, "aperture"), num(e, "distance")) else { continue };
                                    lens.vignetting.push((focal, f, d, [num(e, "k1").unwrap_or(0.0), num(e, "k2").unwrap_or(0.0), num(e, "k3").unwrap_or(0.0)]));
                                }
                                _ => {}
                            }
                        }
                    }
                    for v in [&mut lens.distortion] {
                        v.sort_by(|a, b| a.0.total_cmp(&b.0));
                    }
                    lens.tca.sort_by(|a, b| a.0.total_cmp(&b.0));
                    if !lens.models.is_empty() && lens.has_data() {
                        self.lenses.push(lens);
                    }
                }
                _ => {}
            }
        }
    }

    /// The database: the built-in files, each replaced by a newer copy in the
    /// update folder if there is one, then the user's own files.
    fn load() -> Db {
        let mut files: Vec<(String, String)> = BUILT_IN.iter().map(|(n, x)| (n.to_string(), x.to_string())).collect();
        let dirs = DIRS.get();
        if let Some(dir) = dirs.and_then(|d| d.0.as_ref()) {
            for (name, text) in xml_files(dir) {
                match files.iter_mut().find(|(n, _)| *n == name) {
                    Some(f) => f.1 = text,
                    None => files.push((name, text)),
                }
            }
        }
        if let Some(dir) = dirs.and_then(|d| d.1.as_ref()) {
            files.extend(xml_files(dir));
        }
        let mut db = Db::default();
        for (_, text) in &files {
            db.read(text);
        }
        db
    }

    /// The camera, by its make and model as the RAW decoder names them.
    fn camera(&self, make: &str, model: &str) -> Option<&Camera> {
        let (mk, md) = (squash(make), squash(model));
        if md.is_empty() {
            return None;
        }
        self.cameras.iter().find(|c| {
            let maker = squash(&c.maker);
            (maker.contains(&mk) || mk.contains(&maker)) && c.models.iter().any(|m| {
                let m = squash(m);
                m == md || m == format!("{mk}{md}") || m.strip_prefix(&maker).is_some_and(|rest| rest == md)
            })
        })
    }

    fn fits_mount(&self, lens: &Lens, mount: &str) -> bool {
        lens.mounts.iter().any(|m| m == mount || self.compat.get(mount).is_some_and(|c| c.contains(m)))
    }

    /// The lens a photo was taken with, matched by its EXIF name (its focal
    /// lengths and aperture must agree, then the most words in common), on a
    /// mount the camera takes when the camera is known.
    pub fn find(&self, shot: &LensShot, camera_model: &str) -> Option<&Lens> {
        let want = words(&shot.lens);
        if want.is_empty() {
            return None;
        }
        // Model codes in brackets ("(F045)", Tamron's) often aren't in the database's names: not required.
        let want_nums = numbers(&without_brackets(&shot.lens));
        let camera = self.camera(&shot.make, camera_model);
        let mut best: Option<(f32, &Lens)> = None;
        for lens in &self.lenses {
            if let Some(cam) = camera {
                if !self.fits_mount(lens, &cam.mount) {
                    continue;
                }
            }
            for model in &lens.models {
                let full = format!("{} {}", lens.maker, model);
                // Every number in the EXIF name (focal lengths, aperture) must be in the lens's.
                let have_nums = numbers(&full);
                if !want_nums.iter().all(|n| have_nums.iter().any(|h| (h - n).abs() < 0.01)) {
                    continue;
                }
                let have = words(&full);
                let common = want.iter().filter(|w| have.contains(w)).count() as f32;
                let score = common / (want.len().max(1) as f32) + 0.5 * common / (have.len().max(1) as f32);
                if score >= 0.6 && best.as_ref().is_none_or(|(s, _)| score > *s) {
                    best = Some((score, lens));
                }
            }
        }
        best.map(|(_, l)| l)
    }

    /// The photo's sensor crop factor: its camera's, else the calibration's.
    fn crop_of(&self, make: &str, camera_model: &str, lens: &Lens) -> f32 {
        self.camera(make, camera_model).map(|c| c.crop).filter(|c| *c > 0.0).unwrap_or(lens.crop)
    }
}

/// A lens name's words and numbers, lower case, split where letters meet digits
/// ("XF16-55mmF2.8" -> xf 16 55 mm f 2.8); "mm" and lone "f" dropped.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digit = false;
    for ch in s.to_lowercase().chars() {
        let d = ch.is_ascii_digit() || (ch == '.' && digit);
        if ch.is_alphanumeric() || d {
            if !cur.is_empty() && d != digit {
                out.push(std::mem::take(&mut cur));
            }
            digit = d;
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|t| t != "mm" && t != "f");
    out
}
fn words(s: &str) -> Vec<String> {
    tokens(s).into_iter().filter(|t| !t.chars().next().is_some_and(|c| c.is_ascii_digit())).collect()
}
fn numbers(s: &str) -> Vec<f32> {
    tokens(s).into_iter().filter_map(|t| t.trim_end_matches('.').parse().ok()).collect()
}

/// The text with anything in brackets left out.
fn without_brackets(s: &str) -> String {
    let mut depth = 0;
    s.chars()
        .filter(|&c| {
            match c {
                '(' => depth += 1,
                ')' => depth = (depth - 1).max(0),
                _ => return depth == 0,
            }
            false
        })
        .collect()
}

fn xml_files(dir: &std::path::Path) -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, String)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")))
        .filter_map(|p| Some((p.file_name()?.to_string_lossy().to_string(), std::fs::read_to_string(&p).ok()?)))
        .collect();
    out.sort();
    out
}

/// (updates folder, the user's own folder)
static DIRS: OnceLock<(Option<PathBuf>, Option<PathBuf>)> = OnceLock::new();
static DB: RwLock<Option<Arc<Db>>> = RwLock::new(None);

/// Where newer database files (from Lensfun's GitHub) and the user's own go (set once at start-up).
pub fn set_dirs(updates: Option<PathBuf>, own: Option<PathBuf>) {
    let _ = DIRS.set((updates, own));
}

/// The database, read the first time it's needed.
pub fn db() -> Arc<Db> {
    if let Some(db) = DB.read().unwrap().as_ref() {
        return db.clone();
    }
    let db = Arc::new(Db::load());
    *DB.write().unwrap() = Some(db.clone());
    db
}

/// Read it again (after an update or the user adding files).
pub fn reload() {
    *DB.write().unwrap() = None;
    CORRECTIONS.lock().unwrap().clear();
}

/// Lenses chosen by hand, by the EXIF lens name they're for (a lens the database doesn't
/// recognise, e.g. a manual one): every photo with that name gets it. Kept in the app's data
/// folder beside the updates (lens-choices.json).
static CHOICES: RwLock<Option<HashMap<String, u32>>> = RwLock::new(None);

fn choices_file() -> Option<PathBuf> {
    update_dir().and_then(|d| d.parent().map(|p| p.join("lens-choices.json")))
}

fn with_choices<T>(f: impl FnOnce(&mut HashMap<String, u32>) -> T) -> T {
    let mut guard = CHOICES.write().unwrap();
    let map = guard.get_or_insert_with(|| {
        choices_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// The lens chosen for photos with this EXIF lens name, if one was.
pub fn choice(exif_lens: &str) -> Option<u32> {
    let key = exif_lens.trim().to_string();
    with_choices(|m| m.get(&key).copied())
}

/// Chooses `lens_id` for every photo with this EXIF lens name (0: back to the database's match).
pub fn choose(exif_lens: &str, lens_id: u32) -> Result<(), String> {
    let key = exif_lens.trim().to_string();
    if key.is_empty() {
        return Err("this photo has no lens name to remember the choice by".into());
    }
    let text = with_choices(|m| {
        if lens_id == 0 {
            m.remove(&key);
        } else {
            m.insert(key, lens_id);
        }
        serde_json::to_string_pretty(m).unwrap_or_default()
    });
    CORRECTIONS.lock().unwrap().clear();
    let file = choices_file().ok_or("no folder for lens choices")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(file, text).map_err(|e| e.to_string())
}

/// The user's own Lensfun-format files: each checked, then copied into their lens profiles
/// folder and read in. (names added, a message for each file that isn't one)
pub fn add_own(paths: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut problems = Vec::new();
    let Some(dir) = DIRS.get().and_then(|d| d.1.clone()) else {
        return (added, vec!["no folder for your lens profiles".into()]);
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (added, vec![format!("{}: {e}", dir.display())]);
    }
    for p in paths {
        let file = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            problems.push(format!("{file}: a lens profile is a Lensfun .xml file"));
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            problems.push(format!("{file}: couldn't be read"));
            continue;
        };
        let mut one = Db::default();
        one.read(&text);
        if one.lenses.is_empty() {
            problems.push(format!("{file}: no lens profiles in it (Lensfun's format, with calibration data)"));
            continue;
        }
        match std::fs::write(dir.join(&file), &text) {
            Ok(()) => added.extend(one.lenses.iter().map(|l| l.name())),
            Err(e) => problems.push(format!("{file}: {e}")),
        }
    }
    if !added.is_empty() {
        reload();
    }
    (added, problems)
}

/// Where the newer files go, if set.
pub fn update_dir() -> Option<PathBuf> {
    DIRS.get().and_then(|d| d.0.clone())
}

/// The Lensfun commit the built-in files are from (data/db), and its date.
const BUILT_IN_VERSION: (&str, &str) = ("43f9e00", "2026-10-08T17:57:01Z");

/// The database in use: (Lensfun commit, its date, lenses). An update's, else the built-in one's.
pub fn version() -> (String, String, usize) {
    let lenses = db().lenses.len();
    let saved = update_dir().and_then(|d| std::fs::read_to_string(d.join("version.txt")).ok());
    match saved.as_deref().and_then(|v| v.trim().split_once(' ')) {
        Some((sha, date)) => (sha.to_string(), date.to_string(), lenses),
        None => (BUILT_IN_VERSION.0.to_string(), BUILT_IN_VERSION.1.to_string(), lenses),
    }
}

/// Saves a newer copy of the database (every file, from Lensfun's GitHub at commit `sha`,
/// dated `date`) over the last one, if every file reads as a Lensfun database; then reads
/// it again. Returns the lenses it now has.
pub fn save_update(files: Vec<(String, String)>, sha: &str, date: &str) -> Result<usize, String> {
    let dir = update_dir().ok_or("no folder for lens profile updates")?;
    if files.len() < 20 {
        return Err(format!("only {} files came: kept the lens profiles as they are", files.len()));
    }
    for (name, text) in &files {
        let ok_name = name.ends_with(".xml") && !name.contains(['/', '\\']) && !name.starts_with('.');
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let ok = ok_name && roxmltree::Document::parse_with_options(text, opts).is_ok_and(|d| d.root_element().has_tag_name("lensdatabase"));
        if !ok {
            return Err(format!("{name} isn't a Lensfun database file: kept the lens profiles as they are"));
        }
    }
    // Written beside, then swapped in, so a half-written update is never read.
    let fresh = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    for (name, text) in &files {
        std::fs::write(fresh.join(name), text).map_err(|e| e.to_string())?;
    }
    std::fs::write(fresh.join("version.txt"), format!("{sha} {date}\n")).map_err(|e| e.to_string())?;
    let old = dir.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    if dir.exists() {
        std::fs::rename(&dir, &old).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&fresh, &dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&old);
    reload();
    Ok(db().lenses.len())
}

fn bracket(xs: &[f32], x: f32) -> Option<(usize, usize, f32)> {
    let n = xs.len();
    if n == 0 {
        return None;
    }
    if x <= xs[0] {
        return Some((0, 0, 0.0));
    }
    if x >= xs[n - 1] {
        return Some((n - 1, n - 1, 0.0));
    }
    let i = xs.iter().position(|&v| v >= x)?;
    let (a, b) = (xs[i - 1], xs[i]);
    Some((i - 1, i, if b > a { (x - a) / (b - a) } else { 0.0 }))
}

/// Vignetting at this focal length and f-number: focused far, between the
/// nearest focal lengths, each between its nearest apertures (in stops).
fn vignetting_at(lens: &Lens, focal: f32, fnumber: f32) -> Option<[f32; 3]> {
    let far = lens.vignetting.iter().map(|v| v.2).fold(f32::MIN, f32::max);
    let pool: Vec<&(f32, f32, f32, [f32; 3])> = lens.vignetting.iter().filter(|v| v.2 == far).collect();
    let mut focals: Vec<f32> = pool.iter().map(|v| v.0).collect();
    focals.sort_by(f32::total_cmp);
    focals.dedup();
    let stops = |n: f32| 2.0 * n.max(0.5).log2();
    let at_focal = |f: f32| -> Option<[f32; 3]> {
        let mut row: Vec<&&(f32, f32, f32, [f32; 3])> = pool.iter().filter(|v| v.0 == f).collect();
        row.sort_by(|a, b| a.1.total_cmp(&b.1));
        let xs: Vec<f32> = row.iter().map(|v| stops(v.1)).collect();
        // An unknown f-number: the widest aperture (the strongest vignetting).
        let want = if fnumber > 0.0 { stops(fnumber) } else { xs[0] };
        let (i, j, t) = bracket(&xs, want)?;
        Some(std::array::from_fn(|k| row[i].3[k] + (row[j].3[k] - row[i].3[k]) * t))
    };
    let (i, j, t) = bracket(&focals, focal)?;
    let (a, b) = (at_focal(focals[i])?, at_focal(focals[j])?);
    Some(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t))
}

fn distortion_at(lens: &Lens, focal: f32) -> Option<Distortion> {
    let xs: Vec<f32> = lens.distortion.iter().map(|d| d.0).collect();
    let (i, j, t) = bracket(&xs, focal)?;
    Some(lens.distortion[i].1.mix(&lens.distortion[j].1, t))
}

fn tca_at(lens: &Lens, focal: f32) -> Option<Tca> {
    let xs: Vec<f32> = lens.tca.iter().map(|d| d.0).collect();
    let (i, j, t) = bracket(&xs, focal)?;
    let (a, b) = (lens.tca[i].1, lens.tca[j].1);
    let m = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|k| x[k] + (y[k] - x[k]) * t);
    Some(Tca { red: m(a.red, b.red), blue: m(a.blue, b.blue) })
}

/// Knots across the frame (radius as a fraction of its half diagonal; past 1 for the
/// rounding of the fill), where the corrections are worked out and blended between.
const KNOTS: usize = 64;
const KNOT_MAX: f32 = 1.25;

/// The correction for `lens` on a photo `crop` times smaller than full frame, at this focal
/// length and f-number. `None` when there's nothing to correct.
pub fn correction(lens: &Lens, crop: f32, focal: f32, fnumber: f32) -> Option<LensCorrection> {
    let photo_half_diag = FULL_FRAME_HALF_DIAGONAL / crop.max(0.1);
    let cal_half_diag = FULL_FRAME_HALF_DIAGONAL / lens.crop.max(0.1);
    // Distortion and TCA's unit: half the calibration sensor's shorter side.
    let cal_half_short = cal_half_diag / lens.aspect.hypot(1.0);
    let to_dist = photo_half_diag / cal_half_short;
    let to_vig = photo_half_diag / cal_half_diag;
    let knots: Vec<f32> = (0..=KNOTS).map(|i| i as f32 / KNOTS as f32 * KNOT_MAX).collect();
    let table = |f: &dyn Fn(f32) -> f32| Radial::raw(knots.clone(), knots.iter().map(|&r| f(r)).collect());

    let distortion = (!lens.fisheye).then(|| distortion_at(lens, focal)).flatten().map(|d| table(&|r| d.ratio(r * to_dist).clamp(0.5, 2.0)));
    let chromatic = tca_at(lens, focal).map(|t| {
        let f = |c: [f32; 3]| move |r: f32| {
            let r = r * to_dist;
            (c[0] + c[1] * r + c[2] * r * r).clamp(0.9, 1.1)
        };
        [table(&f(t.red)), table(&f(t.blue))]
    });
    let vignetting = vignetting_at(lens, focal, fnumber).map(|[k1, k2, k3]| {
        table(&|r| {
            let r2 = (r * to_vig).powi(2);
            let kept = 1.0 + r2 * (k1 + r2 * (k2 + r2 * k3));
            1.0 / kept.clamp(1.0 / 16.0, 1.0)
        })
    });
    let c = LensCorrection { vignetting, distortion, chromatic };
    (c.vignetting.is_some() || c.distortion.is_some() || c.chromatic.is_some()).then_some(c)
}

/// What the engine needs to know about the photo for its correction.
pub struct PhotoLens<'a> {
    pub shot: &'a LensShot,
    /// The camera model without its make (the RAW decoder's clean model).
    pub camera_model: &'a str,
}

static CORRECTIONS: Mutex<Vec<(String, Option<Arc<LensCorrection>>)>> = Mutex::new(Vec::new());

/// The correction for a photo: its own lens (`lens_id` 0: found from its EXIF) or the one chosen
/// (by id). Kept, as a slider moving asks again and again.
pub fn for_photo(photo: &PhotoLens, lens_id: u32) -> Option<Arc<LensCorrection>> {
    let key = format!("{}|{}|{}|{}|{}|{}", photo.shot.make, photo.camera_model, photo.shot.lens, photo.shot.focal, photo.shot.fnumber, lens_id);
    if let Some((_, c)) = CORRECTIONS.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return c.clone();
    }
    let db = db();
    // A lens chosen by hand for photos with this EXIF name comes before the database's match.
    let lens_id = if lens_id != 0 { lens_id } else { choice(&photo.shot.lens).unwrap_or(0) };
    let lens = if lens_id != 0 { db.lenses.iter().find(|l| l.id() == lens_id) } else { db.find(photo.shot, photo.camera_model) };
    let c = lens.and_then(|l| {
        let crop = db.crop_of(&photo.shot.make, photo.camera_model, l);
        correction(l, crop, photo.shot.focal, photo.shot.fnumber).map(Arc::new)
    });
    let mut cache = CORRECTIONS.lock().unwrap();
    if cache.len() > 64 {
        cache.remove(0);
    }
    cache.push((key, c.clone()));
    c
}

/// The lens a photo's EXIF names, as the database knows it.
pub fn detected(shot: &LensShot, camera_model: &str) -> Option<(u32, String)> {
    let db = db();
    db.find(shot, camera_model).map(|l| (l.id(), l.name()))
}

/// Lenses whose names have every word of `query`, at most `limit`.
pub fn search(query: &str, limit: usize) -> Vec<(u32, String)> {
    let db = db();
    let want: Vec<String> = query.split_whitespace().map(squash).filter(|w| !w.is_empty()).collect();
    let mut out: Vec<(u32, String)> = Vec::new();
    for l in &db.lenses {
        let name = l.name();
        let all = squash(&format!("{} {}", name, l.models.join(" ")));
        if want.iter().all(|w| all.contains(w)) && !out.iter().any(|(id, _)| *id == l.id()) {
            out.push((l.id(), name));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// A lens's name by its id.
pub fn name_of(id: u32) -> Option<String> {
    db().lenses.iter().find(|l| l.id() == id).map(|l| l.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(make: &str, lens: &str, focal: f32, f: f32) -> LensShot {
        LensShot { make: make.into(), lens: lens.into(), focal, fnumber: f }
    }

    #[test]
    fn reads_the_database() {
        let db = db();
        assert!(db.lenses.len() > 1000, "{}", db.lenses.len());
        assert!(db.cameras.len() > 500, "{}", db.cameras.len());
    }

    #[test]
    fn finds_lenses_by_their_exif_names() {
        let db = db();
        let z = db.find(&shot("Nikon", "NIKKOR Z 24-70mm f/4 S", 24.0, 4.0), "Z 6").expect("Z 24-70");
        assert!(z.name().contains("24-70mm f/4 S"), "{}", z.name());
        // A model code in brackets that the database leaves out.
        let t = db.find(&shot("Nikon", "SP 35mm f/1.4 Di USD (F045)", 35.0, 1.4), "D810").expect("Tamron 35");
        assert!(t.name().contains("Tamron SP 35mm f/1.4"), "{}", t.name());
        // A different focal range never matches.
        let none = db.find(&shot("Nikon", "NIKKOR Z 24-120mm f/4 S", 24.0, 4.0), "Z 6");
        assert!(none.is_none_or(|l| l.name().contains("24-120")), "{:?}", none.map(|l| l.name()));
    }

    #[test]
    fn z_24_70_corrects_as_lensfun_models_it() {
        let db = db();
        let lens = db.find(&shot("Nikon", "NIKKOR Z 24-70mm f/4 S", 24.0, 4.0), "Z 6").unwrap();
        let c = correction(lens, 1.0, 24.0, 4.0).unwrap();
        // ptlens a 0.032 b -0.114 c 0.073 at 24 mm; full frame, 3:2 calibration: the corner is
        // r = 21.63 / 12 = 1.80 half-short-sides.
        let d = c.distortion.as_ref().unwrap();
        let r = 21.633_308 / 12.0;
        let want = 0.032 * r * r * r - 0.114 * r * r + 0.073 * r + 1.0 - 0.032 + 0.114 - 0.073;
        assert!((d.eval(1.0) - want).abs() < 1e-3, "{} vs {want}", d.eval(1.0));
        // Vignetting at the corner (r = 1 half diagonal): 1 / (1 + k1 + k2 + k3).
        let v = c.vignetting.as_ref().unwrap();
        let kept: f32 = 1.0 - 0.444_330_1 - 0.875_733 + 0.462_964_1;
        // (Between table knots: within 2%.)
        assert!((v.eval(1.0) * kept - 1.0).abs() < 0.02, "{} vs {}", v.eval(1.0), 1.0 / kept);
        assert!((v.eval(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ids_and_search() {
        assert_eq!(fnv1a(""), 0x811c_9dc5);
        let hits = search("z 24-70 f/4", 20);
        assert!(hits.iter().any(|(_, n)| n.contains("24-70mm f/4 S")), "{hits:?}");
        let (id, name) = hits[0].clone();
        assert_eq!(name_of(id).as_deref(), Some(name.as_str()));
    }
}
