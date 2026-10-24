use crate::library;
use crate::preview;
use crate::types::{FullPreview, PhotoInfo, ScanProgress};
use crate::xmp::{self, RatingFlags};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Long edge of filmstrip thumbnails. The filmstrip is a narrow column, so
/// this is smaller than RapidCulling's grid thumbnails.
const FILMSTRIP_THUMB_MAX_DIM: u32 = 480;
/// Long edge of the viewer image until real RAW development lands.
const FULL_PREVIEW_MAX_DIM: u32 = 2400;

/// Scans the working folder, emitting `photo-ready` per photo as thumbnails
/// finish (in no particular order - the frontend sorts) and returning the
/// paths that failed to decode.
#[tauri::command]
pub async fn scan_folder(app: tauri::AppHandle, folder: String) -> Result<Vec<String>, String> {
    let found = library::find_photos(Path::new(&folder))?;
    let total = found.len();
    let _ = app.emit("scan-progress", ScanProgress { done: 0, total });
    // In the phone app's list of folders (RapidCull for Android).
    tauri_plugin_rapidculling::phone_app::note_recent(Path::new(&folder));

    // Two cores left free, so the rest of the computer (and the app's own window) stays quick while a folder is read.
    let thread_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).max(1);
    let chunk_size = found.len().div_ceil(thread_count).max(1);
    let progress = Arc::new(AtomicUsize::new(0));
    let failed_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for chunk in found.chunks(chunk_size) {
            let app = app.clone();
            let progress = progress.clone();
            let failed_paths = failed_paths.clone();
            scope.spawn(move || {
                tauri_plugin_rapidculling::priority::background();
                for photo in chunk {
                    match process_photo(&photo.raw_path, photo.jpeg_path.as_deref()) {
                        Ok(info) => {
                            let _ = app.emit("photo-ready", &info);
                        }
                        Err(_) => failed_paths
                            .lock()
                            .unwrap()
                            .push(photo.raw_path.to_string_lossy().to_string()),
                    }
                    let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = app.emit("scan-progress", ScanProgress { done, total });
                }
            });
        }
    });

    // Cull's entries kept during this read are on disk before Cull reads the folder.
    tauri_plugin_rapidculling::cache::flush();
    Ok(Arc::try_unwrap(failed_paths).unwrap().into_inner().unwrap())
}

fn process_photo(raw_path: &Path, jpeg_path: Option<&Path>) -> Result<PhotoInfo, String> {
    use tauri_plugin_rapidculling::{cache, preview as cull_preview};
    // Shared with Cull (its cache, cache.rs in the module): each photo's camera JPEG is pulled out of
    // the RAW and decoded once for both. Kept already: the film strip's thumbnail from its 1,200 px
    // preview. Not yet: decoded here, and Cull's entry kept too, so its scan then finds it ready.
    // Read once for both (cache::load_or_make): kept already, or Cull reading it this moment, or
    // decoded here now: the full picture shrunk once to Cull's 1,200 px and let go (no full-size
    // copies), Cull's fingerprint taken from the full one as its own scan does.
    let kept = cache::load_or_make(raw_path, || {
        let extracted = preview::extract_source_image(raw_path, jpeg_path)?;
        let dhash = cull_preview::compute_dhash(&extracted.image);
        let small = {
            let (w, h) = image::GenericImageView::dimensions(&extracted.image);
            if w > 1200 || h > 1200 { extracted.image.resize(1200, 1200, image::imageops::FilterType::Triangle) } else { extracted.image }
        };
        let (jpeg, width, height) = cull_preview::make_thumb_jpeg_sized(&small, extracted.orientation, 1200, 75)?;
        Ok(cache::Entry { timestamp_ms: extracted.timestamp_ms, orientation: extracted.orientation, width, height, dhash, jpeg, eyes: None })
    })?;
    // The film strip's thumbnail: the photo as last rendered with its edits, if it has any (kept in
    // the folder's cache), else from the kept camera preview (already the right way up).
    let edited = crate::diskcache::preview_jpeg(raw_path).and_then(|j| image::load_from_memory(&j).ok());
    let img = match edited {
        Some(img) => img,
        None => image::load_from_memory(&kept.jpeg).map_err(|e| e.to_string())?,
    };
    let (thumb_base64, _, _) = preview::make_thumb_base64(&img, 1, FILMSTRIP_THUMB_MAX_DIM)?;
    let timestamp_ms = kept.timestamp_ms;
    // Stars and pick from the photo's .rpd, else (not rated in RapidOne yet) its .xmp.
    let flags = crate::sidecar::flags(raw_path)
        .map(|(rating, pick)| RatingFlags { rating, pick })
        .unwrap_or_else(|| xmp::read(&xmp::sidecar_path_for(raw_path)));

    Ok(PhotoInfo {
        path: raw_path.to_string_lossy().to_string(),
        file_name: raw_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        timestamp_ms,
        thumb_base64,
        rating: flags.rating,
        pick: flags.pick,
        rendered: crate::diskcache::preview_current(raw_path),
        edited: photo_edited(raw_path),
        look: crate::sidecar::look(raw_path).or_else(|| {
            (crate::sidecar::lightroom_applies(raw_path) && crate::preset::for_photo(raw_path).is_some())
                .then(|| serde_json::json!({ "from": "xmp", "name": "Lightroom XMP" }))
        }),
        denoised: crate::sidecar::denoised(raw_path),
    })
}

/// The ready-made picture for a photo's quick preview: a TIFF or JPEG is its
/// own picture; a RAW uses its same-name JPEG if it has one (else its embedded preview).
fn picture_file(photo: &Path) -> Option<PathBuf> {
    if crate::imagefile::is_image_file(photo) {
        return Some(photo.to_path_buf());
    }
    ["jpg", "jpeg", "JPG", "JPEG"].iter().map(|ext| photo.with_extension(ext)).find(|p| p.is_file())
}

/// Clears the hand-off TIFFs of these folders' shoots (`handoff::clear`), on closing. Returns the bytes freed.
#[tauri::command]
pub async fn clear_handoffs(folders: Vec<String>) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || crate::handoff::clear(&folders.iter().map(PathBuf::from).collect::<Vec<_>>()))
        .await
        .map_err(|e| e.to_string())
}

/// Sending a photo to the trash moves it to `sidecars/trash/` in its own folder
/// (beside `cache` and `handoff`), with its Lightroom `.xmp` and its `.rpd`. Our
/// own folder, not the system's: Windows has no Recycle Bin on memory cards,
/// many USB sticks or network drives and deletes for good there; a move within
/// the folder never deletes anything, is instant, and is the same on every
/// system. To restore: the photo and .xmp back to the folder, the .rpd back to
/// `sidecars`. A name already in the trash gets " (2)", " (3)"...
pub fn trash_folder(photo_dir: &Path) -> PathBuf {
    photo_dir.join(crate::sidecar::FOLDER).join("trash")
}

/// Whether a photo has an edit, as the thumbnail's "edited" icon shows it: one of RapidOne's in its
/// .rpd, or (none started in RapidOne yet) a Lightroom edit in its .xmp, which RapidOne reads and
/// renders; reset to no edit (the .rpd keeps an empty edit), none.
fn photo_edited(photo: &Path) -> bool {
    if crate::sidecar::has_edit(photo) {
        return true;
    }
    crate::sidecar::lightroom_applies(photo) && crate::preset::for_photo(photo).is_some()
}

/// A photo sent to the trash: where it was, and where it is now (its name there can have " (2)").
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Trashed {
    pub path: String,
    pub trashed: String,
}

/// Sends photos to the trash (`trash_folder`). Returns the photos moved, with where each went.
#[tauri::command]
pub async fn trash_photos(paths: Vec<String>) -> Result<Vec<Trashed>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut moved = Vec::new();
        for path in &paths {
            let photo = PathBuf::from(path);
            if !photo.is_file() {
                continue;
            }
            let trashed = trash_one(&photo).map_err(|e| format!("{}: {e}", photo.display()))?;
            moved.push(Trashed { path: path.clone(), trashed: trashed.to_string_lossy().to_string() });
        }
        Ok(moved)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Puts photos back from the trash (Undo after a delete): each photo where it was, its .xmp beside
