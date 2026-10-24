//! Profiles the user adds (Profile menu → Add your own profile…): camera
//! profiles (.dcp: their own, e.g. from a ColorChecker, a bought pack, or
//! Adobe's from the free DNG Converter they installed) and looks (.xmp, e.g.
//! Adobe Color). Copied, at the user's choice, into their profiles folder
//! (Documents/RapidOne/Profiles), outside the project: nothing here is built into
//! RapidOne, shipped with it, or found on the computer by itself. RapidOne's
//! built-in profiles are only the Shinobi ones (`pipeline::Profile`).
//!
//! A profile is chosen by name ("Adobe Standard", "Adobe Color"): a camera
//! profile name covers every camera added under it, and each photo uses the
//! file for its camera. A look renders on the camera profile it names as its
//! base (its v2 where the camera has one, as Lightroom does).

use crate::dcp::{Look, Profile};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    path: PathBuf,
    name: String,
    /// "camera" (.dcp) or "look" (.xmp).
    kind: &'static str,
    camera: String,
    base: String,
    /// A camera profile carrying its full tone curve.
    has_curve: bool,
}

/// One name in the Profile menu.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    pub name: String,
    pub kind: &'static str,
    /// The cameras it covers (camera profiles); a look covers its base's.
    pub cameras: Vec<String>,
}

static DIR: OnceLock<PathBuf> = OnceLock::new();
static INDEX: Mutex<Option<Vec<Entry>>> = Mutex::new(None);

/// Where the user's profiles live (set once at start-up).
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

fn dir() -> Option<&'static Path> {
    DIR.get().map(|p| p.as_path())
}

fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// What a file is, or why it can't be used.
fn describe(path: &Path) -> Result<Entry, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("profile").to_string();
    let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("profile").to_string();
    match ext.as_str() {
        "dcp" => {
            let p = Profile::load(path)?;
            let name = if p.name.trim().is_empty() { stem } else { p.name.trim().to_string() };
            Ok(Entry { path: path.into(), name, kind: "camera", camera: p.camera_model.clone(), base: String::new(), has_curve: p.tone_curve.len() >= 2 })
        }
        "xmp" => {
            let l = Look::load(path)?;
            if l.look_table.is_none() && l.tone_curve.is_empty() {
                return Err(format!("{file}: not a profile (no look table or curve)"));
            }
            let name = if l.name.trim().is_empty() { stem } else { l.name.trim().to_string() };
            Ok(Entry { path: path.into(), name, kind: "look", camera: String::new(), base: l.base_profile.clone(), has_curve: false })
        }
        _ => Err(format!("{file}: not a .dcp or .xmp profile")),
    }
}

/// The index of the user's profiles (read once, refreshed after an add).
fn with_index<T>(f: impl FnOnce(&[Entry]) -> T) -> T {
    let mut guard = INDEX.lock().unwrap();
    if guard.is_none() {
        let entries = dir()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter_map(|p| describe(&p).ok()).collect())
            .unwrap_or_default();
        *guard = Some(entries);
    }
    f(guard.as_deref().unwrap_or(&[]))
}

fn invalidate() {
    *INDEX.lock().unwrap() = None;
}

/// The names to offer, with the cameras each covers.
pub fn list() -> Vec<UserProfile> {
    with_index(|entries| {
        let mut out: Vec<UserProfile> = Vec::new();
        for e in entries {
            let cams: Vec<String> = if e.kind == "camera" {
                vec![e.camera.clone()]
            } else {
                // A look covers the cameras that have its base profile.
                let base = squash(&e.base);
                entries.iter().filter(|c| c.kind == "camera" && squash(&c.name).starts_with(&base)).map(|c| c.camera.clone()).collect()
            };
            match out.iter_mut().find(|u| u.name == e.name && u.kind == e.kind) {
                Some(u) => u.cameras.extend(cams),
                None => out.push(UserProfile { name: e.name.clone(), kind: e.kind, cameras: cams }),
            }
        }
        for u in &mut out {
            u.cameras.sort();
            u.cameras.dedup();
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    })
}

/// Copies the chosen files into the profiles folder, returning the names
/// added and a message for each file that isn't a profile.
pub fn add(files: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let (mut added, mut skipped) = (Vec::new(), Vec::new());
    let Some(dir) = dir() else {
        return (added, vec!["no profiles folder".into()]);
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        return (added, vec![format!("{}: {e}", dir.display())]);
    }
    for src in files {
        let entry = match describe(src) {
            Ok(e) => e,
            Err(e) => {
                skipped.push(e);
                continue;
            }
        };
        let Some(file) = src.file_name() else { continue };
        let dst = dir.join(file);
        if dst != *src {
            if let Err(e) = std::fs::copy(src, &dst) {
                skipped.push(format!("{}: {e}", src.display()));
                continue;
            }
        }
        if !added.contains(&entry.name) {
            added.push(entry.name);
        }
    }
    invalidate();
    (added, skipped)
}

/// Takes every file of a profile name out of the profiles folder.
pub fn remove(name: &str) -> Result<usize, String> {
    let paths: Vec<PathBuf> = with_index(|entries| entries.iter().filter(|e| e.name == name).map(|e| e.path.clone()).collect());
    for p in &paths {
        std::fs::remove_file(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    invalidate();
    Ok(paths.len())
}

/// The camera profile called `name` for `camera`.
fn camera_profile(entries: &[Entry], name: &str, camera: &str) -> Option<PathBuf> {
    let (want, cam) = (squash(name), squash(camera));
    entries.iter().find(|e| e.kind == "camera" && squash(&e.name) == want && squash(&e.camera) == cam).map(|e| e.path.clone())
}

/// What to render `camera` with for the profile called `name`: its camera
/// profile, or a look on its base camera profile. None when the user has no
/// such profile for this camera: the photo then renders with the built-in one.
pub fn for_render(name: &str, camera: &str) -> Option<(Profile, Option<Look>)> {
    let (profile_path, look_path) = with_index(|entries| {
        if let Some(p) = camera_profile(entries, name, camera) {
            return Some((p, None));
        }
        let look = entries.iter().find(|e| e.kind == "look" && e.name == name)?;
        let base = if look.base.trim().is_empty() { "Adobe Standard".to_string() } else { look.base.clone() };
        // Lightroom renders Adobe Color on the camera's Adobe Standard v2 where it has one.
        let base_path = camera_profile(entries, &format!("{base} v2"), camera).or_else(|| camera_profile(entries, &base, camera))?;
        Some((base_path, Some(look.path.clone())))
    })?;
    let profile = Profile::load(&profile_path).ok()?;
    let look = match look_path {
        Some(p) => Some(Look::load(&p).ok()?),
        None => None,
    };
    Some((profile, look))
}

/// The default tone curve for camera profiles that leave theirs out (newer
/// ones rely on the converter's): taken from one of the user's profiles that
/// carries it in full. Empty when none does.
pub fn default_curve() -> Vec<(f32, f32)> {
    let path = with_index(|entries| entries.iter().find(|e| e.has_curve).map(|e| e.path.clone()));
    path.and_then(|p| Profile::load(&p).ok()).map(|p| p.tone_curve).unwrap_or_default()
}
