//! RapidOne's own sidecar files (`.rpd`): one per photo, in a `sidecars`
//! folder next to the photos (`<folder>/sidecars/<photo file name>.rpd`, so a
//! RAW and a JPEG of the same name don't clash). Plain JSON that the app
//! writes and reads; Lightroom's XMP files are left where they are and only
//! read when a photo has no `.rpd` yet. RapidCulling (embedded) shares it.
//!
//! Stars and pick live in the `.rpd` too (`rating`, `pick`), not the
//! photo's `.xmp` (Chris, 16 Oct: in RapidOne it's `.rpd`, standalone
//! RapidCulling uses `.xmp`); a photo's `.xmp` rating is read until its
//! `.rpd` has one. Every change of stars/pick is written straight to the
//! `.rpd` (`set_flags`, from Edit or Cull), so the file is the truth for
//! them: writing an edit (`write_edit`) keeps what the file says.

use std::path::{Path, PathBuf};

pub const FOLDER: &str = "sidecars";
const EXTENSION: &str = "rpd";

/// Where a photo's `.rpd` lives.
pub fn path_for(photo: &Path) -> Option<PathBuf> {
    let name = photo.file_name()?.to_string_lossy().to_string();
    Some(photo.parent()?.join(FOLDER).join(format!("{name}.{EXTENSION}")))
}

/// The photo's `.rpd` contents, or None if it has none.
pub fn read(photo: &Path) -> Option<String> {
    std::fs::read_to_string(path_for(photo)?).ok()
}

/// The stars (-1 rejected, 0-5) and pick in the photo's `.rpd`, if it has them.
pub fn flags(photo: &Path) -> Option<(i8, bool)> {
    let v: serde_json::Value = serde_json::from_str(&read(photo)?).ok()?;
    let rating = v.get("rating")?.as_i64()?.clamp(-1, 5) as i8;
    Some((rating, v.get("pick").and_then(serde_json::Value::as_bool).unwrap_or(false)))
}

/// Whether the photo's `.rpd` holds an edit (any slider, curve, crop, mask...), not just stars and
/// pick: a photo reset to no edit keeps its `.rpd` with an empty `edit`.
pub fn has_edit(photo: &Path) -> bool {
    let Some(v) = read(photo).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return false };
    v.get("edit").is_some_and(edit_has_content)
}

/// Where the photo's look came from, as its `.rpd` says: the style or preset last put on it (`look`,
/// `{from, name}`), or its Lightroom XMP (`origin`). None when it says neither.
pub fn look(photo: &Path) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(&read(photo)?).ok()?;
    if let Some(look) = v.get("look").filter(|l| l.get("from").is_some_and(|f| f.is_string()) && l.get("name").is_some_and(|n| n.is_string())) {
        return Some(look.clone());
    }
    (v.get("origin").and_then(serde_json::Value::as_str) == Some("lightroom-xmp") && v.get("edit").is_some_and(edit_has_content))
        .then(|| serde_json::json!({ "from": "xmp", "name": "Lightroom XMP" }))
}

/// Whether the photo's `.rpd` has Noise (AI noise reduction) on.
pub fn denoised(photo: &Path) -> bool {
    let Some(v) = read(photo).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return false };
    v.pointer("/edit/sliders/noise").and_then(serde_json::Value::as_f64).is_some_and(|n| n > 0.0)
}

/// A mask's adjustments (gradients.ts `Gradient`): one with all of them at 0, or switched off, changes nothing.
const MASK_ADJUSTMENTS: [&str; 12] =
    ["exposure", "temperature", "tint", "contrast", "highlights", "shadows", "whites", "blacks", "texture", "clarity", "dehaze", "saturation"];

