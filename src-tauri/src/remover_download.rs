//! Downloading Remove's graphics-card model into the app's local data folder
//! (`mask::user_models_dir()`), the first time GPU (Smarter) is chosen. Too big for the installer
//! (over 4 GB, one file alone over GitHub's 2 GB limit), so it's fetched on demand:
//!
//! - **GPU (Smarter)** (`gpumax.rs`, the default): FLUX.2 Klein 4B (Black Forest Labs, Apache-2.0)
//!   with fal's object-remove LoRA (Apache-2.0) fused in, as ONNX, ~4.35 GB, into `gpumax/`.
//! - **RORem** (`rorem.rs`, off since 21 Nov; `RAPIDONE_ROREM=1`): ~2.2 GB into `rorem/`.
//!
//! Files go to `<folder>.download/` first; each is resumed if a part is there, checked against
//! its size and SHA-256 below, and only when all are right is the folder renamed to `<folder>/`,
//! so a half-finished download is never taken for the model. Cancelling keeps what has arrived
//! (the next start carries on from there). Before starting, the free space on the disk is checked.
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// One downloadable model: the folder it installs as, where its files are published, and the files.
pub struct Model {
    /// The folder under the models folder (`gpumax`, `rorem`), as the remover looks for it.
    pub folder: &'static str,
    /// Where the files are published (ends in "/"; each file is at `<url><name>`).
    pub url: &'static str,
    /// An environment variable that replaces `url` (testing: a local server).
    pub url_env: &'static str,
    pub files: &'static [ModelFile],
}

/// GPU (Smarter) as built on 21 Nov 2026 (`~/Work/flux3`: transformer fp16 maths, 8-bit MatMulNBits,
/// MultiHeadAttention fused; VAE fp16; the fixed remove instruction's text-encoder output).
/// Published on Hugging Face (public, Apache-2.0; Chris, 24 Nov).
pub const GPUMAX: Model = Model {
    folder: "gpumax",
    url: "https://huggingface.co/crispin81/rapidone-gpu-remove/resolve/main/",
    url_env: "RAPIDONE_GPUMAX_URL",
    files: &[
        ModelFile { name: "transformer.onnx", size: 4_081_540, sha256: "2f773423fe08fcfaf82c6ca80fcd03952e1d4097d0ed288d1129e52726de0071" },
        ModelFile { name: "transformer.onnx.data", size: 4_162_222_080, sha256: "f5a217f9073fee85f18f62665f33cc35e88f00a861b8717ea4f22375a3a22b02" },
        ModelFile { name: "vae_encoder.onnx", size: 68_956_547, sha256: "dde2ce543c578b91c35e8dfbb07839b8c4cac161d25829319ccd409fc826ce5c" },
        ModelFile { name: "vae_decoder.onnx", size: 99_358_749, sha256: "4431cd8129e1bd9636e43db0f07735648fe0f73f42fe0c75b06ffb7727e33a72" },
        ModelFile { name: "remove_prompt.f32", size: 15_728_640, sha256: "46ef6396bd4a9c9d6d006ebefc757c83406f85e0b58ff338ddc651ad1d8e7687" },
    ],
};

