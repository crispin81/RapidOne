//! The folder's own cache, so reopening a shoot is instant: kept with the
//! photos in `sidecars/cache/` (next to the `.rpd` files), named after each
//! photo file. Safe to delete at any time; it's rebuilt as photos are viewed.
//!
//! - `<photo>.preview.jpg` + `<photo>.preview.json`: the last engine picture
//!   and the settings it was rendered with, so it's shown at once if the edit
//!   hasn't changed since.
//! - `<photo>.mask.png`: the subject mask (`mask.rs`), about 3 s saved.
//! - `<photo>.depth.png`: the depth map's control values (`scene.rs`/`depth.rs`: 16-bit, 1 = nearest,
//!   sky 0), at the model's own size, with `<photo>.depth.version` beside it: phase 1 wrote a
//!   *relative* map under the same name, and its version says which layout the file has.
//! - `<photo>.scene.bin`: the scene geometry (`scene.rs`: metric distances, normals, sky, focal,
//!   field of view, anchors), at the model's own size.

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

/// The engine's built-in look (`Tuning::adobe`'s display curves and the like): raised whenever it
/// changes, so pictures cached with the old one are rendered again rather than shown (their settings
/// key, the app's, doesn't change with the engine). 1: the S curve halved; 2: Yellow +10 -> +5 (2026-11-01). 8: one clipped channel keeps its colour (2026-11-01). 9: two clipped channels ease to neutral from 78%, not brightened (2026-11-11). 10: clipped channels estimated from the others (highlights.rs, 2026-11-12). 3: Colour noise reduction; 4, 5: its strength and method; 6: it follows edges, and sharpening keeps colour; 7: sharpening refitted, 2026-11-01.
const ENGINE_LOOK: u32 = 10;

#[derive(serde::Serialize, serde::Deserialize)]
struct PreviewMeta {
    /// `ENGINE_LOOK` when it was rendered (0: before the stamp).
    #[serde(default)]
    look: u32,
    /// The settings the picture was rendered with (the app's JSON key).
    key: String,
    preview: DevelopedPreview,
}

/// The cached picture's details, if it was rendered with the engine's current look.
fn read_meta(photo: &Path) -> Option<PreviewMeta> {
    let meta: PreviewMeta = serde_json::from_str(&std::fs::read_to_string(file(photo, "preview.json")?).ok()?).ok()?;
    (meta.look == ENGINE_LOOK).then_some(meta)
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
    let meta = read_meta(photo)?;
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
    let meta = read_meta(photo)?;
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
    read_meta(photo)?;
    std::fs::read(file(photo, "preview.jpg")?).ok()
}

