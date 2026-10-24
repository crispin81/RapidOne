//! The folder's own cache, so reopening a shoot is instant: kept with the
//! photos in `sidecars/cache/` (next to the `.rpd` files), named after each
//! photo file. Safe to delete at any time; it's rebuilt as photos are viewed.
//!
//! - `<photo>.preview.jpg` + `<photo>.preview.json`: the last engine picture
//!   and the settings it was rendered with, so it's shown at once if the edit
//!   hasn't changed since.
//! - `<photo>.mask.png`: the subject mask (`mask.rs`), about 3 s saved.

use crate::commands::DevelopedPreview;
use crate::mask::Mask;
use base64::Engine;
use std::path::{Path, PathBuf};

fn file(photo: &Path, suffix: &str) -> Option<PathBuf> {
    let name = photo.file_name()?.to_string_lossy().to_string();
    Some(photo.parent()?.join("sidecars").join("cache").join(format!("{name}.{suffix}")))
}

/// Writes through a temporary file and a rename, creating the folder.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PreviewMeta {
    /// The settings the picture was rendered with (the app's JSON key).
    key: String,
    preview: DevelopedPreview,
}

/// A cached engine picture and the settings key it was rendered with. The
/// picture belongs to the photo's `.rpd`: if that has gone (deleted to start
/// again from the XMP), the cached picture is dropped too and rebuilt.
pub fn load_preview(photo: &Path) -> Option<(String, DevelopedPreview)> {
    if !crate::sidecar::path_for(photo).is_some_and(|p| p.is_file()) {
        for suffix in ["preview.jpg", "preview.json"] {
            if let Some(f) = file(photo, suffix) {
                let _ = std::fs::remove_file(f);
            }
        }
        return None;
    }
    let meta: PreviewMeta = serde_json::from_str(&std::fs::read_to_string(file(photo, "preview.json")?).ok()?).ok()?;
    let jpeg = std::fs::read(file(photo, "preview.jpg")?).ok()?;
    let image_base64 = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg));
    Some((meta.key, DevelopedPreview { image_base64, ..meta.preview }))
}

/// Just the settings key the last engine picture was rendered with, and the photo's as-shot white
/// balance it read, without the picture or touching anything: whether it's still current. None
/// without an .rpd (the picture belongs to it, as `load_preview` treats it).
pub fn preview_key(photo: &Path) -> Option<(String, Option<f32>, Option<f32>)> {
    if !crate::sidecar::path_for(photo).is_some_and(|p| p.is_file()) {
        return None;
    }
    let meta: PreviewMeta = serde_json::from_str(&std::fs::read_to_string(file(photo, "preview.json")?).ok()?).ok()?;
    Some((meta.key, meta.preview.as_shot_temperature, meta.preview.as_shot_tint))
}

/// Just the last engine picture's JPEG (a photo with edits only): the film strip's thumbnail, so it
/// shows the photo as edited (masks, crop, grade) after a reopen, not the camera's own picture.
pub fn preview_jpeg(photo: &Path) -> Option<Vec<u8>> {
    // An edit of RapidOne's, or (none started in RapidOne yet) a Lightroom edit in its .xmp, which
    // RapidOne renders. Reset to no edit (the .rpd keeps an empty edit): the camera's picture.
    let rapidone = crate::sidecar::has_edit(photo);
    if !rapidone && (!crate::sidecar::lightroom_applies(photo) || crate::preset::for_photo(photo).is_none()) {
        return None;
    }
    std::fs::read(file(photo, "preview.jpg")?).ok()
}

/// Whether the photo's last engine picture is still its current one: rendered after its edit was
/// last saved (every edit change rewrites the .rpd). For the "rendered" sparkle after a reopen.
pub fn preview_current(photo: &Path) -> bool {
    let modified = |p: Option<PathBuf>| p.and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
    match (modified(file(photo, "preview.json")), modified(crate::sidecar::path_for(photo))) {
        (Some(picture), Some(edit)) => picture >= edit,
        // No .rpd: the picture isn't the photo's (load_preview drops it).
        _ => false,
    }
}

/// Drops the photo's last engine picture (its edits were reset: the picture is out of date).
pub fn forget_preview(photo: &Path) {
    for suffix in ["preview.jpg", "preview.json"] {
        if let Some(f) = file(photo, suffix) {
            let _ = std::fs::remove_file(f);
        }
    }
}

pub fn save_preview(photo: &Path, key: &str, preview: &DevelopedPreview) -> Result<(), String> {
    let data = preview.image_base64.split_once("base64,").map_or(preview.image_base64.as_str(), |(_, d)| d);
    let jpeg = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    write(&file(photo, "preview.jpg").ok_or("no cache folder")?, &jpeg)?;
    let meta = PreviewMeta { key: key.to_string(), preview: DevelopedPreview { image_base64: String::new(), ..preview.clone() } };
    write(&file(photo, "preview.json").ok_or("no cache folder")?, serde_json::to_string(&meta).map_err(|e| e.to_string())?.as_bytes())
}

pub fn load_mask(photo: &Path) -> Option<Mask> {
    let img = image::open(file(photo, "mask.png")?).ok()?.to_luma8();
    if img.width() != img.height() {
        return None;
    }
    Some(Mask { size: img.width() as usize, data: img.pixels().map(|p| p[0] as f32 / 255.0).collect() })
}

pub fn save_mask(photo: &Path, mask: &Mask) -> Result<(), String> {
    let n = mask.size as u32;
    let img = image::GrayImage::from_fn(n, n, |x, y| image::Luma([(mask.data[(y * n + x) as usize] * 255.0).round() as u8]));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    write(&file(photo, "mask.png").ok_or("no cache folder")?, &png)
}