/// it and its .rpd back in `sidecars`, as `trash_one` moved them. A photo whose place has been taken
/// meanwhile is left in the trash. Returns the photos put back.
#[tauri::command]
pub async fn restore_photos(items: Vec<Trashed>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut back = Vec::new();
        for item in &items {
            let (photo, trashed) = (PathBuf::from(&item.path), PathBuf::from(&item.trashed));
            if photo.exists() || !trashed.is_file() {
                continue;
            }
            let trash = trashed.parent().ok_or("no folder")?;
            let name = trashed.file_stem().ok_or("no name")?.to_string_lossy().to_string();
            let ext = trashed.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            std::fs::rename(&trashed, &photo).map_err(|e| format!("{}: {e}", photo.display()))?;
            // The photo is back; its companions follow.
            let xmp = trash.join(format!("{name}.xmp"));
            if xmp.is_file() {
                let _ = std::fs::rename(&xmp, photo.with_extension("xmp"));
            }
            let rpd = trash.join(format!("{name}{ext}.rpd"));
            if let (true, Some(to)) = (rpd.is_file(), crate::sidecar::path_for(&photo)) {
                let _ = std::fs::rename(&rpd, to);
            }
            back.push(item.path.clone());
        }
        Ok(back)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Moves one photo (and its .xmp and .rpd) into the trash; where the photo went.
fn trash_one(photo: &Path) -> Result<PathBuf, String> {
    let trash = trash_folder(photo.parent().ok_or("no folder")?);
    let stem = photo.file_stem().ok_or("no name")?.to_string_lossy().to_string();
    let ext = photo.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let xmp = photo.with_extension("xmp");
    let rpd = crate::sidecar::path_for(photo).filter(|p| p.is_file());
    // A name free for the photo, its .xmp and its .rpd alike.
    let name_for = |n: u32| if n == 1 { stem.clone() } else { format!("{stem} ({n})") };
    let free = |n: u32| {
        let s = name_for(n);
        ["", ".rpd"].iter().all(|tail| !trash.join(format!("{s}{ext}{tail}")).exists()) && !trash.join(format!("{s}.xmp")).exists()
    };
    let name = name_for((1..).find(|&n| free(n)).unwrap_or(1));
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    std::fs::rename(photo, trash.join(format!("{name}{ext}"))).map_err(|e| e.to_string())?;
    // The photo is in the trash now; its companions follow.
    if xmp.is_file() {
        let _ = std::fs::rename(&xmp, trash.join(format!("{name}.xmp")));
    }
    if let Some(rpd) = rpd {
        let _ = std::fs::rename(&rpd, trash.join(format!("{name}{ext}.rpd")));
    }
    Ok(trash.join(format!("{name}{ext}")))
}

/// One more photo for the library (the TIFF Retouch has just handed back):
/// its thumbnail, stars and capture time, as the folder scan makes them. A
/// TIFF or JPEG is its own source image.
#[tauri::command]
pub async fn add_photo(path: String) -> Result<PhotoInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(&path);
        process_photo(&p, Some(&p))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_full_preview(path: String) -> Result<FullPreview, String> {
    let raw_path = PathBuf::from(&path);
    let jpeg_candidate = picture_file(&raw_path);

    // Embedded preview / sibling JPEG for now. Real RAW development (demosaic
    // + the adjustment pipeline) replaces this in a later step.
    let extracted = preview::extract_source_image(&raw_path, jpeg_candidate.as_deref())?;
    let (image_base64, width, height) =
        preview::make_thumb_base64(&extracted.image, extracted.orientation, FULL_PREVIEW_MAX_DIM)?;

    Ok(FullPreview {
        path,
        image_base64,
        width,
        height,
    })
}

// Ratings and picks use RapidCulling's exact sidecar format, so a folder
// culled in either app shows the same stars/picks/rejects in the other.

/// Changes a photo's stars/pick: in its .rpd (RapidOne's rule; standalone
/// RapidCulling keeps them in the .xmp), starting from what it has now.
fn update_flags(path: &str, update: impl FnOnce(&mut RatingFlags)) -> Result<(), String> {
    let photo = PathBuf::from(path);
    let mut flags = crate::sidecar::flags(&photo)
        .map(|(rating, pick)| RatingFlags { rating, pick })
        .unwrap_or_else(|| xmp::read(&xmp::sidecar_path_for(&photo)));
    update(&mut flags);
    crate::sidecar::set_flags(&photo, flags.rating, flags.pick)
}

#[tauri::command]
pub async fn set_rating(path: String, rating: i8) -> Result<(), String> {
    update_flags(&path, |f| f.rating = rating)
}

#[tauri::command]
pub async fn set_pick(path: String, pick: bool) -> Result<(), String> {
    update_flags(&path, |f| f.pick = pick)
}

/// Changing both at once must be one read-modify-write, or two concurrent
/// calls can race and leave a photo both picked and rejected.
#[tauri::command]
pub async fn set_rating_and_pick(path: String, rating: i8, pick: bool) -> Result<(), String> {
    update_flags(&path, |f| {
        f.rating = rating;
        f.pick = pick;
    })
}

/// Measures how much to rotate and keystone-correct a photo (see
/// `transform.rs`). Runs on the embedded preview / sibling JPEG, upright.
#[tauri::command]
pub async fn auto_transform(path: String) -> Result<AutoTransformed, String> {
    // At background priority (several photos measured at once while "Level all" goes through a
    // folder): the photo on screen renders first.
    tauri::async_runtime::spawn_blocking(move || background_pool().install(|| auto_transform_of(&path)))
        .await
        .map_err(|e| e.to_string())?
}

fn auto_transform_of(path: &str) -> Result<AutoTransformed, String> {
    let raw_path = PathBuf::from(path);
    let jpeg_candidate = picture_file(&raw_path);
    let extracted = preview::extract_source_image(&raw_path, jpeg_candidate.as_deref())?;
    let upright = preview::apply_orientation(extracted.image, extracted.orientation);
    // Focal length is assumed to equal the long edge (~36mm full-frame
    // equivalent). The preview's perspective correction uses the same
    // assumption, so the lines still come out straight; only the reported
    // tilt in degrees is nominal. Reading the real focal length from EXIF is
    // a later refinement.
    let auto = crate::transform::analyse(&upright, 1.0);
    // The Scale that hides the white the warp uncovers, for Level and for Level + keystone: the
    // sliders as the app sets them from the measurement (transform.ts's roundRotate, tiltToVertical).
    let (w, h) = (upright.width() as f32, upright.height() as f32);
    let turn = |deg: f32| (deg.clamp(-45.0, 45.0) * 10.0).round() / 10.0;
    let vertical = auto.vertical_tilt_deg.map_or(0.0, |t| (t / 0.4).clamp(-100.0, 100.0).round());
    // Level turns by its own angle (horizontals too); keystone by the verticals' with the tilt.
    let level = crate::keystone::Keystone { rotate: turn(auto.rotate_level_deg), ..Default::default() };
    let keystone = crate::keystone::Keystone { rotate: turn(auto.rotate_deg), vertical, ..Default::default() };
    Ok(AutoTransformed {
        cover_scale_level: level.cover_scale(w, h),
        cover_scale_keystone: keystone.cover_scale(w, h),
        auto,
    })
}

/// The auto measurement, and the Scale that crops out the white after each way of applying it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTransformed {
    #[serde(flatten)]
    pub auto: crate::transform::AutoTransform,
    pub cover_scale_level: f32,
    pub cover_scale_keystone: f32,
}

/// What the viewer shows from the RAW engine: our own development (not the
/// camera's embedded JPEG), plus timings so speed is visible while building it.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DevelopedPreview {
    pub path: String,
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
    pub camera: String,
    pub decode_ms: u128,
    pub develop_ms: u128,
    /// The exposure this image was rendered at, so the viewer can bridge the
    /// gap with a quick approximation while the next render is on its way.
    pub exposure: f32,
    /// The photo's as-shot white balance in Lightroom's numbers (Kelvin, tint),
    /// for the Temperature/Tint sliders' starting point. None without matrices.
    pub as_shot_temperature: Option<f32>,
    pub as_shot_tint: Option<f32>,
    /// Full-resolution pixels per pixel of this preview (the sensor's long
    /// edge over the developed scene's), so the viewer knows what 100% is.
    #[serde(default)]
    pub full_scale: f32,
}

/// Recently developed photos, kept as scene-referred images so slider moves
/// re-render from real RAW data without decoding again.
#[derive(Default)]
pub struct SceneCache {
    entries: Mutex<Vec<(String, Arc<crate::develop::Scene>)>>,
    /// Smaller copies (by photo and long side), for renders while a slider
    /// moves on a screen showing the photo smaller than the preview.
    small: Mutex<Vec<((String, usize), Arc<crate::develop::Scene>)>>,
}

