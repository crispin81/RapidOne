//! The optional **High quality** depth model: Depth Anything 3 Mono-Large (ByteDance-Seed,
//! Apache-2.0; the ONNX export is `Jens-Duttke/Depth-Anything-3-MONO-ONNX`, Apache-2.0 with its
//! LICENSE file). It is far bigger than the models the installer carries (656 MB fp16, 336 MB
//! 4-bit), so it is **never bundled**: the Depth quality setting offers it as an on-demand
//! download into the app's own `models/` folder, with resume and a SHA-256 check.
//!
//! Its own export sees one square picture at a time (252 / 364 / 518 / 728 / 1036; any other size
//! gives a 1 x 1 NaN), float16, ImageNet-normalised, and gives **inverse disparity** (larger =
//! nearer, `1/(depth + 0.2)`, with its own sky head folded in so sky is 0). So a photo that isn't
//! square is letterboxed with its own edge pixels (never black), the output is cropped back, and
//! the values are fitted to the scene's metric distances in `fusion.rs` — the +0.2 shift is
//! absorbed by the affine fit there.
//!
//! Measured here (RTX 5070 Ti, WebGPU/Vulkan; fp16 file): 97 ms at 518, 208 ms at 728, 524 ms at
//! 1036 — so 1036 is the default, well inside the 1.5 s target. The 4-bit file is smaller but
//! slower on this card (288 / 592 / 1314 ms) and no faster on the CPU, so the fp16 file is the one
//! the app downloads.

use crate::gradient::Grid;
use crate::pipeline::SceneImage;
use half::f16;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file the app downloads (fp16: the fastest on the graphics card, and the accurate weights).
pub const FILE: &str = "da-v3-mono-large_fp16_opset21_optimized.onnx";
/// The 4-bit export: half the download, but slower on the card and no faster on the CPU here.
pub const Q4_FILE: &str = "da-v3-mono-large_q4f16_opset21_optimized.onnx";
pub const URL: &str = "https://huggingface.co/Jens-Duttke/Depth-Anything-3-MONO-ONNX/resolve/main/da-v3-mono-large_fp16_opset21_optimized.onnx";
pub const BYTES: u64 = 687_732_291;
/// The file's SHA-256 (checked here after downloading it); every download is checked against it,
/// and a corrupt or truncated one is deleted rather than run.
pub const SHA256: &str = "33f7e4e609bb13ba7874edcc274a92a6ca5cbea8d1c54abeb1dbb2e7441fb764";
pub const LICENCE: &str = "Depth Anything 3 Mono-Large (ByteDance-Seed), Apache-2.0; ONNX export by Jens-Duttke (Apache-2.0)";

/// The square sizes the export accepts (anything else gives a 1 x 1 NaN).
pub const SIZES: [usize; 5] = [252, 364, 518, 728, 1036];
/// The one the app runs at: the sharpest within the 1.5 s target on a graphics card (524 ms here).
pub const SIZE: usize = 1036;

/// What the model is called in the UI and the notes.
pub const NAME: &str = "Depth Anything 3 Mono-Large";

/// Where the model's session ran (set when it's built): "CPU" or the card's provider.
static EP: OnceLock<String> = OnceLock::new();

fn session() -> Option<&'static std::sync::Mutex<ort::session::Session>> {
    static SESSION: OnceLock<Option<std::sync::Mutex<ort::session::Session>>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let env = crate::mask::ort_env()?;
            let path = crate::mask::find_model(FILE)?;
            // RAPIDONE_HQ_CPU=1: as a machine without a graphics card would (testing).
            let cpu_only = std::env::var("RAPIDONE_HQ_CPU").is_ok() || std::env::var("RAPIDONE_DEPTH_CPU").is_ok();
            #[cfg(any(windows, target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
            if !cpu_only {
                let probe = vec![f16::from_f32(0.5); 3 * 518 * 518];
                if let Some((s, device)) = crate::ortgpu::fastest_with(env, &path, 3.0, |s| run(s, probe.clone(), 518).is_some()) {
                    let _ = EP.set(crate::ortgpu::ep_name(device));
                    return Some(std::sync::Mutex::new(s));
                }
            }
            let _ = EP.set("CPU".to_string());
            Some(std::sync::Mutex::new(crate::ortgpu::build(env, &path, None)?))
        })
        .as_ref()
}

/// Whether the High quality model is installed (and runs: its session is built and kept).
pub fn available() -> bool {
    session().is_some()
}

/// Whether the file the app runs is installed, without building (or running) it. The 4-bit export
/// (`Q4_FILE`) is not used: it was slower on the card and no faster on the CPU here.
pub fn installed() -> Option<PathBuf> {
    crate::mask::find_model(FILE)
}

