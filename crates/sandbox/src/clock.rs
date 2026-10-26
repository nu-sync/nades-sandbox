//! The start-up clock: stage marks in milliseconds since `main`, plus how long the process had been alive before it
//! (from the kernel on macOS), so a first frame can be stated from the process start.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Clock {
    start: Instant,
    pre_main_ms: f64,
    marks: Mutex<Vec<(&'static str, f64)>>,
}

static CLOCK: OnceLock<Clock> = OnceLock::new();

pub fn init() {
    CLOCK.get_or_init(|| Clock { start: Instant::now(), pre_main_ms: pre_main_ms().unwrap_or(f64::NAN), marks: Mutex::new(Vec::new()) });
}

fn clock() -> &'static Clock {
    CLOCK.get().expect("clock::init runs first in main")
}

/// Records `stage` now; returns its time in ms since `main`.
pub fn mark(stage: &'static str) -> f64 {
    let at = clock().start.elapsed().as_secs_f64() * 1e3;
    clock().marks.lock().expect("marks").push((stage, at));
    at
}

/// The stages so far, in order.
pub fn marks() -> Vec<(&'static str, f64)> {
    clock().marks.lock().expect("marks").clone()
}

/// ms from the process start to `main` (dynamic linking, Rust's runtime set-up), or NaN where it cannot be read.
pub fn pre_main() -> f64 {
    clock().pre_main_ms
}

/// The marks and the pre-main time as one JSON line, for timing runs.
pub fn json() -> String {
    let marks: Vec<String> = marks().iter().map(|(stage, at)| format!("[\"{stage}\",{at:.2}]")).collect();
    let pre = if pre_main().is_finite() { format!("{:.2}", pre_main()) } else { "null".into() };
    format!("{{\"pre_main_ms\":{pre},\"marks\":[{}]}}", marks.join(","))
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
