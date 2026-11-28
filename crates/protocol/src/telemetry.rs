//! Lightweight session telemetry shared by host, viewer and transport.
//!
//! Lines are appended to `%LOCALAPPDATA%\AgentDesk\logs\<role>-YYYY-MM-DD.log` (falls back to the
//! temp dir), one timestamped line per event, so a slow session can be diagnosed afterwards by
//! lining up the host, viewer and server (relay) logs by time.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

/// Directory the session logs are written to.
pub fn log_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("PROGRAMDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("AgentDesk").join("logs")
}

/// Appends `line` to the `role` log (e.g. "host", "viewer"). Never panics, never blocks long.
pub fn tlog(role: &str, line: impl AsRef<str>) {
    let now = chrono::Local::now();
    let dir = log_dir();
    let path = dir.join(format!("{role}-{}.log", now.format("%Y-%m-%d")));
    let _g = LOCK.lock();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [{role}] {}", now.format("%H:%M:%S%.3f"), line.as_ref());
    }
    if FORWARD.load(std::sync::atomic::Ordering::Relaxed) {
        if let Ok(mut q) = PENDING.lock() {
            if q.len() < MAX_PENDING {
                q.push((role.to_string(), format!("{} {}", now.format("%H:%M:%S%.3f"), line.as_ref())));
            }
        }
    }
}

/// Lines waiting to be sent to the server (bounded: a host offline for hours must not grow).
static PENDING: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static FORWARD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const MAX_PENDING: usize = 600;

/// Also queue every telemetry line for the server (the app's agent connection drains it).
pub fn enable_forwarding() {
    FORWARD.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Takes the queued lines grouped by role, oldest first.
pub fn drain_for_server() -> Vec<(String, Vec<String>)> {
    let lines = match PENDING.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (role, line) in lines {
        match out.iter_mut().find(|(r, _)| *r == role) {
            Some((_, v)) => v.push(line),
            None => out.push((role, vec![line])),
        }
    }
    out
}

/// Simple running stats bucket (count / sum / max) reset on every report.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stat {
    pub n: u64,
    pub sum: f64,
    pub max: f64,
}

impl Stat {
    pub fn add(&mut self, v: f64) {
        self.n += 1;
        self.sum += v;
        if v > self.max {
            self.max = v;
        }
    }
    pub fn avg(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.sum / self.n as f64 }
    }
    pub fn take(&mut self) -> Stat {
        std::mem::take(self)
    }
}