/// Whether an `.rpd`'s edit changes anything. Sliders all at 0, curves that are straight lines (as
/// panels leave them after being touched and put back) and masks with no adjustment don't.
fn edit_has_content(edit: &serde_json::Value) -> bool {
    use serde_json::Value;
    let Some(edit) = edit.as_object() else { return false };
    let straight = |points: &Value| {
        points.as_array().is_none_or(|ps| {
            ps.iter().all(|p| {
                let (x, y) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64));
                matches!((x, y), (Some(x), Some(y)) if (x - y).abs() < 1e-6)
            })
        })
    };
    edit.iter().any(|(key, x)| match (key.as_str(), x) {
        (_, Value::Null) => false,
        ("sliders", Value::Object(m)) => m.values().any(|v| v.as_f64().is_none_or(|n| n != 0.0)),
        ("curves", Value::Object(m)) => m.values().any(|c| !straight(c)),
        ("gradients", Value::Array(masks)) => masks.iter().any(|g| {
            !g.get("hidden").and_then(Value::as_bool).unwrap_or(false)
                && MASK_ADJUSTMENTS.iter().any(|k| g.get(*k).and_then(Value::as_f64).is_some_and(|n| n != 0.0))
        }),
        (_, Value::Object(m)) => !m.is_empty(),
        (_, Value::Array(a)) => !a.is_empty(),
        _ => true,
    })
}

/// Whether the photo's Lightroom edit (its `.xmp`, from Lightroom or Aftershoot) is the one to use:
/// RapidOne hasn't started an edit of its own, or has only an empty one (every photo opened gets one)
/// or one it took from an earlier `.xmp`, written before the `.xmp` was. An `.xmp` written later is the
/// newer edit (re-edited in Lightroom or Aftershoot); a reset made in RapidOne after it (the `.rpd`
/// newer) stays, and an edit made in RapidOne always wins.
pub fn lightroom_applies(photo: &Path) -> bool {
    let text = read(photo);
    let from_xmp = text.as_deref().is_some_and(|t| t.contains("\"origin\": \"lightroom-xmp\"") || t.contains("\"origin\":\"lightroom-xmp\""));
    if has_edit(photo) && !from_xmp {
        return false;
    }
    if !text.is_some_and(|t| t.contains("\"edit\"")) {
        return true;
    }
    let modified = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut xmps = vec![photo.with_extension("xmp")];
    if let Some(name) = photo.file_name() {
        xmps.push(photo.with_file_name(format!("{}.xmp", name.to_string_lossy())));
    }
    let xmp = xmps.into_iter().find_map(modified);
    let rpd = path_for(photo).and_then(modified);
    matches!((xmp, rpd), (Some(x), Some(r)) if x > r)
}

/// Sets the stars and pick in the photo's `.rpd` (made if need be, with no
/// edit yet, so a Lightroom edit is still read), keeping everything else.
pub fn set_flags(photo: &Path, rating: i8, pick: bool) -> Result<(), String> {
    let mut obj = read(photo)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_else(|| {
            let mut m = serde_json::Map::new();
            m.insert("format".into(), "rapidone-sidecar".into());
            m.insert("version".into(), 1.into());
            m.insert("origin".into(), "rapidone".into());
            m
        });
    obj.insert("rating".into(), rating.into());
    obj.insert("pick".into(), pick.into());
    obj.insert("updated".into(), chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true).into());
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(obj)).map_err(|e| e.to_string())? + "\n";
    write(photo, &text)
}

/// Writes the app's edit for the photo (its whole `.rpd` text), keeping the
/// stars and pick the file already has: they're changed only through
/// `set_flags`, so another tab's newer rating is never overwritten by an
/// older copy.
pub fn write_edit(photo: &Path, contents: &str) -> Result<(), String> {
    let Some((rating, pick)) = flags(photo) else { return write(photo, contents) };
    let Ok(serde_json::Value::Object(mut obj)) = serde_json::from_str::<serde_json::Value>(contents) else { return write(photo, contents) };
    obj.insert("rating".into(), rating.into());
    obj.insert("pick".into(), pick.into());
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(obj)).map_err(|e| e.to_string())? + "\n";
    write(photo, &text)
}