/// GPU (Smarter) for Macs (Apple silicon): the same model with plain fp16 weights instead of 8-bit
/// MatMulNBits. ONNX Runtime's WebGPU runs the 8-bit layers with a slow general kernel on Apple's
/// graphics (95% of the time, 15 s a step at 512 px on an M1 Max); fp16 MatMuls are 4-5x faster
/// (24 Nov 2026). Made from GPUMAX's files by dequantizing (no rebuild); the VAE and prompt are GPUMAX's.
pub const GPUMAX_MAC: Model = Model {
    folder: "gpumax-mac",
    url: "https://huggingface.co/crispin81/rapidone-gpu-remove/resolve/main/mac/",
    url_env: "RAPIDONE_GPUMAX_URL",
    files: &[
        ModelFile { name: "transformer.onnx", size: 4_060_570, sha256: "2889ba360cae7ae3f91df2eeee28a1fe8c752c85a94e4b20cde84a810324e544" },
        ModelFile { name: "transformer.onnx.data", size: 7_751_325_696, sha256: "d0bed464b7c1dcace46a0a4a60784f3e0e649e3051c65a516135fc8c6a81d0f1" },
        ModelFile { name: "vae_encoder.onnx", size: 68_956_547, sha256: "dde2ce543c578b91c35e8dfbb07839b8c4cac161d25829319ccd409fc826ce5c" },
        ModelFile { name: "vae_decoder.onnx", size: 99_358_749, sha256: "4431cd8129e1bd9636e43db0f07735648fe0f73f42fe0c75b06ffb7727e33a72" },
        ModelFile { name: "remove_prompt.f32", size: 15_728_640, sha256: "46ef6396bd4a9c9d6d006ebefc757c83406f85e0b58ff338ddc651ad1d8e7687" },
    ],
};

/// GPU (Smarter)'s model on this kind of machine: the fp16 one on a Mac, the 8-bit one elsewhere.
pub fn gpumax_model() -> &'static Model {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) { &GPUMAX_MAC } else { &GPUMAX }
}

/// RORem as built on 20 Nov 2026 (4-bit MatMuls, block 64, accuracy level 4; 12-channel conv_in).
pub const ROREM: Model = Model {
    folder: "rorem",
    url: "https://huggingface.co/crispin81/rorem-onnx/resolve/main/",
    url_env: "RAPIDONE_ROREM_URL",
    files: &[
        ModelFile { name: "unet.onnx", size: 3_304_974, sha256: "926fcd65bfd2dae47ac179202a7a622f7f87c85bb2111f2c63da281e40843dae" },
        ModelFile { name: "unet.onnx.data", size: 1_892_325_120, sha256: "21b97ab5b23a28b47bc5203da59fa7c3a00a9f1da0268cf826b7ff79d6e7d9c9" },
        ModelFile { name: "vae_encoder.onnx", size: 136_757_697, sha256: "3215686a5ee0371ae8e4e5d7033f0028d214d95861a0b4e02d8c60ef931eb7f1" },
        ModelFile { name: "vae_decoder.onnx", size: 198_078_383, sha256: "14129b23cfd7a42b3fbb8d555bc60e8434169fd5a32f69a91cdcf70aaef0288a" },
        ModelFile { name: "empty_prompt.f32", size: 635_904, sha256: "3274fa4c35f8f89612a9096cad2a5adac0343dc042b8f2a30d3a63392b1b4873" },
    ],
};

/// The model GPU (Smarter) downloads: Klein, or RORem while that's switched on for testing.
pub fn current() -> &'static Model {
    if crate::removal::rorem_enabled() { &ROREM } else { gpumax_model() }
}

