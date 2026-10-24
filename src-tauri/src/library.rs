use std::path::{Path, PathBuf};

const JPEG_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "JPG", "JPEG"];

#[derive(Debug)]
pub struct FoundPhoto {
    pub raw_path: PathBuf,
    pub jpeg_path: Option<PathBuf>,
}

/// The subfolders Organise sorts a shoot into.
pub const VIEW_FOLDERS: [&str; 2] = ["rated", "unrated"];

/// The shoot's own folder for `folder`: itself, or its parent when it is a
/// shoot's "rated" / "unrated" subfolder.
pub fn shoot_root(folder: &Path) -> PathBuf {
    let is_view = folder
        .file_name()
        .map(|n| VIEW_FOLDERS.iter().any(|v| n.to_string_lossy().eq_ignore_ascii_case(v)))
        .unwrap_or(false);
    match (is_view, folder.parent()) {
        (true, Some(parent)) => parent.to_path_buf(),
        _ => folder.to_path_buf(),
    }
}

/// Lists the photos of `folder`: those directly inside it, and - for a
/// shoot that has been organised - those in its "rated" and "unrated"
/// subfolders too (the "All" view; capture time orders them together).
pub fn find_photos(folder: &Path) -> Result<Vec<FoundPhoto>, String> {
    let mut photos = find_photos_in(folder)?;
    let is_view = shoot_root(folder) != folder;
    if !is_view {
        for name in VIEW_FOLDERS {
            let sub = folder.join(name);
            if sub.is_dir() {
                photos.extend(find_photos_in(&sub)?);
            }
        }
    }
    Ok(photos)
}

/// Lists photos directly inside `folder` (non-recursive): RAW files, each
/// paired with any same-basename JPEG sibling, plus any standalone JPEG that
/// has no RAW sibling - for photographers who shoot JPEG-only.
fn find_photos_in(folder: &Path) -> Result<Vec<FoundPhoto>, String> {
    let raw_extensions = rawler::decoders::supported_extensions();
    let entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();

    let is_jpeg = |p: &Path| -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
            .unwrap_or(false)
    };

    let is_tiff = |p: &Path| -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"))
            .unwrap_or(false)
    };

    let mut photos = Vec::new();
    let mut claimed_jpegs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for path in &entries {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext_upper = ext.to_uppercase();
        if !raw_extensions.contains(&ext_upper.as_str()) {
            continue;
        }

        let jpeg_path = JPEG_EXTENSIONS
            .iter()
            .map(|jpeg_ext| path.with_extension(jpeg_ext))
            .find(|candidate| candidate.is_file());
        if let Some(jp) = &jpeg_path {
            claimed_jpegs.insert(jp.clone());
        }

        photos.push(FoundPhoto {
            raw_path: path.clone(),
            jpeg_path,
        });
    }

    // A standalone JPEG is the photo in its own right: raw_path and
    // jpeg_path both point at the same file, so preview extraction takes
    // the JPEG branch (see extract_source_image) while every other
    // identity-derived piece (xmp sidecar, filename, move destination)
    // stays correctly scoped to this one file.
    for path in &entries {
        if is_jpeg(path) && !claimed_jpegs.contains(path) {
            photos.push(FoundPhoto {
                raw_path: path.clone(),
                jpeg_path: Some(path.clone()),
            });
        }
    }

    // A TIFF is a photo in its own right too (like a JPEG-only shoot): the
    // TIFF Retouch hands back, or one made elsewhere. Edit opens it through
    // `imagefile`, already toned (the "As is" profile).
    for path in &entries {
        if is_tiff(path) {
            photos.push(FoundPhoto {
                raw_path: path.clone(),
                jpeg_path: Some(path.clone()),
            });
        }
    }

    Ok(photos)
}