/// The current photo and its neighbours either side (prefetched), with room
/// to step back and forth; each costs about 50 MB at screen size.
const SCENE_CACHE_SIZE: usize = 6;

impl SceneCache {
    /// The photo at most `long` on its long side (made once from the cached
    /// one); the cached one itself when that is already no bigger.
    fn get_small(&self, path: &str, long: usize) -> Result<Arc<crate::develop::Scene>, String> {
        let scene = self.get_or_develop(path)?;
        let full = scene.image.width.max(scene.image.height);
        if long >= full {
            return Ok(scene);
        }
        let key = (path.to_string(), long);
        if let Some((_, s)) = self.small.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return Ok(s.clone());
        }
        let small = Arc::new(crate::develop::Scene {
            image: scene.image.downscaled(long),
            camera: scene.camera.clone(),
            make: scene.make.clone(),
            wb: scene.wb,
            exposure_offset: scene.exposure_offset,
            display_referred: scene.display_referred,
            lens: scene.lens.clone(),
            decode_ms: 0,
            develop_ms: 0,
            // The whole photo's subject mask, if found already (it's the same frame).
            subject_mask: scene.subject_mask.get().cloned().map(std::sync::OnceLock::from).unwrap_or_default(),
            subject_refined: scene.subject_refined.get().cloned().map(std::sync::OnceLock::from).unwrap_or_default(),
            denoised: Default::default(),
        });
        let mut small_entries = self.small.lock().unwrap();
        small_entries.retain(|(k, _)| *k != key);
        small_entries.push((key, small.clone()));
        if small_entries.len() > 3 {
            small_entries.remove(0);
        }
        Ok(small)
    }

    fn get_or_develop(&self, path: &str) -> Result<Arc<crate::develop::Scene>, String> {
        {
            // A hit moves to the back, so the least recently used goes first.
            let mut entries = self.entries.lock().unwrap();
            if let Some(i) = entries.iter().position(|(p, _)| p == path) {
                let hit = entries.remove(i);
                let scene = hit.1.clone();
                entries.push(hit);
                return Ok(scene);
            }
        }
        let scene = Arc::new(crate::develop::develop_scene(Path::new(path), FULL_PREVIEW_MAX_DIM as usize)?);
        // Its subject mask, if found before (kept in the folder's cache), so subject masks render at once.
        if let Some(m) = crate::diskcache::load_mask(Path::new(path)) {
            let _ = scene.subject_mask.set(Some(Arc::new(m)));
        }
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|(p, _)| p != path);
        entries.push((path.to_string(), scene.clone()));
        if entries.len() > SCENE_CACHE_SIZE {
            entries.remove(0);
        }
        Ok(scene)
    }
}

fn render_scene(
    path: String,
    scene: &crate::develop::Scene,
    settings: &crate::pipeline::RenderSettings,
) -> Result<DevelopedPreview, String> {
    let img = render_image(scene, settings);
    preview_of(path, scene, settings, &img)
}

/// The viewer's picture of a rendered photo.
fn preview_of(
    path: String,
    scene: &crate::develop::Scene,
    settings: &crate::pipeline::RenderSettings,
    img: &image::DynamicImage,
) -> Result<DevelopedPreview, String> {
    let (image_base64, width, height) = preview::make_thumb_base64(img, 1, FULL_PREVIEW_MAX_DIM)?;
    Ok(DevelopedPreview {
        path,
        image_base64,
        width,
        height,
        camera: scene.camera.clone(),
        decode_ms: scene.decode_ms,
        develop_ms: scene.develop_ms,
        exposure: settings.exposure,
        // A TIFF or JPEG has no camera white balance: neutral is 5500 K, tint 0,
        // so Temperature and Tint work on it as a warm / cool slider.
        as_shot_temperature: scene.wb.map(|w| w.shot_kelvin).or(scene.display_referred.then_some(crate::pipeline::NEUTRAL_KELVIN)),
        as_shot_tint: scene.wb.map(|w| w.shot_tint).or(scene.display_referred.then_some(0.0)),
        // Sensor pixels per scene pixel, times scene pixels per sent pixel (the
        // render is sent scaled down only if it's over the preview size).
        full_scale: scene.image.source_long as f32 / scene.image.width.max(scene.image.height).max(1) as f32
            * (img.width().max(img.height()) as f32 / width.max(height).max(1) as f32),
    })
}

/// A developed photo rendered with these settings, exactly as the viewer
/// shows it (profile, per-camera corrections and all).
pub fn render_image(scene: &crate::develop::Scene, settings: &crate::pipeline::RenderSettings) -> image::DynamicImage {
    render_image_depth(scene, settings, false)
}

/// As `render_image`, 16 bits per channel (the hand-off to RapidRetouch).
pub fn render_image16(
    scene: &crate::develop::Scene,
    settings: &crate::pipeline::RenderSettings,
) -> image::ImageBuffer<image::Rgb<u16>, Vec<u16>> {
    match render_image_depth(scene, settings, true) {
        image::DynamicImage::ImageRgb16(img) => img,
        other => other.to_rgb16(),
    }
}

fn render_image_depth(scene: &crate::develop::Scene, settings: &crate::pipeline::RenderSettings, sixteen: bool) -> image::DynamicImage {
    use image::DynamicImage::{ImageRgb16, ImageRgb8};
    // Subject-aware vignette: the subject mask (found once per photo, kept with
    // its scene) turned into the vignette's weights.
    let with_map;
    let settings = if settings.tools.adaptive_vignette != 0.0 && settings.tools.vignette_map.is_none() {
        let mask = scene.subject_mask.get_or_init(|| crate::mask::subject(&scene.image).map(Arc::new));
        let mut s = settings.clone();
        s.tools.vignette_map = mask.as_ref().and_then(|m| crate::subject::from_mask(m, scene.image.width, scene.image.height)).map(Arc::new);
        with_map = s;
        &with_map
    } else {
        settings
    };
    // Subject masks: the photo's own subject mask (found once per photo, kept with its scene).
    let with_subject;
    let settings = if settings.subject_mask.is_none() && settings.gradients.iter().any(crate::gradient::needs_subject) {
        let mut s = settings.clone();
        s.subject_mask = scene.subject_refined();
        with_subject = s;
        &with_subject
    } else {
        settings
    };
    let mut img = {
        let _p = crate::prof::span("render (tone + colour)");
        render_image_tone(scene, settings, sixteen)
    };
    // Detail last, on the finished picture: noise reduction, sharpening, grain.
    let t = &settings.tools;
    // The picture's size against the sensor's, for sharpening's radius (a
    // part of the photo: the whole photo's size at the part's scale).
    let long = match &settings.region {
        Some(r) => r.frame.full_w.max(r.frame.full_h),
        None => img.width().max(img.height()) as f32,
    };
    let scale = (long / scene.image.source_long.max(1) as f32).min(1.0);
    if t.sharpen > 0.0 || t.grain > 0.0 {
        let _p = crate::prof::span("sharpen / grain");
        img = match img {
            ImageRgb16(mut rgb) => {
                // Noise is AI noise reduction now, on the scene (render_image_tone).
                crate::detail::apply(&mut rgb, t.sharpen, 0.0, t.grain, scale);
                ImageRgb16(rgb)
            }
            other => {
                let mut rgb = other.to_rgb8();
                crate::detail::apply(&mut rgb, t.sharpen, 0.0, t.grain, scale);
                ImageRgb8(rgb)
            }
        };
    }
    // Crop, straighten and quarter turns last, so the preview is cut as the export is.
    if !settings.crop.is_none() || !settings.keystone.is_none() {
        let _p = crate::prof::span("crop / straighten");
        img = match img {
            ImageRgb16(rgb) => ImageRgb16(crate::crop::apply(rgb, &settings.crop, &settings.keystone)),
            other => ImageRgb8(crate::crop::apply(other.to_rgb8(), &settings.crop, &settings.keystone)),
        };
    }
    img
}

/// The photo's lens correction (Lensfun), when its Lens button is on and its lens is known.
pub fn lens_correction(scene: &crate::develop::Scene, settings: &crate::pipeline::RenderSettings) -> Option<std::sync::Arc<crate::lens::LensCorrection>> {
    if settings.lens_correct <= 0.0 || scene.display_referred {
        return None;
    }
    let shot = scene.lens.as_ref()?;
    let model = scene.camera.strip_prefix(&scene.make).unwrap_or(&scene.camera).trim();
    crate::lensfun::for_photo(&crate::lensfun::PhotoLens { shot, camera_model: model }, settings.lens_id as u32)
}

