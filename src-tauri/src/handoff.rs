//! The hand-off to RapidRetouch: the photo as graded in Edit, at full
//! resolution and 16 bits per channel, as a TIFF in the folder's
//! `sidecars/handoff/` (named after the photo, so RapidRetouch keeps its own
//! settings for it across re-grades). Rendered again only when the grade
//! (the app's settings key) has changed.
//!
//! sRGB, as the viewer shows it, untagged (TIFF readers take that as sRGB).
//! Deflate with a horizontal predictor (lossless; only ~12% smaller on 16-bit
//! photos, whose noise barely compresses): RapidRetouch reads TIFFs with
//! tifffile, which decodes both with Python's own zlib and numpy (no extra
//! codec package needed).

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use tiff::encoder::{colortype::RGB16, Compression, DeflateLevel, Predictor, TiffEncoder};

/// Clears the hand-off TIFFs (and their keys) of the shoots in `folders` - each
/// folder, and its `rated` / `unrated` views if it's an organised shoot. They're
/// only a cache (about 100 MB each); one is rendered again the next time its
/// photo goes to Retouch. The retouched TIFFs handed back are the photos' own
/// files, beside them, and aren't touched. Returns the bytes freed.
pub fn clear(folders: &[PathBuf]) -> u64 {
    let mut freed = 0;
    let mut dirs = std::collections::BTreeSet::new();
    for f in folders {
        let root = crate::library::shoot_root(f);
        for d in [root.clone(), root.join("rated"), root.join("unrated"), f.clone()] {
            dirs.insert(d.join(crate::sidecar::FOLDER).join("handoff"));
        }
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if p.is_file() && (name.ends_with(".tif") || name.ends_with(".key") || name.ends_with(".tif.tmp")) {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&p).is_ok() {
                    freed += size;
                }
            }
        }
        // The folder itself, if nothing else is in it.
        let _ = std::fs::remove_dir(&dir);
    }
    freed
}

/// Larger than any sensor: develop at full resolution.
const FULL_RESOLUTION: usize = 1 << 20;

fn files(photo: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = photo.file_name()?.to_string_lossy().to_string();
    let dir = photo.parent()?.join("sidecars").join("handoff");
    Some((dir.join(format!("{name}.tif")), dir.join(format!("{name}.key"))))
}

/// The hand-off TIFF for `photo` graded with `settings`, rendered if it's
/// missing or was rendered with other settings. Returns its path.
pub fn render(photo: &Path, settings: &crate::pipeline::RenderSettings, key: &str) -> Result<PathBuf, String> {
    let (tif, key_file) = files(photo).ok_or("no folder for the hand-off")?;
    if tif.exists() && std::fs::read_to_string(&key_file).ok().as_deref() == Some(key) {
        return Ok(tif);
    }
    let scene = crate::develop::develop_scene_export(photo, FULL_RESOLUTION)?;
    // The subject mask the viewer found (the folder's cache), not found again
    // on the full-size picture; it's in fractions of the frame, so any size fits.
    if let Some(mask) = crate::diskcache::load_mask(photo) {
        let _ = scene.subject_mask.set(Some(std::sync::Arc::new(mask)));
    }
    let img = crate::commands::render_image16(&scene, settings);
    std::fs::create_dir_all(tif.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
    // Through a temporary file and a rename, so RapidRetouch never reads half a file.
    let tmp = tif.with_extension("tif.tmp");
    {
        let out = BufWriter::new(File::create(&tmp).map_err(|e| e.to_string())?);
        let mut encoder = TiffEncoder::new(out)
            .map_err(|e| e.to_string())?
            .with_compression(Compression::Deflate(DeflateLevel::Fast))
            .with_predictor(Predictor::Horizontal);
        encoder
            .write_image::<RGB16>(img.width(), img.height(), img.as_raw())
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &tif).map_err(|e| e.to_string())?;
    std::fs::write(&key_file, key).map_err(|e| e.to_string())?;
    Ok(tif)
}

#[cfg(test)]
mod clear_tests {
    use super::*;

    #[test]
    fn clearing_removes_hand_off_tiffs_in_the_shoot_and_its_views_and_nothing_else() {
        let root = std::env::temp_dir().join(format!("rapidone-handoff-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for view in ["", "rated", "unrated"] {
            let h = root.join(view).join("sidecars").join("handoff");
            std::fs::create_dir_all(&h).unwrap();
            std::fs::write(h.join("A.NEF.tif"), vec![0u8; 10]).unwrap();
            std::fs::write(h.join("A.NEF.key"), b"k").unwrap();
        }
        // A retouched TIFF beside the photos and an .rpd: kept.
        std::fs::write(root.join("rated").join("A_retouched.tif"), b"t").unwrap();
        std::fs::write(root.join("rated").join("sidecars").join("A.NEF.rpd"), b"r").unwrap();
        let freed = clear(&[root.join("rated")]);
        assert_eq!(freed, 33);
        for view in ["", "rated", "unrated"] {
            assert!(!root.join(view).join("sidecars").join("handoff").exists());
        }
        assert!(root.join("rated").join("A_retouched.tif").is_file());
        assert!(root.join("rated").join("sidecars").join("A.NEF.rpd").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