/// What it runs on ("CPU", or the card's provider), once it has run or been set up.
pub fn execution_provider() -> &'static str {
    let _ = session();
    EP.get().map_or("CPU", |s| s.as_str())
}

/// One pass: a `side` x `side` float16 picture in, the square disparity out.
fn run(session: &std::sync::Mutex<ort::session::Session>, input: Vec<f16>, side: usize) -> Option<Vec<f32>> {
    let tensor = ort::value::Tensor::from_array(([1usize, 3, side, side], input.into_boxed_slice())).ok()?;
    let mut session = session.lock().ok()?;
    let outputs = session.run(ort::inputs!["input" => tensor]).ok()?;
    let out = outputs.get("depth")?;
    let (shape, data) = out.try_extract_tensor::<f16>().ok()?;
    let dims: Vec<usize> = shape.iter().map(|&d| d.max(0) as usize).collect();
    let (ow, oh) = (*dims.get(dims.len() - 1)?, *dims.get(dims.len() - 2)?);
    (ow == side && oh == side && data.len() == side * side).then(|| data.iter().map(|v| v.to_f32()).collect())
}

/// The picture as the model sees it, letterboxed with its own edge pixels to a `side` x `side`
/// square: ImageNet-normalised RGB 0..1, channels first, float16. Also returns where the picture
/// itself sits in the square, so its output can be cropped back (the padding's values are edge
/// repeats, and are thrown away).
fn tensor(image: &SceneImage, rect: (usize, usize, usize, usize), side: usize, white: f32) -> (Vec<f16>, (usize, usize, usize, usize)) {
    let (x0, y0, rw, rh) = rect;
    let (rw, rh) = (rw.max(1), rh.max(1));
    let (iw, ih) = if rw >= rh { (side, ((side * rh) / rw).clamp(1, side)) } else { (((side * rw) / rh).clamp(1, side), side) };
    let (px, py) = ((side - iw) / 2, (side - ih) / 2);
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let mut input = vec![f16::from_f32(0.0); 3 * side * side];
    for y in 0..side {
        // The picture's own row, clamped at its edges: padding is a repeat of the last pixel.
        let sy = y0 + (((y as isize - py as isize).clamp(0, ih as isize - 1) as usize) * rh) / ih;
        let sy = sy.min(image.height.saturating_sub(1));
        for x in 0..side {
            let sx = x0 + (((x as isize - px as isize).clamp(0, iw as isize - 1) as usize) * rw) / iw;
            let sx = sx.min(image.width.saturating_sub(1));
            let p = image.px[sy * image.width + sx];
            for c in 0..3 {
                // As the other models see the picture: brightness so the 99th percentile is near
                // white, then gamma (the model was trained on sRGB-like 0..1 values).
                let v = (p[c] / white).clamp(0.0, 1.0).powf(1.0 / 2.2);
                input[c * side * side + y * side + x] = f16::from_f32((v - MEAN[c]) / STD[c]);
            }
        }
    }
    (input, (px, py, iw, ih))
}

/// The model's inverse depth (disparity: larger = nearer) over a window of the picture —
/// `(x0, y0, w, h)` in its own pixels, or the whole picture — resampled to a `w` x `h` grid, with
/// the picture's white point (as `depth::white_point`). Sky is 0 (the export's own sky head).
/// None without the model.
pub fn inverse_depth_white(image: &SceneImage, rect: (usize, usize, usize, usize), w: usize, h: usize, white: f32) -> Option<Grid> {
    let session = session()?;
    let side = SIZE;
    let (input, (px, py, iw, ih)) = tensor(image, rect, side, white.max(1e-4));
    let raw = run(session, input, side)?;
    // Cropped back to the picture: the square's padding is not part of it.
    let mut data = Vec::with_capacity(iw * ih);
    for y in 0..ih {
        for x in 0..iw {
            data.push(raw[(py + y) * side + px + x]);
        }
    }
    let small = Grid { w: iw, h: ih, data };
    Some(resample(&small, w, h))
}

/// A grid resampled (bilinear) to `w` x `h`.
pub fn resample(grid: &Grid, w: usize, h: usize) -> Grid {
    if grid.w == w && grid.h == h {
        return Grid { w: grid.w, h: grid.h, data: grid.data.clone() };
    }
    let (w, h) = (w.max(1), h.max(1));
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            data.push(grid.at((x as f32 + 0.5) / w as f32, v));
        }
    }
    Grid { w, h, data }
}

// --- The download --------------------------------------------------------------------------------

/// The first bytes of the 4-bit file's URL, for a note (the download itself is `download`).
pub fn size_text() -> String {
    format!("{:.0} MB", BYTES as f64 / 1e6)
}