/// What the Lens button shows for a photo: the lens its EXIF names, and the database's
/// match for it (id, name), if any.
#[derive(serde::Serialize)]
pub struct LensInfo {
    pub exif: Option<String>,
    pub detected: Option<(u32, String)>,
    /// The lens chosen by hand for this EXIF name, if one was (id, name).
    pub chosen: Option<(u32, String)>,
}

#[tauri::command]
pub async fn lens_info(cache: tauri::State<'_, SceneCache>, path: String, lens_id: Option<f64>) -> Result<(LensInfo, Option<String>), String> {
    let scene = cache.get_or_develop(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let model = scene.camera.strip_prefix(&scene.make).unwrap_or(&scene.camera).trim().to_string();
        let info = LensInfo {
            exif: scene.lens.as_ref().map(|l| l.lens.clone()),
            detected: scene.lens.as_ref().and_then(|l| crate::lensfun::detected(l, &model)),
            chosen: scene.lens.as_ref().and_then(|l| crate::lensfun::choice(&l.lens)).and_then(|id| Some((id, crate::lensfun::name_of(id)?))),
        };
        // The chosen lens's name, when one was chosen.
        let chosen = lens_id.filter(|id| *id > 0.0).and_then(|id| crate::lensfun::name_of(id as u32));
        Ok((info, chosen))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Chooses a lens for every photo whose EXIF names this lens (0: back to the database's match).
#[tauri::command]
pub fn lens_choose(exif: String, lens_id: u32) -> Result<(), String> {
    crate::lensfun::choose(&exif, lens_id)
}

/// The user's own lens profiles (Lensfun .xml): (lenses added, problems).
#[tauri::command]
pub fn lens_add_profiles(paths: Vec<String>) -> (Vec<String>, Vec<String>) {
    crate::lensfun::add_own(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())
}

/// Each photo's EXIF lens name ("" if none), read from its metadata only (fast).
#[tauri::command]
pub async fn lens_names(paths: Vec<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use rayon::prelude::*;
        paths
            .par_iter()
            .map(|p| {
                (|| {
                    let src = rawler::rawsource::RawSource::new(std::path::Path::new(p)).ok()?;
                    let dec = rawler::get_decoder(&src).ok()?;
                    dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default()).ok()?.exif.lens_model
                })()
                .unwrap_or_default()
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// The lens database in use: (Lensfun commit, date, lenses).
#[tauri::command]
pub fn lens_db_version() -> (String, String, usize) {
    crate::lensfun::version()
}

/// A newer lens database from Lensfun's GitHub (fetched by the app's weekly check), saved
/// over the last one if every file reads. Returns the lenses it now has.
#[tauri::command]
pub async fn lens_db_save(files: Vec<(String, String)>, sha: String, date: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || crate::lensfun::save_update(files, &sha, &date)).await.map_err(|e| e.to_string())?
}

/// Lenses in the database matching every word typed: (id, name), at most 40.
#[tauri::command]
pub fn lens_search(query: String) -> Vec<(u32, String)> {
    crate::lensfun::search(&query, 40)
}

fn render_image_tone(scene: &crate::develop::Scene, settings: &crate::pipeline::RenderSettings, sixteen: bool) -> image::DynamicImage {
    // AI noise reduction first (Noise, 0..100: how much of the cleaned
    // scene is used), so every other tool works on the cleaned picture.
    let noise = settings.tools.noise / 100.0;
    let cleaned = (noise > 0.0)
        .then(|| scene.denoised.get_or_init(|| crate::denoise::scene(&scene.image).map(std::sync::Arc::new)).clone())
        .flatten()
        .map(|d| {
            if noise >= 1.0 {
                (*d).clone()
            } else {
                use rayon::prelude::*;
                let px = scene.image.px.par_iter().zip(d.px.par_iter()).map(|(o, c)| std::array::from_fn(|k| o[k] + (c[k] - o[k]) * noise)).collect();
                crate::pipeline::SceneImage { px, ..(*d).clone() }
            }
        });
    let source = cleaned.as_ref().unwrap_or(&scene.image);
    // Lens corrections (Lensfun): distortion, fringing and vignetting, first, so every
    // tool and the removals work on the corrected photo. A part of the photo (zoomed in)
    // comes already corrected (`zoom::render_part`).
    let lensfun = if settings.region.is_none() { lens_correction(scene, settings) } else { None };
    let fixed = lensfun.as_ref().map(|c| c.apply(source));
    let source = fixed.as_ref().unwrap_or(source);
    // Patches (Heal / Clone) and removed objects, in the order they were made, blended into the scene
    // light before every other tool. A part of the photo (zoomed in) comes with them already done
    // (`zoom::render_part`, its patches' sources being anywhere in the photo; it clears the list).
    let spotted = crate::removal::apply_in_order(source, &settings.removals, settings.region.as_ref().map(|r| &r.frame), None);
    let source = spotted.as_ref().unwrap_or(source);
    // Lens profile vignetting, divided out of the scene light first (not on top of Lensfun's).
    let corrected = if settings.lens_correct > 0.0 && scene.lens.is_some() {
        None
    } else {
        scene.lens.as_ref().and_then(|l| crate::lensprofile::correct_vignetting(source, l, settings.lens_vignette / 100.0))
    };
    let image = corrected.as_ref().unwrap_or(source);
    // A TIFF or JPEG is already toned: the "As is" profile, never a camera's or the user's.
    let as_is;
    let settings = if scene.display_referred {
        as_is = crate::pipeline::RenderSettings { profile: crate::pipeline::Profile::AsIs, user_profile: None, ..settings.clone() };
        &as_is
    } else {
        settings
    };
    // A profile the user added (`userprofiles`, by name), where they have it
    // for this camera; else RapidOne's built-in Shinobi profile. Adobe's
    // profiles are used only if the user added them: RapidOne doesn't look
    // for them by itself. A Lightroom edit's Adobe Color / Adobe Standard
    // asks for the user's profile of that name.
    let wanted = settings.user_profile.clone().or_else(|| match settings.profile {
        crate::pipeline::Profile::AdobeColor => Some("Adobe Color".to_string()),
        crate::pipeline::Profile::AdobeStandard => Some("Adobe Standard".to_string()),
        _ => None,
    });
    let adobe = wanted
        .and_then(|name| crate::userprofiles::for_render(&name, &scene.camera))
        .map(|(profile, look)| (profile, look, crate::userprofiles::default_curve()))
        .zip(scene.wb.as_ref());
    match adobe {
        Some(((profile, look, default_curve), wb)) => {
            let render = crate::dngcolour::ProfileRender::new(
                &profile,
                look.as_ref(),
                wb,
                settings.temperature.unwrap_or(wb.shot_kelvin),
                settings.tint.unwrap_or(wb.shot_tint),
                crate::dngcolour::LookOptions { profile_look_table: true, look_curve: true },
                &default_curve,
            );
            let baseline = crate::adobe::baseline_exposure(&scene.make, &scene.camera) + scene.exposure_offset;
            let settings = crate::pipeline::RenderSettings { shadow_tint_scale: crate::adobe::shadow_tint_scale(&scene.camera), ..settings.clone() };
            if sixteen {
                image::DynamicImage::ImageRgb16(crate::pipeline::render16(image, &settings, baseline, Some(wb), Some(&render)))
            } else {
                image::DynamicImage::ImageRgb8(crate::pipeline::render_adobe(image, &settings, baseline, Some(wb), &render))
            }
        }
        None => {
            let baseline = settings.profile.baseline_for(&scene.make, &scene.camera) + scene.exposure_offset;
            let mut settings = settings.clone();
            settings.shadow_tint_scale = crate::adobe::shadow_tint_scale(&scene.camera);
            // The camera model's colour correction, for both Shinobi profiles
            // (it corrects the camera, not the look).
            settings.camera_fix = crate::pipeline::camera_fix(&scene.camera);
            if sixteen {
                image::DynamicImage::ImageRgb16(crate::pipeline::render16(image, &settings, baseline, scene.wb.as_ref(), None))
            } else {
                crate::pipeline::render_dynamic(image, &settings, baseline, scene.wb.as_ref())
            }
        }
    }
}

/// Whether the user has added Adobe Color (for the Lightroom-edit fallback
/// notes). RapidOne doesn't look for Adobe's own install.
#[tauri::command]
pub fn adobe_color_available() -> bool {
    crate::userprofiles::list().iter().any(|p| p.name == "Adobe Color")
}

/// Develops a RAW with RapidOne's engine (cached) and renders it neutral.
#[tauri::command]
pub async fn develop_preview(cache: tauri::State<'_, SceneCache>, path: String) -> Result<DevelopedPreview, String> {
    let scene = cache.get_or_develop(&path)?;
    render_scene(path, &scene, &crate::pipeline::RenderSettings::default())
}

/// Develops a photo into the cache ahead of time (the neighbours of the one
/// on screen), so stepping to it skips the RAW decode.
#[tauri::command]
pub async fn prefetch_scene(cache: tauri::State<'_, SceneCache>, path: String) -> Result<(), String> {
    cache.get_or_develop(&path).map(|_| ())
}

/// Cleans a cached photo's noise ahead of time (a neighbour with Noise on),
/// so stepping to it shows at once; kept with its scene as when rendered.
#[tauri::command]
pub async fn prepare_noise(cache: tauri::State<'_, SceneCache>, path: String) -> Result<(), String> {
    let scene = cache.get_or_develop(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _ = scene.denoised.get_or_init(|| crate::denoise::scene(&scene.image).map(Arc::new));
    })
    .await
    .map_err(|e| e.to_string())
}

/// Every saved RapidOne preset (JSON each), from the app's presets folder.
#[tauri::command]
pub fn list_presets(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    crate::presets::list(&app)
}

/// Saves a RapidOne preset under its name (the same name replaces it).
#[tauri::command]
pub fn save_preset(app: tauri::AppHandle, name: String, contents: String) -> Result<(), String> {
    crate::presets::save(&app, &name, &contents)
}

/// Copies every preset into a folder, as a backup (restore with Import).
#[tauri::command]
pub fn export_presets(app: tauri::AppHandle, folder: String) -> Result<usize, String> {
    crate::presets::export(&app, Path::new(&folder))
}

/// Reads a file as text (a RapidOne preset backup being imported).
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}

#[tauri::command]
pub fn delete_preset(app: tauri::AppHandle, name: String) -> Result<(), String> {
    crate::presets::delete(&app, &name)
}

/// The photo's RapidOne sidecar (`.rpd`, JSON), or None if it has none yet.
#[tauri::command]
pub fn read_sidecar(path: String) -> Option<String> {
    crate::sidecar::read(Path::new(&path))
}

/// Writes the photo's RapidOne sidecar (`.rpd`, JSON).
#[tauri::command]
pub fn write_sidecar(path: String, contents: String) -> Result<(), String> {
    crate::sidecar::write_edit(Path::new(&path), &contents)
}

/// A shoot organised by Cull (Organise: `rated` and `unrated` inside it):
/// its `rated` folder, which Edit opens instead. None otherwise, or when
/// `folder` is already `rated`/`unrated` itself.
#[tauri::command]
pub fn rated_subfolder(folder: String) -> Option<String> {
    let f = Path::new(&folder);
    let name = f.file_name()?.to_string_lossy().to_lowercase();
    if name == "rated" || name == "unrated" {
        return None;
    }
    let rated = f.join("rated");
    rated.is_dir().then(|| rated.to_string_lossy().to_string())
}

/// The shoot's views once Organise has sorted it (All = its own folder,
/// Rated, Unrated), for Edit's switch. None for a folder never organised.
#[tauri::command]
pub fn folder_views(folder: String) -> Option<FolderViews> {
    let root = library::shoot_root(Path::new(&folder));
    let (rated, unrated) = (root.join("rated").is_dir(), root.join("unrated").is_dir());
    (rated || unrated).then(|| FolderViews { root: root.to_string_lossy().into_owned(), rated, unrated })
}

/// Photos in an organised shoot's `rated` folder that are no longer rated (no
/// stars, not picked: Cull's rule) moved to its `unrated` folder, with their
/// `.xmp`, paired JPEG and `.rpd` (Cull's own move). Photos not in a `rated`
/// folder are left alone. Returns the photos moved (their old paths).
#[tauri::command]
pub async fn move_to_unrated(paths: Vec<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut moved = Vec::new();
        for path in &paths {
            let photo = Path::new(path);
            let Some(dir) = photo.parent() else { continue };
            if !dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("rated")) {
                continue;
            }
            let dest = library::shoot_root(dir).join("unrated");
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            tauri_plugin_rapidculling::library::move_photo(photo, &dest).map_err(|e| format!("{}: {e}", photo.display()))?;
            moved.push(path.clone());
        }
        Ok(moved)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
pub struct FolderViews {
    pub root: String,
    pub rated: bool,
    pub unrated: bool,
}

/// The stars and pick of these photos as their sidecars now say (after
/// rating in another tab), as [path, rating, pick].
#[tauri::command]
pub fn read_flags(paths: Vec<String>) -> Vec<(String, i8, bool)> {
    paths
        .into_iter()
        .map(|p| {
            let path = PathBuf::from(&p);
            let (rating, pick) = crate::sidecar::flags(&path).unwrap_or_else(|| {
                let f = xmp::read(&xmp::sidecar_path_for(&path));
                (f.rating, f.pick)
            });
            (p, rating, pick)
        })
        .collect()
}

/// Finds a photo's subject mask in the background (for the subject-aware
/// vignette), so the slider never waits on it. Kept with the photo's scene.
#[tauri::command]
pub async fn prepare_subject(cache: tauri::State<'_, SceneCache>, path: String) -> Result<bool, String> {
    let scene = cache.get_or_develop(&path)?;
    tauri::async_runtime::spawn_blocking(move || subject_mask_of(&scene, &path).is_some())
        .await
        .map_err(|e| e.to_string())
}

/// A photo brought up to date with these settings (after a paste onto many photos):
/// its fit-to-screen picture, as the viewer shows it (so opening it later shows the
/// edit at once), and its thumbnail for the filmstrip and library, from one render.
/// On a develop of its own, so the photos being edited stay in the scene cache. Its
/// subject mask, if a subject mask needs it, from the folder's cache (`find_subject`
/// puts it there first).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshedPhoto {
    pub preview: DevelopedPreview,
    pub thumb: String,
}

/// Work done ahead for photos not on screen (their full pictures after a paste or a style): on
/// threads of its own at background priority, so a slider moved meanwhile gets the cores first.
fn background_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("background-{i}"))
            .start_handler(|_| tauri_plugin_rapidculling::priority::background())
            .build()
            .expect("background thread pool")
    })
}

