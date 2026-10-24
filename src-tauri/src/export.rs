//! Export: a photo as graded in Edit, rendered by RapidOne's own engine at
//! full resolution (so the colours are the viewer's), as a maximum-quality
//! JPEG or a 16-bit TIFF, optionally scaled to a long edge.
//!
//! The options match RapidRetouch's export dialog. File names: the original's
//! name by default (a clash is warned about, not avoided), or a name plus a
//! number in capture-time order. An output that would be the source file
//! itself (a JPEG or TIFF original exported next to itself) gets `_edited`.
//!
//! sRGB, as the viewer shows it, untagged (like the Retouch hand-off).

use std::collections::HashSet;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Larger than any sensor: develop at full resolution.
const FULL_RESOLUTION: usize = 1 << 20;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Tiff,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum NameMode {
    /// The photo's own name.
    Original,
    /// `base` and a number, counted in capture-time order.
    Sequence,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Naming {
    pub mode: NameMode,
    /// Sequence only: the name the numbers follow.
    #[serde(default)]
    pub base: String,
    /// Sequence only: the first number.
    #[serde(default = "one")]
    pub start: u32,
    /// Sequence only: the number is padded with zeros to this many digits;
    /// 0 (the default) fits the highest number, with at least three.
    #[serde(default)]
    pub digits: u32,
}

fn one() -> u32 {
    1
}

/// Digits for the numbers `start` to `start + count - 1` when the width is left
/// automatic: enough for the highest, and never fewer than three (001).
fn auto_digits(start: u32, count: usize) -> usize {
    let highest = start as u64 + count.saturating_sub(1) as u64;
    highest.to_string().len().max(3)
}

/// A photo to export: where it is, and when it was taken.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub path: String,
    pub timestamp_ms: i64,
}

/// One planned output, in export order (capture time).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    pub path: String,
    pub out: String,
    /// The output file is already there (an earlier export, or something else).
    pub exists: bool,
}