/// SHA-256 of a file, lowercase hex.
pub fn file_sha256(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `path`'s SHA-256 is the one above.
pub fn sha256_ok(path: &Path) -> bool {
    if SHA256.is_empty() {
        return true;
    }
    file_sha256(path).is_some_and(|h| h == SHA256)
}

/// Downloads the model into `dir` (the app's own `models/` folder), resuming a `.part` file, then
/// checking its SHA-256 before it's renamed into place. `progress(done, total)` is called as it
/// goes (total 0 when the server doesn't say). Returns the file's path.
pub fn download(dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let dest = dir.join(FILE);
    if dest.is_file() && sha256_ok(&dest) {
        progress(BYTES, BYTES);
        return Ok(dest);
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{FILE}.part"));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > BYTES {
        // A part file from something else: start again.
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    if have < BYTES {
        // The system's TLS (native-tls), not ureq's default rustls: the app already carries it.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build())
            .build()
            .into();
        let mut request = agent.get(URL);
        if have > 0 {
            request = request.header("Range", format!("bytes={have}-"));
        }
        let mut response = request.call().map_err(|e| format!("download failed: {e}"))?;
        // A server that ignores the range sends the whole file again (200, not 206): start over.
        if have > 0 && response.status().as_u16() != 206 {
            have = 0;
        }
        let total = response.body_mut().content_length().map(|n| n + have).unwrap_or(BYTES);
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&part).map_err(|e| e.to_string())?;
        if have == 0 {
            out.set_len(0).map_err(|e| e.to_string())?;
        }
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| format!("download failed: {e}"))?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
            have += n as u64;
            progress(have, total);
        }
        drop(out);
    }
    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if got != BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the download ended at {got} bytes, not {BYTES}; try again"));
    }
    if !sha256_ok(&part) {
        let _ = std::fs::remove_file(&part);
        return Err("the download's SHA-256 doesn't match: deleted, try again".to_string());
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sha_256_matches_the_published_digests() {
        // The digests the algorithm's own test vectors give, so a download's check is the real thing.
        let dir = std::env::temp_dir().join(format!("rapidone-hq-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.bin");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(file_sha256(&path).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(file_sha256(&path).unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        // A file that isn't the model is refused before it's ever run.
        assert!(!sha256_ok(&path), "something that isn't the model");
        // And the real thing passes (when it's here, as on a machine that has run the model).
        if let Some(model) = installed() {
            assert!(sha256_ok(&model), "the model file's own digest");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_picture_is_letterboxed_with_its_own_edges_not_black() {
        // A 4 x 2 picture of two values, seen at 8 x 8: 8 wide, 4 tall, padded top and bottom with
        // a repeat of its own edge rows (ImageNet-normalised, so the check is the sign of the two).
        let mut px = vec![[0.0f32; 3]; 8];
        for p in px.iter_mut() {
            p[0] = 0.9;
            p[1] = 0.9;
            p[2] = 0.9;
        }
        let image = SceneImage { width: 4, height: 2, px, source_long: 4 };
        // A white point of 1: the values are used as they are.
        let (input, (px, py, iw, ih)) = tensor(&image, (0, 0, 4, 2), 8, 1.0);
        assert_eq!((px, py, iw, ih), (0, 2, 8, 4));
        // The top row of the square is a repeat of the picture's first row, not a black border.
        let at = |x: usize, y: usize| input[y * 8 + x].to_f32();
        assert!(at(0, 0) > 1.0, "{}", at(0, 0));
        assert!((at(0, 0) - at(0, 2)).abs() < 1e-3, "the padding repeats the edge");
        assert!((at(4, 7) - at(4, 2)).abs() < 1e-3);
        // The normalisation is the model's own.
        let expect = (0.9f32.powf(1.0 / 2.2) - 0.485) / 0.229;
        assert!((at(0, 2) - expect).abs() < 0.01, "{} vs {expect}", at(0, 2));
    }

    #[test]
    fn a_grid_resamples_to_any_size() {
        let g = Grid { w: 2, h: 1, data: vec![0.0, 1.0] };
        let r = resample(&g, 5, 3);
        assert_eq!((r.w, r.h), (5, 3));
        // The ends stay the ends (a one-row grid has nowhere to interpolate), and every output row
        // is the same: the source has one row.
        assert!(r.data[0].abs() < 1e-6, "{}", r.data[0]);
        assert!((r.data[4] - 1.0).abs() < 1e-6, "{}", r.data[4]);
        assert!((r.data[1] - 0.2).abs() < 0.1, "{}", r.data[1]);
        for x in 0..5 {
            assert!((r.data[x] - r.data[5 + x]).abs() < 1e-6 && (r.data[x] - r.data[10 + x]).abs() < 1e-6, "column {x}");
        }
    }
}
