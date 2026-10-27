//! FLUX.2 Klein 4B (Apache-2.0), the Medium removal model, run by a Python
//! worker (`python/klein_worker.py`) that RapidOne starts the first time it's
//! needed and keeps, so the model loads once.
//!
//! The same worker runs SAM 3 (Meta's SAM License), when set up, to find
//! objects from words or a box and outline them exactly (`find`).
//!
//! Where Python and the models live is machine-specific, so it's read from
//! `klein.json` in a models folder (git-ignored):
//!   {"python": "C:/path/to/venv/Scripts/python.exe",
//!    "model": "C:/path/to/models/FLUX.2-klein-4B",
//!    "sam": "C:/path/to/models/sam3.1-comfy/checkpoints/sam3.1_multiplex_fp16.safetensors"}
//! (A bundled environment and a download button come with the release.)
//!
//! GPU memory: the worker (and the models on the card with it) lives only
//! while the Remove tool is in use. Opening Remove loads it (`removal::
//! prepare`); closing it lets it go RELEASE_AFTER later unless Remove opens
//! again (`hold`); quitting RapidOne ends it (`shutdown`). The worker also
//! ends itself if left idle, or if RapidOne goes away without saying (its
//! input closes) - so the models never sit on the card unused.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

#[derive(Deserialize)]
struct Config {
    python: String,
    model: String,
    /// SAM 3's checkpoint; without it, objects are found by Klein alone.
    #[serde(default)]
    sam: Option<String>,
    /// RORem (the Medium remover): its diffusers folder.
    #[serde(default)]
    rorem: Option<String>,
}

fn config() -> Option<Config> {
    crate::mask::model_dirs()
        .into_iter()
        .map(|d| d.join("klein.json"))
        .find(|p| p.is_file())
        .and_then(|p| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok())
}

/// Whether removal can run on this computer (Klein is set up).
pub fn available() -> bool {
    config().is_some_and(|c| Path::new(&c.python).is_file() && Path::new(&c.model).is_dir())
}

/// Whether RORem (the Medium remover) is set up.
pub fn rorem_available() -> bool {
    config().is_some_and(|c| Path::new(&c.python).is_file() && c.rorem.as_ref().is_some_and(|r| Path::new(r).join("model_index.json").is_file()))
}

/// RORem fills `mask` (a white-on-black PNG the input's size) in `input` (a
/// PNG at width x height), writing `output`. `seed` picks the version.
pub fn rorem(input: &Path, mask: &Path, output: &Path, width: u32, height: u32, seed: u32) -> Result<f32, String> {
    let c = config().ok_or("RORem isn't set up on this computer")?;
    let model = c.rorem.ok_or("RORem isn't set up on this computer (no \"rorem\" in klein.json)")?;
    let job = serde_json::json!({
        "op": "rorem", "model": model, "input": input.to_string_lossy(), "mask": mask.to_string_lossy(),
        "output": output.to_string_lossy(), "width": width, "height": height, "seed": seed,
    });
    let v = request(job)?;
    Ok(v["seconds"].as_f64().unwrap_or(0.0) as f32)
}

/// Whether SAM 3 is set up too.
pub fn sam_available() -> bool {
    config().and_then(|c| c.sam).is_some_and(|s| Path::new(&s).is_file())
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// Whether the Remove tool is open (the worker is wanted).
static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Bumped on every `hold`, so only the latest close lets the worker go.
static HOLDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How long after Remove closes the worker stays (reopening within it is instant).
const RELEASE_AFTER: std::time::Duration = std::time::Duration::from_secs(45);

/// The Remove tool opened (`true`) or closed (`false`). Closed for
/// RELEASE_AFTER, the worker ends and its GPU memory is freed (after any job
/// still running).
pub fn hold(open: bool) {
    use std::sync::atomic::Ordering;
    HELD.store(open, Ordering::SeqCst);
    let n = HOLDS.fetch_add(1, Ordering::SeqCst) + 1;
    if !open {
        std::thread::spawn(move || {
            std::thread::sleep(RELEASE_AFTER);
            if !HELD.load(Ordering::SeqCst) && HOLDS.load(Ordering::SeqCst) == n {
                shutdown();
            }
        });
    }
}

/// Ends the worker now (waiting for a job in progress), freeing its GPU memory.
pub fn shutdown() {
    let mut guard = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut w) = guard.take() {
        // Closing its input ends it cleanly; then make sure.
        drop(w.stdin);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if matches!(w.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = w.child.kill();
        let _ = w.child.wait();
    }
}

/// Whether a worker is running (its models on the card).
pub fn loaded() -> bool {
    WORKER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn script() -> PathBuf {
    // Beside the executable in a release; the source folder in development.
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("python").join("klein_worker.py")));
    beside
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python").join("klein_worker.py"))
}

