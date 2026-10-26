//! The start-up clock: stage marks in milliseconds since `main`, plus how long the process had been alive before it
//! (from the kernel on macOS), so a first frame can be stated from the process start.

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use nades_render::memory;

struct Clock {
    start: Instant,
    pre_main_ms: f64,
    marks: Mutex<Vec<(&'static str, f64)>>,
    /// The process's memory at each mark (macOS).
    memory: Mutex<Vec<(&'static str, memory::Footprint)>>,
}

static CLOCK: OnceLock<Clock> = OnceLock::new();
static LOG_MARKS: AtomicBool = AtomicBool::new(false);

pub fn enable_logs() {
    LOG_MARKS.store(true, Ordering::Relaxed);
}

pub fn init() {
    CLOCK.get_or_init(|| Clock { start: Instant::now(), pre_main_ms: pre_main_ms().unwrap_or(f64::NAN), marks: Mutex::new(Vec::new()), memory: Mutex::new(Vec::new()) });
}

fn clock() -> &'static Clock {
    CLOCK.get().expect("clock::init runs first in main")
}

/// Records `stage` now; returns its time in ms since `main`.
pub fn mark(stage: &'static str) -> f64 {
    let at = clock().start.elapsed().as_secs_f64() * 1e3;
    clock().marks.lock().expect("marks").push((stage, at));
    if let Some(footprint) = memory::footprint() {
        clock().memory.lock().expect("memory").push((stage, footprint));
    }
    if LOG_MARKS.load(Ordering::Relaxed) {
        eprintln!("{}", serde_json::json!({ "event": "startup_stage", "stage": stage, "elapsed_ms": at }));
    }
    at
}

/// Records `stage` as having happened `ago` ms before now (a moment another thread learns of late); returns its time
/// in ms since `main`.
pub fn mark_ago(stage: &'static str, ago: f64) -> f64 {
    let at = clock().start.elapsed().as_secs_f64() * 1e3 - ago;
    let mut marks = clock().marks.lock().expect("marks");
    let place = marks.iter().position(|&(_, t)| t > at).unwrap_or(marks.len());
    marks.insert(place, (stage, at));
    at
}

/// The process's memory at each mark so far, in order.
pub fn memory() -> Vec<(&'static str, memory::Footprint)> {
    clock().memory.lock().expect("memory").clone()
}

/// The stages so far, in order.
pub fn marks() -> Vec<(&'static str, f64)> {
    clock().marks.lock().expect("marks").clone()
}

/// ms from the process start to `main` (dynamic linking, Rust's runtime set-up), or NaN where it cannot be read.
pub fn pre_main() -> f64 {
    clock().pre_main_ms
}

/// The marks and the pre-main time as one JSON line, for timing runs, with the memory at each mark (footprint and its
/// graphics share, MB) and the footprint's peak so far where it can be read.
pub fn json() -> String {
    let marks: Vec<String> = marks().iter().map(|(stage, at)| format!("[\"{stage}\",{at:.2}]")).collect();
    let pre = if pre_main().is_finite() { format!("{:.2}", pre_main()) } else { "null".into() };
    let memory: Vec<String> = memory().iter().map(|(stage, f)| format!("[\"{stage}\",{},{}]", f.now_mb(), memory::mb(f.graphics))).collect();
    let peak = memory::footprint().map_or("null".into(), |f| memory::mb(f.peak).to_string());
    format!("{{\"pre_main_ms\":{pre},\"marks\":[{}],\"memory\":[{}],\"peak_footprint_mb\":{peak}}}", marks.join(","), memory.join(","))
}

#[cfg(target_os = "macos")]
fn pre_main_ms() -> Option<f64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64() * 1e3;
    // SAFETY: proc_pidinfo writes at most `size` bytes into the zeroed struct, and its return value is checked
    let started = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size);
        if written != size {
            return None;
        }
        info.pbi_start_tvsec as f64 * 1e3 + info.pbi_start_tvusec as f64 / 1e3
    };
    Some((now - started).max(0.0))
}

#[cfg(not(target_os = "macos"))]
fn pre_main_ms() -> Option<f64> {
    None
}