#[tauri::command]
pub async fn refresh_photo(path: String, settings: crate::pipeline::RenderSettings) -> Result<RefreshedPhoto, String> {
    tauri::async_runtime::spawn_blocking(move || background_pool().install(|| {
        let photo = Path::new(&path);
        let scene = crate::develop::develop_scene(photo, FULL_PREVIEW_MAX_DIM as usize)?;
        if let Some(m) = crate::diskcache::load_mask(photo) {
            let _ = scene.subject_mask.set(Some(Arc::new(m)));
        }
        let img = render_image(&scene, &settings);
        let thumb = preview::make_thumb_base64(&img, 1, THUMB_EDGE)?.0;
        Ok(RefreshedPhoto { preview: preview_of(path.clone(), &scene, &settings, &img)?, thumb })
    }))
    .await
    .map_err(|e| e.to_string())?
}

/// Keeps a picture in the folder's cache under the app's settings key for it, so the
/// viewer shows it at once when the photo is opened.
#[tauri::command]
pub fn cache_preview(path: String, key: String, preview: DevelopedPreview) -> Result<(), String> {
    crate::diskcache::save_preview(Path::new(&path), &key, &preview)
}

/// The long side of a thumbnail rendered with a photo's edit.
const THUMB_EDGE: u32 = 480;