/// Writes the photo's `.rpd` (creating the `sidecars` folder), through a
/// temporary file and a rename, so a crash never leaves half a file.
pub fn write(photo: &Path, contents: &str) -> Result<(), String> {
    let path = path_for(photo).ok_or("no folder for the sidecar")?;
    let dir = path.parent().ok_or("no folder for the sidecar")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, contents).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_xmp_replaces_only_an_empty_edit() {
        let dir = std::env::temp_dir().join(format!("rapidone-lr-applies-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(FOLDER)).unwrap();
        let photo = dir.join("P1.RW2");
        std::fs::write(&photo, b"").unwrap();
        let rpd = path_for(&photo).unwrap();
        let xmp = dir.join("P1.xmp");
        let set_time = |p: &Path, secs: u64| {
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        };
        // No .rpd, or one with flags only: the .xmp.
        assert!(lightroom_applies(&photo));
        std::fs::write(&rpd, r#"{"pick":true}"#).unwrap();
        assert!(lightroom_applies(&photo));
        // An empty edit: only an .xmp written after it.
        std::fs::write(&rpd, r#"{"edit":{"sliders":{}}}"#).unwrap();
        std::fs::write(&xmp, b"xmp").unwrap();
        set_time(&rpd, 2_000_000_000);
        set_time(&xmp, 1_000_000_000);
        assert!(!lightroom_applies(&photo));
        set_time(&xmp, 2_000_000_100);
        assert!(lightroom_applies(&photo));
        // Sliders at 0, a straight curve and a mask with no adjustment are no edit.
        std::fs::write(&rpd, r#"{"edit":{"sliders":{"exposure":0},"curves":{"luma":[{"x":0,"y":0},{"x":0.5,"y":0.5}]},"gradients":[{"kind":"radial","rx":0.2,"exposure":0}]}}"#).unwrap();
        set_time(&rpd, 1_000_000_000);
        assert!(lightroom_applies(&photo));
        // An edit made in RapidOne always wins; one taken from an older .xmp gives way.
        std::fs::write(&rpd, r#"{"edit":{"sliders":{"exposure":0.5}}}"#).unwrap();
        set_time(&rpd, 1_000_000_000);
        assert!(!lightroom_applies(&photo));
        std::fs::write(&rpd, r#"{"origin":"lightroom-xmp","edit":{"sliders":{"exposure":0.5}}}"#).unwrap();
        set_time(&rpd, 1_000_000_000);
        assert!(lightroom_applies(&photo));
        set_time(&rpd, 2_000_000_200);
        assert!(!lightroom_applies(&photo));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stars_from_another_tab_survive_an_edit_save() {
        let dir = std::env::temp_dir().join(format!("rapidone-flags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("IMG_0001.CR3");
        // Cull rates it: an .rpd with stars only (no edit, so a Lightroom edit is still read).
        set_flags(&photo, 4, true).unwrap();
        assert_eq!(flags(&photo), Some((4, true)));
        assert!(!read(&photo).unwrap().contains("\"edit\""));
        // Edit then saves its edit with an older copy of the stars: the file's win.
        let edit = r#"{"format":"rapidone-sidecar","version":1,"origin":"rapidone","edit":{"sliders":{"exposure":0.5}},"rating":0,"pick":false}"#;
        write_edit(&photo, edit).unwrap();
        assert_eq!(flags(&photo), Some((4, true)));
        assert!(read(&photo).unwrap().contains("exposure"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_sits_in_the_sidecars_folder_with_the_full_file_name() {
        let p = path_for(Path::new("shoot/IMG_0001.CR3")).unwrap();
        assert_eq!(p, Path::new("shoot").join("sidecars").join("IMG_0001.CR3.rpd"));
    }

    #[test]
    fn written_sidecar_reads_back() {
        let dir = std::env::temp_dir().join(format!("rapidone-sidecar-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("a.RW2");
        write(&photo, "{\"version\":1}").unwrap();
        assert_eq!(read(&photo).as_deref(), Some("{\"version\":1}"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