impl Model {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// What `download` returns when it was cancelled (the frontend shows "paused", not an error).
pub const CANCELLED: &str = "cancelled";

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Stops a download in progress (what's been fetched stays, and is resumed next time).
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Where a download stands, for the progress bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Downloading,
    /// Checking a finished file's SHA-256 (~10 s for the 4 GB one).
    Checking,
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Free bytes on the disk holding `path` (None if it can't be told).
fn free_space(path: &Path) -> Option<u64> {
    let path = path.canonicalize().ok()?;
    // Windows: canonicalize gives a verbatim path (\\?\C:\...), which never starts with a disk's
    // mount point (C:\), so the check was skipped there (Master-Control, 24 Nov).
    #[cfg(windows)]
    let path = match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(s) if !s.starts_with("UNC\\") => std::path::PathBuf::from(s),
        _ => path,
    };
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

/// Downloads `model`. `progress(done, total, phase)` in bytes, at most ~10 times a second.
/// Only one download runs at a time; a second call returns an error. Err(CANCELLED) if cancelled.
pub fn download(model: &Model, mut progress: impl FnMut(u64, u64, Phase)) -> Result<(), String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("The GPU remover is already downloading.".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let r = run(model, &mut progress);
    RUNNING.store(false, Ordering::SeqCst);
    r
}

fn run(model: &Model, progress: &mut impl FnMut(u64, u64, Phase)) -> Result<(), String> {
    let root = crate::mask::user_models_dir().ok_or("The app's data folder isn't known yet")?;
    let tmp = root.join(format!("{}.download", model.folder));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("Couldn't make {}: {e}", tmp.display()))?;
    let total = model.total_bytes();
    // What's still to come (parts already here count), plus room to spare: the disk must hold it.
    let have_all: u64 = model.files.iter().map(|f| std::fs::metadata(tmp.join(f.name)).map(|m| m.len().min(f.size)).unwrap_or(0)).sum();
    let need = total - have_all;
    if let Some(free) = free_space(&tmp) {
        if free < need + 500_000_000 {
            return Err(format!(
                "Not enough disk space: the GPU (Smarter) remover needs {} more and {} has {} free.",
                gb(need + 500_000_000),
                root.display(),
                gb(free)
            ));
        }
    }
    let mut done = 0u64;
    progress(have_all, total, Phase::Downloading);
    let mut last = std::time::Instant::now();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(60)))
        // A 404 etc. comes back as a response, so its status can be told apart below.
        .http_status_as_error(false)
        .build()
        .into();
    let base = std::env::var(model.url_env).unwrap_or_else(|_| model.url.to_string());
    for f in model.files {
        let part = tmp.join(f.name);
        let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > f.size {
            let _ = std::fs::remove_file(&part);
            have = 0;
        }
        if have < f.size {
            // A dropped connection or a stalled read is tried again (carrying on from what arrived)
            // up to ATTEMPTS times, waiting 1, 2, 4 s; a refusal (404, 416, 5xx) or cancel is not.
            let mut attempt = 1;
            loop {
                let shown = |have: u64| {
                    // Later files' parts already here count as done.
                    let later: u64 = model.files.iter().skip_while(|g| g.name != f.name).skip(1).map(|g| std::fs::metadata(tmp.join(g.name)).map(|m| m.len().min(g.size)).unwrap_or(0)).sum();
                    done + have + later
                };
                match fetch(&agent, &base, f, &part, &mut |have| {
                    if last.elapsed().as_millis() >= 100 {
                        progress(shown(have), total, Phase::Downloading);
                        last = std::time::Instant::now();
                    }
                }) {
                    Ok(()) => break,
                    Err(Fail::Stop(e)) => return Err(e),
                    Err(Fail::Retry(e)) if attempt >= ATTEMPTS => return Err(e),
                    Err(Fail::Retry(e)) => {
                        eprintln!("remover download: {e} (attempt {attempt} of {ATTEMPTS})");
                        for _ in 0..(10u64 << (attempt - 1)) {
                            if CANCEL.load(Ordering::SeqCst) {
                                return Err(CANCELLED.into());
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        attempt += 1;
                    }
                }
            }
        }
        // Never below what was already here at the start (a resumed bar doesn't jump back).
        progress((done + f.size).max(have_all), total, Phase::Checking);
        if sha256_of(&part)? != f.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} arrived damaged (its checksum didn't match) and was deleted. Try again.", f.name));
        }
        done += f.size;
        progress(done.max(have_all), total, Phase::Downloading);
    }
    let dest = root.join(model.folder);
    // A link (a developer's own copy elsewhere, maybe gone) is replaced, never followed: only the
    // link goes, never what it points to.
    match std::fs::symlink_metadata(&dest) {
        Ok(m) if m.file_type().is_symlink() || m.is_file() => {
            remove_link(&dest).map_err(|e| format!("Couldn't replace {}: {e}", dest.display()))?;
        }
        Ok(_) => std::fs::remove_dir_all(&dest).map_err(|e| format!("Couldn't replace {}: {e}", dest.display()))?,
        Err(_) => {}
    }
    std::fs::rename(&tmp, &dest).map_err(|e| format!("Couldn't finish the install: {e}"))?;
    Ok(())
}

/// Tries per file before a dropped connection is reported.
const ATTEMPTS: u32 = 4;

enum Fail {
    /// Worth trying again at once (the network).
    Retry(String),
    /// Not: cancelled, refused, or the disk.
    Stop(String),
}

/// Fetches `f` into `part`, carrying on from what's there. `on_bytes(have)` as it arrives.
fn fetch(agent: &ureq::Agent, base: &str, f: &ModelFile, part: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), Fail> {
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let url = format!("{base}{}", f.name);
    let mut req = agent.get(&url);
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| Fail::Retry(format!("Couldn't reach the download ({e}). Check the internet connection and try again.")))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Err(Fail::Stop("The GPU (Smarter) remover isn't available to download yet.".into()));
    }
    if status == 416 {
        // The range is past the end: the part is bad; start the file again next time.
        let _ = std::fs::remove_file(part);
        return Err(Fail::Stop(format!("{} has to start again. Try again.", f.name)));
    }
    if !(200..300).contains(&status) {
        return Err(Fail::Stop(format!("The download server answered {status} for {}. Try again later.", f.name)));
    }
    // 206: the rest of a partial file; 200: the whole file (the server ignored the range).
    let append = status == 206 && have > 0;
    if !append {
        have = 0;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .map_err(|e| Fail::Stop(format!("Couldn't write {} ({e})", part.display())))?;
    let mut body = resp.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = out.flush();
            return Err(Fail::Stop(CANCELLED.into()));
        }
        let n = body.read(&mut buf).map_err(|e| Fail::Retry(format!("The download of {} broke off ({e}). Try again to carry on from here.", f.name)))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| Fail::Stop(format!("Couldn't write {} ({e}). Is the disk full?", part.display())))?;
        have += n as u64;
        on_bytes(have);
    }
    out.flush().map_err(|e| Fail::Stop(e.to_string()))?;
    if have != f.size {
        return Err(Fail::Retry(format!("{} came in at {have} bytes, not {}. Try again to carry on from here.", f.name, f.size)));
    }
    Ok(())
}