/// A file name without anything Windows or the shell would object to.
fn clean(name: &str) -> String {
    let swapped: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let trimmed = swapped.trim_matches(|c: char| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "photo".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The same file, however its path is spelled (slashes, and capitals on
/// Windows).
fn same_file(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

/// Where each photo goes, in capture-time order (ties by file name).
/// `folder`: all outputs go there; None puts each next to its original.
pub fn plan(sources: &[Source], naming: &Naming, folder: Option<&str>, format: Format) -> Vec<Planned> {
    let mut ordered: Vec<&Source> = sources.iter().collect();
    ordered.sort_by(|a, b| {
        let name = |s: &Source| Path::new(&s.path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        a.timestamp_ms.cmp(&b.timestamp_ms).then_with(|| name(a).cmp(&name(b)))
    });
    let ext = format.extension();
    let base = clean(&naming.base);
    // A separator between the name and its number, unless the name ends in one.
    let sep = if base.ends_with(['_', '-', ' ', '.']) || naming.base.trim().is_empty() { "" } else { "_" };
    let mut taken: HashSet<String> = HashSet::new();
    let mut planned = Vec::with_capacity(ordered.len());
    for (i, source) in ordered.iter().enumerate() {
        let src = Path::new(&source.path);
        let dir: PathBuf = match folder {
            Some(f) => PathBuf::from(f),
            None => src.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        let stem = match naming.mode {
            NameMode::Original => src.file_stem().map(|s| clean(&s.to_string_lossy())).unwrap_or_else(|| "photo".to_string()),
            NameMode::Sequence => {
                let digits = if naming.digits == 0 { auto_digits(naming.start, ordered.len()) } else { naming.digits.clamp(1, 9) as usize };
                let number = naming.start as u64 + i as u64;
                if naming.base.trim().is_empty() {
                    format!("{number:0digits$}")
                } else {
                    format!("{base}{sep}{number:0digits$}")
                }
            }
        };
        let make = |stem: &str| dir.join(format!("{stem}.{ext}")).to_string_lossy().replace('\\', "/");
        let mut out = make(&stem);
        // Never write over the photo itself.
        if same_file(&out, &source.path) {
            out = make(&format!("{stem}_edited"));
        }
        // Two photos that would share a name: the later one is told apart.
        let mut n = 1;
        while !taken.insert(out.to_lowercase()) {
            n += 1;
            out = make(&format!("{stem}_edited{}", if n > 2 { format!("_{n}") } else { String::new() }));
        }
        let exists = Path::new(&out).exists();
        planned.push(Planned { path: source.path.clone(), out, exists });
    }
    planned
}

/// Develops `photo` at full resolution and writes it to `out` with these
/// settings: the grade, crop, removals and gradients included, exactly as the
/// viewer renders them. Written through a temporary file, so a stopped export
/// never leaves half a picture, and an earlier export of the same name is
/// replaced only when the new one is complete.
pub fn write(
    photo: &Path,
    settings: &crate::pipeline::RenderSettings,
    out: &Path,
    format: Format,
    long_edge: Option<u32>,
    progress: &dyn Fn(f32, &str),
) -> Result<(), String> {
    let scene = crate::develop::develop_scene_export(photo, FULL_RESOLUTION)?;
    progress(0.45, "Rendering…");
    // The subject mask the viewer found (the folder's cache), as in the hand-off.
    if let Some(mask) = crate::diskcache::load_mask(photo) {
        let _ = scene.subject_mask.set(Some(std::sync::Arc::new(mask)));
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = out.with_extension(format!("{}.tmp", format.extension()));
    let result = match format {
        Format::Tiff => {
            let mut img = crate::commands::render_image16(&scene, settings);
            if let Some((w, h)) = scaled(img.width(), img.height(), long_edge) {
                img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
            }
            progress(0.85, "Saving the TIFF…");
            write_tiff(&tmp, &img)
        }
        Format::Jpeg => {
            let mut img = crate::commands::render_image(&scene, settings).to_rgb8();
            if let Some((w, h)) = scaled(img.width(), img.height(), long_edge) {
                img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
            }
            progress(0.85, "Saving the JPEG…");
            let _p = crate::prof::span("JPEG");
            write_jpeg(&tmp, &img)
        }
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// The size for a long edge of `long`; None when no scaling is wanted (no
/// long edge, or the picture is not larger than it: never scaled up).
fn scaled(w: u32, h: u32, long: Option<u32>) -> Option<(u32, u32)> {
    let long = long?;
    let current = w.max(h);
    if long == 0 || long >= current {
        return None;
    }
    let f = long as f64 / current as f64;
    Some((((w as f64 * f).round() as u32).max(1), ((h as f64 * f).round() as u32).max(1)))
}

fn write_tiff(path: &Path, img: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>) -> Result<(), String> {
    use tiff::encoder::{colortype::RGB16, Compression, DeflateLevel, Predictor, TiffEncoder};
    let out = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
    let mut encoder = TiffEncoder::new(out)
        .map_err(|e| e.to_string())?
        .with_compression(Compression::Deflate(DeflateLevel::Fast))
        .with_predictor(Predictor::Horizontal);
    encoder.write_image::<RGB16>(img.width(), img.height(), img.as_raw()).map_err(|e| e.to_string())
}

fn write_jpeg(path: &Path, img: &image::RgbImage) -> Result<(), String> {
    let out = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(out, 100);
    encoder.encode_image(img).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, ts: i64) -> Source {
        Source { path: path.to_string(), timestamp_ms: ts }
    }

    fn original() -> Naming {
        Naming { mode: NameMode::Original, base: String::new(), start: 1, digits: 3 }
    }

    fn sequence(base: &str, start: u32, digits: u32) -> Naming {
        Naming { mode: NameMode::Sequence, base: base.to_string(), start, digits }
    }

    fn outs(p: &[Planned]) -> Vec<String> {
        p.iter().map(|x| x.out.clone()).collect()
    }

    #[test]
    fn photos_at_once_follow_cores_and_free_memory() {
        const GB: u64 = 1 << 30;
        assert_eq!(workers_for(32, 50 * GB), 3);
        assert_eq!(workers_for(8, 16 * GB), 2);
        assert_eq!(workers_for(2, 16 * GB), 1);
        // Little memory free: fewer, never none.
        assert_eq!(workers_for(32, 4 * GB), 2);
        assert_eq!(workers_for(32, GB), 1);
    }

    #[test]
    fn keeps_original_names_next_to_the_originals() {
        let p = plan(&[src("D:/shoot/P1.RAF", 1)], &original(), None, Format::Jpeg);
        assert_eq!(outs(&p), vec!["D:/shoot/P1.jpg"]);
    }

    #[test]
    fn a_jpeg_exported_next_to_itself_gets_edited() {
        let p = plan(&[src("D:/shoot/P1.jpg", 1)], &original(), None, Format::Jpeg);
        assert_eq!(outs(&p), vec!["D:/shoot/P1_edited.jpg"]);
        // Whatever the capitals or slashes (backslashes separate folders on Windows only; elsewhere
        // they're part of a file's name).
        if cfg!(windows) {
            let p = plan(&[src("D:\\shoot\\p1.JPG", 1)], &original(), Some("D:/shoot"), Format::Jpeg);
            assert_eq!(outs(&p), vec!["D:/shoot/p1_edited.jpg"]);
        }
    }

    #[test]
    fn a_raw_and_its_jpeg_do_not_share_an_output() {
        let p = plan(&[src("D:/s/P1.RAF", 1), src("D:/s/P1.JPG", 1)], &original(), Some("E:/out"), Format::Jpeg);
        assert_eq!(p.len(), 2);
        assert_ne!(p[0].out.to_lowercase(), p[1].out.to_lowercase());
    }

    #[test]
    fn numbers_follow_capture_time_not_the_order_given() {
        let sources = [src("D:/s/c.RAF", 300), src("D:/s/a.RAF", 100), src("D:/s/b.RAF", 200)];
        let p = plan(&sources, &sequence("Wedding", 1, 3), Some("E:/out"), Format::Tiff);
        assert_eq!(outs(&p), vec!["E:/out/Wedding_001.tif", "E:/out/Wedding_002.tif", "E:/out/Wedding_003.tif"]);
        assert_eq!(p[0].path, "D:/s/a.RAF");
        assert_eq!(p[2].path, "D:/s/c.RAF");
    }

    #[test]
    fn a_burst_with_one_timestamp_is_ordered_by_file_name() {
        let sources = [src("D:/s/P2.RAF", 100), src("D:/s/P1.RAF", 100)];
        let p = plan(&sources, &sequence("Shot", 10, 2), Some("E:/o"), Format::Jpeg);
        assert_eq!(p[0].path, "D:/s/P1.RAF");
        assert_eq!(outs(&p), vec!["E:/o/Shot_10.jpg", "E:/o/Shot_11.jpg"]);
    }

    #[test]
    fn a_trailing_separator_is_not_doubled_and_an_empty_name_is_just_numbers() {
        let p = plan(&[src("D:/s/a.RAF", 1)], &sequence("Day-", 5, 4), Some("E:/o"), Format::Jpeg);
        assert_eq!(outs(&p), vec!["E:/o/Day-0005.jpg"]);
        let p = plan(&[src("D:/s/a.RAF", 1)], &sequence("", 1, 3), Some("E:/o"), Format::Jpeg);
        assert_eq!(outs(&p), vec!["E:/o/001.jpg"]);
    }

    #[test]
    fn unsafe_characters_in_the_name_are_replaced() {
        let p = plan(&[src("D:/s/a.RAF", 1)], &sequence("a/b:c?", 1, 2), Some("E:/o"), Format::Jpeg);
        assert_eq!(outs(&p), vec!["E:/o/a_b_c_01.jpg"]);
    }

    #[test]
    fn automatic_digits_fit_the_highest_number_with_at_least_three() {
        assert_eq!(auto_digits(1, 12), 3);
        assert_eq!(auto_digits(1, 999), 3);
        assert_eq!(auto_digits(1, 1200), 4);
        assert_eq!(auto_digits(950, 100), 4);
        let sources: Vec<Source> = (0..12).map(|i| src(&format!("D:/s/p{i:02}.RAF"), i)).collect();
        let p = plan(&sources, &sequence("Day", 1, 0), Some("E:/o"), Format::Jpeg);
        assert_eq!(p[0].out, "E:/o/Day_001.jpg");
        assert_eq!(p[11].out, "E:/o/Day_012.jpg");
    }

    #[test]
    fn long_edge_never_scales_up() {
        assert_eq!(scaled(6000, 4000, Some(3000)), Some((3000, 2000)));
        assert_eq!(scaled(4000, 6000, Some(3000)), Some((2000, 3000)));
        assert_eq!(scaled(6000, 4000, Some(8000)), None);
        assert_eq!(scaled(6000, 4000, None), None);
    }
}

/// What the export dialog needs before it starts: each output, in export
/// order, and which already exist (to warn about).
#[tauri::command]
pub fn export_plan(sources: Vec<Source>, naming: Naming, folder: Option<String>, format: Format) -> Vec<Planned> {
    plan(&sources, &naming, folder.as_deref(), format)
}

/// How many photos Export does at once on this computer. A photo's RAW decode and JPEG writing use
/// one core each while its render uses them all, so a second photo at once fills the gaps (12 photos:
/// 34 s one at a time, 24 s two, 23 s three, no better beyond; 32 cores, 24 Oct). So: two from 4
/// cores, three from 16; and never more than free memory holds (about 1.4 GB a photo, with room).
pub fn workers_for(cores: usize, free_bytes: u64) -> usize {
    let by_cores = if cores >= 16 { 3 } else if cores >= 4 { 2 } else { 1 };
    let by_memory = (free_bytes / (2 << 30)) as usize;
    by_cores.min(by_memory).max(1)
}

#[tauri::command]
pub fn export_workers() -> usize {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    workers_for(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4), sys.available_memory())
}

/// Its stage while a photo exports, for the Export dialog's bar ("export-progress").
#[derive(Clone, serde::Serialize)]
struct ExportProgress<'a> {
    path: &'a str,
    fraction: f32,
    message: &'a str,
}

/// Renders and writes one photo (the frontend calls it photo by photo, so it
/// can show progress and stop between photos), telling the dialog each stage as it's reached.
#[tauri::command]
pub async fn export_photo(
    app: tauri::AppHandle,
    path: String,
    settings: crate::pipeline::RenderSettings,
    out: String,
    format: Format,
    long_edge: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter;
        let progress = |fraction: f32, message: &str| {
            let _ = app.emit("export-progress", ExportProgress { path: &path, fraction, message });
        };
        let result = write(Path::new(&path), &settings, Path::new(&out), format, long_edge, &progress).map(|_| out);
        // A full-size photo's buffers, freed: back to the system (memtrim.rs).
        crate::memtrim::trim();
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