/// Whether the photo's last engine picture is still its current one: rendered after its edit was
/// last saved (every edit change rewrites the .rpd). For the "rendered" sparkle after a reopen.
pub fn preview_current(photo: &Path) -> bool {
    if read_meta(photo).is_none() {
        return false;
    }
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
    let meta = PreviewMeta { look: ENGINE_LOOK, key: key.to_string(), preview: DevelopedPreview { image_base64: String::new(), ..preview.clone() } };
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

/// The depth cache's layout: 2 is the control depth (1 = nearest, 0 = farthest, sky 0), which
/// phase 1's relative map (no version file) isn't: those are recomputed once. Beside the version
/// the marker says where it came from - "scene" (MoGe-2's control depth) or "relative" (Depth
/// Anything's fallback) - so the fallback is redone once the scene models are there.
const DEPTH_VERSION: u32 = 2;

/// The depth cache marker's words: `"<version> <source>"`.
fn marker(source: &str) -> String {
    format!("{DEPTH_VERSION} {source}")
}

/// The photo's depth map (`scene.rs`/`depth.rs`), if it's been found before: the control values,
/// 16-bit greyscale (its 518-odd pixels of depth need the range, not just 256 levels).
pub fn load_depth(photo: &Path) -> Option<crate::depth::DepthMap> {
    let marker = std::fs::read_to_string(file(photo, "depth.version")?).ok()?;
    if marker.split_whitespace().next()?.parse::<u32>().ok()? != DEPTH_VERSION {
        return None;
    }
    crate::depth::from_png16(&std::fs::read(file(photo, "depth.png")?).ok()?)
}

pub fn save_depth(photo: &Path, map: &crate::depth::DepthMap) -> Result<(), String> {
    write(&file(photo, "depth.png").ok_or("no cache folder")?, &crate::depth::to_png16(map)?)?;
    write(&file(photo, "depth.version").ok_or("no cache folder")?, marker("scene").as_bytes())
}

/// Depth Anything's relative map as the fallback: as `save_depth`, marked as the fallback.
pub fn save_depth_relative(photo: &Path, map: &crate::depth::DepthMap) -> Result<(), String> {
    write(&file(photo, "depth.png").ok_or("no cache folder")?, &crate::depth::to_png16(map)?)?;
    write(&file(photo, "depth.version").ok_or("no cache folder")?, marker("relative").as_bytes())
}

/// Whether the cached depth map is Depth Anything's fallback (not the scene's control depth).
pub fn depth_is_relative(photo: &Path) -> bool {
    file(photo, "depth.version").is_some_and(|p| std::fs::read_to_string(p).is_ok_and(|m| m.contains("relative")))
}

/// Whether the photo's control depth is cached (in the current layout).
pub fn has_depth(photo: &Path) -> bool {
    file(photo, "depth.version").is_some_and(|p| p.is_file()) && file(photo, "depth.png").is_some_and(|p| p.is_file())
}

/// Whether the photo's scene geometry is cached.
pub fn has_scene(photo: &Path) -> bool {
    file(photo, "scene.bin").is_some_and(|p| p.is_file())
}

/// The photo's scene geometry (`scene.rs`), if it's been found before: MoGe-2's metric points,
/// normals, sky mask, focal and field of view, at the model's own size.
pub fn load_scene(photo: &Path) -> Option<crate::scene::Scene3D> {
    crate::scene::from_bytes(&std::fs::read(file(photo, "scene.bin")?).ok()?)
}

pub fn save_scene(photo: &Path, scene: &crate::scene::Scene3D) -> Result<(), String> {
    write(&file(photo, "scene.bin").ok_or("no cache folder")?, &crate::scene::to_bytes(scene))?;
    // Which Depth quality made it (phase 1c): a photo made at the other quality is made again.
    let quality = if scene.model.contains("DA3") { "high" } else { "standard" };
    write(&file(photo, "scene.quality").ok_or("no cache folder")?, quality.as_bytes())
}

/// The Depth quality the photo's scene was made at ("standard" or "high"), if it says.
pub fn scene_quality(photo: &Path) -> Option<String> {
    std::fs::read_to_string(file(photo, "scene.quality")?).ok().map(|s| s.trim().to_string())
}

/// Records the quality a photo's depth was made at (the fallback map has no scene to ask).
pub fn save_scene_quality(photo: &Path, quality: &str) -> Result<(), String> {
    write(&file(photo, "scene.quality").ok_or("no cache folder")?, quality.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_depth_cache_file_round_trips_through_the_folder() {
        let dir = std::env::temp_dir().join(format!("rapidone-depth-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("P1000123.RAF");
        assert!(load_depth(&photo).is_none(), "nothing cached yet");
        let map = crate::depth::DepthMap { w: 4, h: 3, data: (0..12).map(|i| i as f32 / 11.0).collect() };
        save_depth(&photo, &map).unwrap();
        assert!(photo.with_file_name("sidecars").join("cache").join("P1000123.RAF.depth.png").is_file());
        let back = load_depth(&photo).unwrap();
        assert_eq!((back.w, back.h), (4, 3));
        for (a, b) in map.data.iter().zip(&back.data) {
            assert!((a - b).abs() < 1e-4);
        }
        // A file from phase 1 (no version, or another one) is ignored, so it's recomputed once.
        let version = photo.with_file_name("sidecars").join("cache").join("P1000123.RAF.depth.version");
        std::fs::write(&version, b"1").unwrap();
        assert!(load_depth(&photo).is_none());
        std::fs::remove_file(&version).unwrap();
        assert!(load_depth(&photo).is_none());
        // The fallback's marker: the same layout, but it says so (so it's redone when the scene
        // models are there), and the scene's own is the preferred one.
        assert!(!depth_is_relative(&photo), "no marker: nothing to upgrade");
        save_depth_relative(&photo, &map).unwrap();
        assert!(load_depth(&photo).is_some() && depth_is_relative(&photo));
        save_depth(&photo, &map).unwrap();
        assert!(!depth_is_relative(&photo));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scene_cache_file_round_trips_through_the_folder() {
        let dir = std::env::temp_dir().join(format!("rapidone-scene-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("DSC_0094.NEF");
        assert!(load_scene(&photo).is_none(), "nothing cached yet");
        let scene = crate::scene::Scene3D {
            w: 2,
            h: 2,
            distance: vec![1.5, 2.0, 3.0, f32::INFINITY],
            control: crate::gradient::Grid { w: 2, h: 2, data: vec![1.0, 0.6, 0.2, 0.0] },
            normals: std::sync::Arc::new(vec![[0.0, 0.0, 1.0]; 4]),
            sky: crate::gradient::Grid { w: 2, h: 2, data: vec![0.0, 0.0, 0.0, 1.0] },
            fov_x_deg: 74.0,
            focal_px: 620.0,
            d_near: 1.4,
            d_far: 9.0,
            model: "MoGe-2 ViT-S",
            detail: None,
            edges: std::sync::OnceLock::new(),
        };
        save_scene(&photo, &scene).unwrap();
        assert!(photo.with_file_name("sidecars").join("cache").join("DSC_0094.NEF.scene.bin").is_file());
        let back = load_scene(&photo).expect("its own file");
        assert_eq!((back.w, back.h, back.model), (2, 2, "MoGe-2 ViT-S"));
        assert!((back.fov_x_deg - 74.0).abs() < 1e-4 && (back.focal_px - 620.0).abs() < 0.5);
        assert!(back.distance[3].is_infinite() && (back.distance[1] - 2.0).abs() < 0.01);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