/// Removes a link (or a stray file) itself. On Windows a directory link needs remove_dir.
fn remove_link(p: &Path) -> std::io::Result<()> {
    std::fs::remove_file(p).or_else(|e| if cfg!(windows) { std::fs::remove_dir(p) } else { Err(e) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_add_up() {
        for m in [&GPUMAX, &ROREM] {
            assert_eq!(m.files.len(), 5);
            assert!(m.files.iter().all(|f| f.sha256.len() == 64 && f.size > 0));
            assert!(m.url.ends_with('/'));
        }
        // ~2.2 GB: the 4-bit UNet, not the 5.1 GB fp16 one.
        assert!((2_000_000_000..2_400_000_000).contains(&ROREM.total_bytes()));
        // ~4.35 GB: Klein 4B 8-bit + the fp16 VAE.
        assert!((4_300_000_000..4_400_000_000).contains(&GPUMAX.total_bytes()));
        // gpumax::folder() looks for exactly these files.
        let mut names: Vec<_> = GPUMAX.files.iter().map(|f| f.name).collect();
        names.sort();
        assert_eq!(names, ["remove_prompt.f32", "transformer.onnx", "transformer.onnx.data", "vae_decoder.onnx", "vae_encoder.onnx"]);
    }

    #[test]
    fn free_space_is_known() {
        // The disk-space check before a download: the temp folder's disk has some room.
        assert!(free_space(&std::env::temp_dir()).is_some_and(|b| b > 0));
    }
}