/// Finds a photo's subject mask ahead of time for its subject masks (after a paste
/// onto many photos), kept in the folder's cache: on a small develop of its own,
/// so the photos being edited stay in the scene cache. Quick if it's found already.
#[tauri::command]
pub async fn find_subject(path: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let photo = Path::new(&path);
        if crate::diskcache::load_mask(photo).is_some() {
            return Ok(true);
        }
        let scene = crate::develop::develop_scene(photo, 1600)?;
        Ok(match crate::mask::subject(&scene.image) {
            Some(m) => {
                let _ = crate::diskcache::save_mask(photo, &m);
                true
            }
            None => false,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The photo's subject mask, kept with its scene: from the folder's cache if
/// it's been found before; else found now and kept there.
fn subject_mask_of(scene: &crate::develop::Scene, path: &str) -> Option<Arc<crate::mask::Mask>> {
    scene
        .subject_mask
        .get_or_init(|| {
            let photo = Path::new(path);
            crate::diskcache::load_mask(photo)
                .or_else(|| {
                    let m = crate::mask::subject(&scene.image)?;
                    let _ = crate::diskcache::save_mask(photo, &m);
                    Some(m)
                })
                .map(Arc::new)
        })
        .clone()
}

/// A mask (any kind, combined or not) to show over the photo while it's being set: red where
/// it acts (as Lightroom's overlay), as a PNG data URL over the whole frame, 1024 pixels on
/// its long side. The photo's subject mask is found for it if it needs one.
#[tauri::command]
pub async fn mask_overlay(cache: tauri::State<'_, SceneCache>, path: String, mask: crate::gradient::Gradient) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let scene = cache.get_or_develop(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let subject = if crate::gradient::needs_subject(&mask) {
            subject_mask_of(&scene, &path);
            scene.subject_refined()
        } else {
            None
        };
        let (iw, ih) = (scene.image.width.max(1) as f32, scene.image.height.max(1) as f32);
        let aspect = iw / ih;
        let shape = crate::gradient::MaskShape::new(&mask, aspect, subject.as_deref());
        let (w, h) = if aspect >= 1.0 { (1024u32, (1024.0 / aspect).round().max(1.0) as u32) } else { ((1024.0 * aspect).round().max(1.0) as u32, 1024u32) };
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            let m = shape.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32).clamp(0.0, 1.0);
            image::Rgba([255, 40, 40, (m * 150.0).round() as u8])
        });
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(Some(format!("data:image/png;base64,{}", STANDARD.encode(&bytes))))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The photo's subject mask (refined) as a PNG, base64: frozen into a combined mask with a
/// subject in it when it's copied, so the photos it's pasted onto get this shape as it is.
#[tauri::command]
pub async fn subject_snapshot(cache: tauri::State<'_, SceneCache>, path: String) -> Result<Option<String>, String> {
    let scene = cache.get_or_develop(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        subject_mask_of(&scene, &path);
        Ok(scene.subject_refined().and_then(|g| g.to_png_base64(1024)))
    })
    .await
    .map_err(|e| e.to_string())?
}


/// Re-renders a developed photo with the given settings - what the sliders
/// call. Uses the cached scene, so it's fast after the first develop.
#[tauri::command]
pub async fn render_preview(
    cache: tauri::State<'_, SceneCache>,
    measured: tauri::State<'_, crate::zoom::MeasuredCache>,
    path: String,
    settings: crate::pipeline::RenderSettings,
    cache_key: Option<String>,
    interactive: Option<bool>,
    shown_long: Option<u32>,
    background: Option<bool>,
) -> Result<DevelopedPreview, String> {
    // While a slider moves: a real render at the size the photo is shown
    // (never smaller, so it looks the same when the full preview follows),
    // not kept or measured.
    if interactive.unwrap_or(false) {
        let long = shown_long.map(|l| (l as usize).div_ceil(200) * 200).unwrap_or(usize::MAX);
        let small = cache.get_small(&path, long)?;
        // The last full render's Definition measures, while they hold what these settings need.
        let reuse = measured.latest_for(&path).filter(|m| m.covers(&settings));
        let settings = crate::pipeline::RenderSettings { reuse_measured: reuse, ..settings };
        return render_scene(path, &small, &settings);
    }
    let full = || {
        let scene = cache.get_or_develop(&path)?;
        // What the whole photo measures, kept for zoomed-in detail with these settings.
        let key = crate::zoom::settings_key(&path, &settings);
        let slot = Arc::new(std::sync::Mutex::new(None));
        let measuring = crate::pipeline::RenderSettings { measure_into: Some(slot.clone()), ..settings.clone() };
        let preview = render_scene(path.clone(), &scene, &measuring)?;
        if let Some(m) = slot.lock().unwrap().take() {
            measured.put(key, m);
        }
        // Kept in the folder's cache, so reopening the shoot shows it at once.
        if let Some(key) = &cache_key {
            let _ = crate::diskcache::save_preview(Path::new(&preview.path), key, &preview);
        }
        Ok(preview)
    };
    // A photo not on screen (the ones either side, made ahead): at background priority.
    if background.unwrap_or(false) {
        background_pool().install(full)
    } else {
        full()
    }
}

/// Zoomed-in detail: the part of the photo on screen (`rect`: left, top,
/// right, bottom as fractions of the photo as shown), rendered from the
/// full-resolution photo with these settings, `width` x `height` pixels at
/// most. The first time for a photo, its full-resolution develop comes first
/// (about 1 s at 24 MP, 5 s at 100 MP).
#[tauri::command]
pub async fn render_detail(
    cache: tauri::State<'_, SceneCache>,
    full: tauri::State<'_, crate::zoom::FullScene>,
    measured: tauri::State<'_, crate::zoom::MeasuredCache>,
    path: String,
    settings: crate::pipeline::RenderSettings,
    rect: [f32; 4],
    width: u32,
    height: u32,
    interactive: Option<bool>,
) -> Result<String, String> {
    let preview = cache.get_or_develop(&path)?;
    // The whole photo's measures with these settings: from its preview render,
    // or measured now. While a slider moves (`interactive`), the photo's newest
    // measures stand in, so nothing waits on a whole-photo render.
    let key = crate::zoom::settings_key(&path, &settings);
    // ...as long as they hold what these settings need (Clarity, Dehaze or a
    // tone slider just moved off zero: measured once, then reused).
    let quick = if interactive.unwrap_or(false) { measured.latest_for(&path).filter(|m| m.covers(&settings)) } else { None };
    let m = match measured.get(&key).or(quick) {
        Some(m) => m,
        None => {
            let slot = Arc::new(std::sync::Mutex::new(None));
            let measuring = crate::pipeline::RenderSettings { measure_into: Some(slot.clone()), ..settings.clone() };
            let _ = render_image(&preview, &measuring);
            let m = slot.lock().unwrap().take().unwrap_or_default();
            measured.put(key, m);
            measured.get(&crate::zoom::settings_key(&path, &settings)).unwrap_or_default()
        }
    };
    let full_scene = full.get_or_develop(&path)?;
    let img = crate::zoom::render_part(&full_scene, &preview, m, &settings, rect, width, height)?;
    let mut jpeg = Vec::new();
    // While a slider moves, a lighter JPEG (it is replaced as soon as it stops).
    let quality = if interactive.unwrap_or(false) { 80 } else { 92 };
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality).encode_image(&img).map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg)))
}

/// The photo as graded in Edit, full size and 16-bit, for RapidRetouch
/// (`handoff`): the TIFF's path. Rendered again only when `key` (the
/// grade) has changed since the last time.
#[tauri::command]
pub async fn render_handoff(path: String, settings: crate::pipeline::RenderSettings, key: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::handoff::render(Path::new(&path), &settings, &key))
        .await
        .map_err(|e| e.to_string())?
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// The installed Auto Style styles: name, file, and what they learned from.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleInfo {
    name: String,
    path: String,
    /// Edited photos and shoots (folders) it learned from; 0 for a style made
    /// before these were kept (it can't be added to).
    photos: usize,
    shoots: usize,
}

/// The app's own styles folder (Train Style saves there).
/// The profiles the user has added, by name.
#[tauri::command]
pub fn user_profiles() -> Vec<crate::userprofiles::UserProfile> {
    crate::userprofiles::list()
}

/// Adds the chosen .dcp / .xmp files (copied into RapidOne's profiles
/// folder): the names added and a message for each file that isn't a profile.
#[tauri::command]
pub fn add_user_profiles(paths: Vec<String>) -> (Vec<String>, Vec<String>) {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    crate::userprofiles::add(&files)
}

/// Takes an added profile (every camera's file of that name) out again.
#[tauri::command]
pub fn remove_user_profile(name: String) -> Result<usize, String> {
    crate::userprofiles::remove(&name)
}

fn styles_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("styles");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

fn style_info(path: &Path, style: &crate::autostyle::Style) -> StyleInfo {
    StyleInfo { name: style.name.clone(), path: path.to_string_lossy().replace('\\', "/"), photos: style.samples.len(), shoots: style.shoots() }
}

/// A style, loaded once and kept while its file is unchanged (Apply Style to
/// all photos asks for it once per photo).
fn cached_style(path: &Path) -> Result<Arc<crate::autostyle::Style>, String> {
    type Entry = (PathBuf, Option<std::time::SystemTime>, Arc<crate::autostyle::Style>);
    static CACHE: Mutex<Option<Entry>> = Mutex::new(None);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, m, s)) = cache.as_ref() {
        if p == path && *m == modified {
            return Ok(s.clone());
        }
    }
    let style = Arc::new(crate::autostyle::load(path)?);
    *cache = Some((path.to_path_buf(), modified, style.clone()));
    Ok(style)
}

