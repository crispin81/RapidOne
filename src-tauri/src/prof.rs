//! Timing of the render's stages, for speed work: with `RAPIDONE_PROFILE=1` set, each `span` prints
//! its wall-clock time and the processor time the whole program used meanwhile (all cores) when it
//! ends. Off (the default) it costs one check.

use std::sync::OnceLock;
use std::time::Instant;

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RAPIDONE_PROFILE").is_some())
}

/// Processor time used so far by this program (user + system, all threads), in seconds (Linux).
fn cpu() -> f32 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = stat.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    let tick = |i: usize| f.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    (tick(11) + tick(12)) / 100.0
}

pub struct Span {
    name: &'static str,
    start: Option<(Instant, f32)>,
}

/// Times from now until the returned guard is dropped.
pub fn span(name: &'static str) -> Span {
    Span { name, start: on().then(|| (Instant::now(), cpu())) }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((t, c)) = self.start {
            eprintln!("[profile] {:<28} {:>6.3} s  ({:.1} s processor)", self.name, t.elapsed().as_secs_f32(), cpu() - c);
        }
    }
}