fn start() -> Result<Worker, String> {
    let c = config().ok_or("Object removal isn't set up on this computer (no klein.json in the models folder).")?;
    let mut cmd = Command::new(&c.python);
    cmd.arg("-I").arg(script()).arg(&c.model);
    if let Some(sam) = c.sam.as_ref().filter(|s| Path::new(s).is_file()) {
        cmd.arg(sam);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start the removal model: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let mut line = String::new();
    stdout.read_line(&mut line).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).map_err(|_| "The removal model didn't start.".to_string())?;
    if v["ready"] != true {
        return Err(v["error"].as_str().unwrap_or("The removal model didn't start.").to_string());
    }
    Ok(Worker { child, stdin, stdout })
}

/// Runs Klein on `input` (a PNG at width x height) with `prompt`, writing
/// `output`; `seed` picks the version (Try again asks for the next one).
/// Returns the seconds it took.
pub fn edit(input: &Path, output: &Path, prompt: &str, width: u32, height: u32, seed: u32) -> Result<f32, String> {
    let job = serde_json::json!({
        "op": "edit", "input": input.to_string_lossy(), "output": output.to_string_lossy(),
        "prompt": prompt, "width": width, "height": height, "seed": seed,
    });
    let v = request(job)?;
    Ok(v["seconds"].as_f64().unwrap_or(0.0) as f32)
}

/// An object SAM 3 found: its outline (a white-on-black PNG the size of the
/// picture it was given), bounding box in that picture's pixels, and score.
pub struct Found {
    pub mask: PathBuf,
    pub bbox: [f32; 4],
    pub score: f32,
}

/// SAM 3 on `image`: every object matching `words`, or (with `bbox`, pixels)
/// the object that box is round. Outlines are written to `out` as
/// `<stem>-mask<n>.png`, most confident first.
pub fn find(image: &Path, words: Option<&str>, bbox: Option<[f32; 4]>, out: &Path, stem: &str) -> Result<Vec<Found>, String> {
    let mut job = serde_json::json!({
        "op": "find", "image": image.to_string_lossy(), "out": out.to_string_lossy(), "stem": stem,
    });
    if let Some(b) = bbox {
        job["box"] = serde_json::json!(b);
    } else {
        job["words"] = serde_json::json!(words.unwrap_or(""));
    }
    let v = request(job)?;
    Ok(v["found"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let b = f["box"].as_array()?;
                    Some(Found {
                        mask: PathBuf::from(f["mask"].as_str()?),
                        bbox: std::array::from_fn(|i| b.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32),
                        score: f["score"].as_f64().unwrap_or(0.0) as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// SAM 3 analyses `image` now and keeps it, so `find` on it is quick.
pub fn prepare(image: &Path) -> Result<(), String> {
    request(serde_json::json!({ "op": "prepare", "image": image.to_string_lossy() })).map(|_| ())
}

/// One job to the worker (started if need be); its reply when it went well.
fn request(job: serde_json::Value) -> Result<serde_json::Value, String> {
    let mut guard = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    // It ends itself when idle a long while: start afresh.
    if guard.as_mut().is_some_and(|w| !matches!(w.child.try_wait(), Ok(None))) {
        *guard = None;
    }
    if guard.is_none() {
        *guard = Some(start()?);
    }
    let result = (|| -> Result<serde_json::Value, String> {
        let w = guard.as_mut().expect("started");
        writeln!(w.stdin, "{job}").map_err(|e| e.to_string())?;
        w.stdin.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        if w.stdout.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("The removal model stopped.".into());
        }
        serde_json::from_str(line.trim()).map_err(|e| e.to_string())
    })();
    match result {
        Ok(v) if v["ok"] == true => Ok(v),
        Ok(v) => Err(v["error"].as_str().unwrap_or("Removal failed.").to_string()),
        Err(e) => {
            // The worker is gone or confused: start afresh next time.
            if let Some(mut w) = guard.take() {
                let _ = w.child.kill();
                let _ = w.child.wait();
            }
            Err(e)
        }
    }
}