#[tauri::command]
pub fn auto_style_list(app: tauri::AppHandle) -> Vec<StyleInfo> {
    let extra: Vec<PathBuf> = styles_dir(&app).into_iter().collect();
    crate::autostyle::styles(&extra)
        .into_iter()
        .filter_map(|p| cached_style(&p).ok().map(|s| style_info(&p, &s)))
        .collect()
}

/// A style's look (every setting its edits share): from an .xmp, in RapidOne's preset terms, or a
/// RapidOne edit (`.rpd` form) the app turns into the same; both None for a style without one.
#[derive(serde::Serialize)]
pub struct StyleLook {
    lightroom: Option<crate::preset::LightroomSettings>,
    rapidone: Option<serde_json::Value>,
}

#[tauri::command]
pub fn auto_style_look(style: String) -> Result<StyleLook, String> {
    let s = cached_style(Path::new(&style))?;
    // The profile falls back to its Shinobi equivalent when the Adobe
    // profiles aren't installed, as for any preset (`applyLightroom`).
    Ok(StyleLook { lightroom: (!s.look_xmp.is_empty()).then(|| crate::preset::parse(&s.look_xmp)), rapidone: s.look_rpd.clone() })
}

/// Auto Style for one photo: its white balance and exposure as the style
/// would set them.
/// A photo's ISO and aperture (f-number), for the Curves panel's corner. From the RAW's metadata
/// (as Auto Style reads it), else the file's EXIF (a JPEG or TIFF); either can be missing.
#[derive(serde::Serialize)]
pub struct ShotDetails {
    pub iso: Option<u32>,
    pub aperture: Option<f32>,
}

#[tauri::command]
pub async fn shot_details(path: String) -> ShotDetails {
    tauri::async_runtime::spawn_blocking(move || {
        let photo = Path::new(&path);
        let rat = |r: rawler::formats::tiff::Rational| (r.d > 0).then(|| r.n as f32 / r.d as f32);
        let from_raw = rawler::rawsource::RawSource::new(photo).ok().and_then(|src| {
            let dec = rawler::get_decoder(&src).ok()?;
            let e = dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default()).ok()?.exif;
            Some(ShotDetails {
                iso: e.iso_speed_ratings.map(u32::from).or(e.iso_speed).or(e.recommended_exposure_index),
                aperture: e.fnumber.and_then(rat),
            })
        });
        from_raw.filter(|d| d.iso.is_some() || d.aperture.is_some()).unwrap_or_else(|| {
            let exif = std::fs::File::open(photo).ok().and_then(|f| exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)).ok());
            let field = |tag| exif.as_ref().and_then(|e| e.get_field(tag, exif::In::PRIMARY)).map(|f| f.value.clone());
            ShotDetails {
                iso: field(exif::Tag::PhotographicSensitivity).and_then(|v| v.get_uint(0)),
                aperture: match field(exif::Tag::FNumber) {
                    Some(exif::Value::Rational(r)) => r.first().map(|r| r.to_f32()),
                    _ => None,
                },
            }
        })
    })
    .await
    .unwrap_or(ShotDetails { iso: None, aperture: None })
}

/// The computer's core count, for sizing work done several photos at once (the webview's own
/// figure can be capped, as WebKit does against fingerprinting).
#[tauri::command]
pub fn core_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

#[tauri::command]
pub async fn auto_style_photo(style: String, path: String) -> Result<crate::autostyle::Prediction, String> {
    // At background priority: the photo on screen renders first while a style goes through a folder.
    tauri::async_runtime::spawn_blocking(move || background_pool().install(|| cached_style(Path::new(&style))?.predict(Path::new(&path))))
        .await
        .map_err(|e| e.to_string())?
}

/// Train Style's progress: `stage` "reading" (photos measured) or
/// "training" (passes), `done` of `total`.
#[derive(serde::Serialize, Clone)]
struct StyleProgress<'a> {
    stage: &'a str,
    done: usize,
    total: usize,
}

/// Train Style: a new style called `name` from folders of edited RAWs, or,
/// with `style` (Increase training), those folders added to that style and
/// it retrained on everything. Emits `style-progress`.
#[tauri::command]
pub async fn auto_style_train(app: tauri::AppHandle, name: String, folders: Vec<String>, style: Option<String>) -> Result<StyleInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (path, base) = match &style {
            Some(p) => {
                let base = crate::autostyle::load(Path::new(p))?;
                if base.samples.is_empty() {
                    return Err("This style was made before styles kept their training photos, so it can't be added to. Train a new style from all its folders instead.".to_string());
                }
                (PathBuf::from(p), Some(base))
            }
            None => {
                if name.trim().is_empty() {
                    return Err("Give the style a name first.".to_string());
                }
                let file = crate::autostyle::file_name(&name);
                let extra: Vec<PathBuf> = styles_dir(&app).into_iter().collect();
                if crate::autostyle::styles(&extra).iter().any(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == file)) {
                    return Err(format!("There's already a style called \"{}\". Pick another name, or use Increase training on it.", name.trim()));
                }
                (styles_dir(&app)?.join(file), None)
            }
        };
        let name = base.as_ref().map_or(name.clone(), |b| b.name.clone());
        // Progress at most every 1% so the frontend isn't flooded.
        let last = Mutex::new((String::new(), usize::MAX));
        let progress = |stage: &str, done: usize, total: usize| {
            let step = (done * 100) / total.max(1);
            let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
            if l.0 != stage || l.1 != step {
                *l = (stage.to_string(), step);
                let _ = app.emit("style-progress", StyleProgress { stage, done, total });
            }
        };
        let trained = crate::autostyle::train(&name, &folders, base, &progress)?;
        crate::autostyle::save(&trained, &path)?;
        Ok(style_info(&path, &trained))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A photo's last engine picture from the folder's cache, with the settings
/// key it was rendered with; None if it has none.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPreview {
    key: String,
    preview: DevelopedPreview,
}

/// Photos whose edits were reset (many at once): each one's thumbnail back to the camera's picture
/// (from the shared scan cache, no render), and its out-of-date edited picture dropped from the
/// folder's cache so a reopen doesn't bring the old look back. [path, thumbnail] for each it could do.
#[tauri::command]
pub async fn camera_thumbs(paths: Vec<String>) -> Vec<(String, String)> {
    tauri::async_runtime::spawn_blocking(move || {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).max(1);
        let out = Mutex::new(Vec::with_capacity(paths.len()));
        std::thread::scope(|s| {
            for chunk in paths.chunks(paths.len().div_ceil(threads).max(1)) {
                let out = &out;
                s.spawn(move || {
                    tauri_plugin_rapidculling::priority::background();
                    for p in chunk {
                        let photo = Path::new(p);
                        crate::diskcache::forget_preview(photo);
                        let Some(kept) = tauri_plugin_rapidculling::cache::load(photo) else { continue };
                        let Ok(img) = image::load_from_memory(&kept.jpeg) else { continue };
                        if let Ok((thumb, _, _)) = preview::make_thumb_base64(&img, 1, FILMSTRIP_THUMB_MAX_DIM) {
                            out.lock().unwrap().push((p.clone(), thumb));
                        }
                    }
                });
            }
        });
        out.into_inner().unwrap()
    })
    .await
    .unwrap_or_default()
}

/// The settings key of the photo's kept picture and the as-shot white balance it read (no picture):
/// whether it's still current, for many photos at once.
#[tauri::command]
pub fn cached_preview_key(path: String) -> Option<(String, Option<f32>, Option<f32>)> {
    crate::diskcache::preview_key(Path::new(&path))
}

#[tauri::command]
pub fn cached_preview(path: String) -> Option<CachedPreview> {
    crate::diskcache::load_preview(Path::new(&path)).map(|(key, preview)| CachedPreview { key, preview: DevelopedPreview { path, ..preview } })
}


/// A Lightroom preset (`.xmp`), in RapidOne's terms (`preset::LightroomSettings`).
#[tauri::command]
pub fn read_lightroom_preset(path: String) -> Result<crate::preset::LightroomSettings, String> {
    let xmp = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let settings = crate::preset::parse(&xmp);
    if settings.is_empty() {
        return Err("No Lightroom develop settings in this file.".into());
    }
    Ok(settings)
}

/// A photo's own Lightroom edit, from its XMP sidecar; None if it has none, or RapidOne's edit is
/// the one to use (`sidecar::lightroom_applies`).
#[tauri::command]
pub fn read_lightroom_edit(path: String) -> Option<crate::preset::LightroomSettings> {
    let photo = std::path::Path::new(&path);
    if !crate::sidecar::lightroom_applies(photo) {
        return None;
    }
    crate::preset::for_photo(photo)
}

/// The parametric (region) curve as the engine applies it, sampled for the
/// Curves panel to draw: amounts [Shadows, Darks, Lights, Highlights] and
/// splits (0..100).
#[tauri::command]
pub fn parametric_curve_points(amounts: [f32; 4], splits: [f32; 3]) -> Vec<f32> {
    crate::tools::parametric_points(amounts, splits, 97)
}

/// Whether object removal is set up on this computer.
#[tauri::command]
pub fn removal_available() -> bool {
    let (large, medium, lite) = crate::removal::installed();
    large || medium || lite
}

/// The removers installed and which is in use, for the Remove bar's switch.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalModes {
    pub large: bool,
    pub medium: bool,
    pub lite: bool,
    pub mode: crate::removal::Mode,
}

#[tauri::command]
pub fn removal_modes() -> RemovalModes {
    let (large, medium, lite) = crate::removal::installed();
    RemovalModes { large, medium, lite, mode: crate::removal::mode() }
}

/// The Remove bar's Large / Medium / Lite switch.
#[tauri::command]
pub fn set_removal_mode(mode: crate::removal::Mode) {
    crate::removal::set_mode(mode);
}

/// Removes what's in `area` (fractions of the upright, uncropped photo:
/// left, top, right, bottom) from the photo; `what` optionally names it, and
/// `outline` (from `find_objects`) gives its exact shape. Returns the
/// removal to add to the photo's edit.
#[tauri::command]
pub async fn remove_object(path: String, area: [f32; 4], what: String, outline: Option<String>, seed: Option<u32>) -> Result<crate::removal::Removal, String> {
    let seed = seed.unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || crate::removal::create(Path::new(&path), area, &what, outline.as_deref().map(Path::new), seed))
        .await
        .map_err(|e| e.to_string())?
}

/// Brush: removes the painted shape (`painted`: a white-on-black PNG data
/// URL or base64, covering the whole uncropped photo).
#[tauri::command]
pub async fn remove_painted(path: String, painted: String, what: String, seed: Option<u32>) -> Result<crate::removal::Removal, String> {
    let seed = seed.unwrap_or(0);
    use base64::Engine;
    let b64 = painted.split_once(',').map_or(painted.as_str(), |(_, b)| b).to_string();
    let png = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || crate::removal::remove_painted(Path::new(&path), &png, &what, seed))
        .await
        .map_err(|e| e.to_string())?
}

/// Loads the noise model (and picks the fastest graphics card) in the
/// background, so the first photo with Noise on doesn't wait for it. Called
/// when Edit opens; returns at once.
#[tauri::command]
pub fn warm_denoise() {
    std::thread::spawn(|| {
        let _ = crate::denoise::available();
    });
}

/// Loads Lite's models in the background (Edit opened), so the first Remove
/// doesn't wait for them. Only Lite: Klein loads with the Remove tool and
/// leaves the GPU when it closes.
#[tauri::command]
pub fn warm_lite() {
    std::thread::spawn(|| {
        if crate::removal::lite() {
            crate::lite::warm();
        }
    });
}

/// Whether objects can be found by words (SAM 3 is set up).
#[tauri::command]
pub fn can_find_objects() -> bool {
    crate::removal::can_find()
}

/// The Remove tool opened or closed: its models load with it and leave the
/// GPU a little after it closes (`klein::hold`).
#[tauri::command]
pub fn removal_hold(open: bool) {
    crate::klein::hold(open);
}

/// Remove opened on this photo: SAM 3 analyses it in the background.
#[tauri::command]
pub async fn prepare_find(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::removal::prepare(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}

/// SAM 3: every object in the photo matching `words`, or (with `area`, no
/// words) the object in that box, with outlines to show.
#[tauri::command]
pub async fn find_objects(path: String, words: String, area: Option<[f32; 4]>) -> Result<Vec<crate::removal::Outline>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::removal::find(Path::new(&path), &words, area))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod trash_tests {
    use super::*;

    #[test]
    fn a_trashed_photo_moves_with_its_xmp_and_rpd_and_a_second_one_of_the_same_name_is_numbered() {
        let dir = std::env::temp_dir().join(format!("rapidone-trash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sidecars")).unwrap();
        let photo = dir.join("IMG_1.NEF");
        for _ in 0..2 {
            std::fs::write(&photo, b"raw").unwrap();
            std::fs::write(dir.join("IMG_1.xmp"), b"xmp").unwrap();
            std::fs::write(dir.join("sidecars").join("IMG_1.NEF.rpd"), b"rpd").unwrap();
            trash_one(&photo).unwrap();
            assert!(!photo.exists() && !dir.join("IMG_1.xmp").exists() && !dir.join("sidecars").join("IMG_1.NEF.rpd").exists());
        }
        let t = trash_folder(&dir);
        for f in ["IMG_1.NEF", "IMG_1.xmp", "IMG_1.NEF.rpd", "IMG_1 (2).NEF", "IMG_1 (2).xmp", "IMG_1 (2).NEF.rpd"] {
            assert!(t.join(f).is_file(), "{f} in the trash");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn undo_puts_a_photo_back_with_its_xmp_and_rpd() {
        let dir = std::env::temp_dir().join(format!("rapidone-restore-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sidecars")).unwrap();
        let photo = dir.join("IMG_2.RW2");
        // One of the same name in the trash already, so this one goes in as "IMG_2 (2)".
        std::fs::create_dir_all(trash_folder(&dir)).unwrap();
        std::fs::write(trash_folder(&dir).join("IMG_2.RW2"), b"older").unwrap();
        std::fs::write(&photo, b"raw").unwrap();
        std::fs::write(dir.join("IMG_2.xmp"), b"xmp").unwrap();
        std::fs::write(dir.join("sidecars").join("IMG_2.RW2.rpd"), b"rpd").unwrap();
        let trashed = trash_one(&photo).unwrap();
        assert!(trashed.ends_with("IMG_2 (2).RW2") && !photo.exists());
        let item = Trashed { path: photo.to_string_lossy().to_string(), trashed: trashed.to_string_lossy().to_string() };
        let back = tauri::async_runtime::block_on(restore_photos(vec![item])).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(std::fs::read(&photo).unwrap(), b"raw");
        assert_eq!(std::fs::read(dir.join("IMG_2.xmp")).unwrap(), b"xmp");
        assert_eq!(std::fs::read(dir.join("sidecars").join("IMG_2.RW2.rpd")).unwrap(), b"rpd");
        // The other one of that name stays in the trash.
        assert_eq!(std::fs::read(trash_folder(&dir).join("IMG_2.RW2")).unwrap(), b"older");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod scan_memory {
    /// Edit's folder read on RAPIDONE_SCAN_FOLDER with RAPIDONE_SCAN_THREADS threads (no cache: the
    /// folder's cull-cache is removed first), then a pause so its memory can be watched from outside:
    /// cargo test --release --lib scan_memory -- --ignored --nocapture
    #[test]
    #[ignore]
    fn read_folder() {
        let Ok(folder) = std::env::var("RAPIDONE_SCAN_FOLDER") else { return };
        let threads: usize = std::env::var("RAPIDONE_SCAN_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&folder).join("sidecars"));
        let found = crate::library::find_photos(std::path::Path::new(&folder)).unwrap();
        let t = std::time::Instant::now();
        std::thread::scope(|s| {
            for chunk in found.chunks(found.len().div_ceil(threads).max(1)) {
                s.spawn(move || {
                    for p in chunk {
                        let _ = super::process_photo(&p.raw_path, p.jpeg_path.as_deref());
                    }
                });
            }
        });
        tauri_plugin_rapidculling::cache::flush();
        println!("SCAN {} photos, {threads} threads: {:.2} s", found.len(), t.elapsed().as_secs_f32());
        std::thread::sleep(std::time::Duration::from_secs(6));
        println!("DONE");
    }
}

/// Scene light at (x, y) in pixels (bilinear, clamped to the picture).
pub(crate) fn bilinear(px: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    std::array::from_fn(|c| {
        let top = px[y0 * w + x0][c] + (px[y0 * w + x1][c] - px[y0 * w + x0][c]) * fx;
        let bottom = px[y1 * w + x0][c] + (px[y1 * w + x1][c] - px[y1 * w + x0][c]) * fx;
        top + (bottom - top) * fy
    })
}
