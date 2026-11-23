//! Unified Debug Mode, Performance Probes, Native Rust Panic Hook, and Frontend Error Ingestion.
//!
//! - Zero-cost fast path when Debug Mode is OFF (single relaxed `AtomicBool` load).
//! - Native Rust panics, `ERROR`/`WARN` logs, and frontend crash reports are ALWAYS captured.
//! - Thread-safe bounded ring buffer (2,000 entries) + optional 10 MiB rotating log file.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const RING_CAPACITY: usize = 2000;
const MAX_RING_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIENT_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_CLIENT_DETAIL_BYTES: usize = 64 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB rotation cap

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);
static PANIC_HOOK_ONCE: Once = Once::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub level: String,
    pub source: String,
    pub probe: Option<String>,
    pub duration_us: Option<u64>,
    pub message: String,
    pub detail: Option<String>,
}

struct TelemetrySink {
    ring: VecDeque<DebugLogEntry>,
    ring_bytes: usize,
    log_file_path: Option<PathBuf>,
}

static SINK: OnceLock<Mutex<TelemetrySink>> = OnceLock::new();

fn sink() -> &'static Mutex<TelemetrySink> {
    SINK.get_or_init(|| {
        Mutex::new(TelemetrySink {
            ring: VecDeque::with_capacity(RING_CAPACITY),
            ring_bytes: 0,
            log_file_path: None,
        })
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Enables or disables verbose debug mode and performance probe emission at runtime.
pub fn set_debug_mode(enabled: bool) {
    let prev = DEBUG_ENABLED.swap(enabled, Ordering::SeqCst);
    if prev != enabled {
        record_event_always(
            "INFO",
            "rust.debug",
            Some("debug.mode_toggle"),
            None,
            format!("debug mode switched to {}", if enabled { "ON" } else { "OFF" }),
            None,
        );
    }
}

/// Fast-path check for whether verbose debug logging & probes are active.
#[inline]
pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Configures a rotating disk log file (e.g. `<data_dir>/lanvault_debug.log`).
pub fn configure_debug_log_file(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut guard) = sink().lock() {
        guard.log_file_path = Some(path.to_path_buf());
    }
}

/// Installs a global Rust panic hook that records native panics + backtraces into the unified log.
pub fn install_rust_panic_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown".to_string());
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let bt = std::backtrace::Backtrace::force_capture();
            record_event_always(
                "FATAL",
                "rust.panic",
                Some("panic.hook"),
                None,
                format!("panic on thread `{thread_name}` at {location}: {payload}"),
                Some(format!("{bt}")),
            );
            prev_hook(info);
        }));
    });
}

fn write_file_line(path: &Path, entry: &DebugLogEntry) {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() >= MAX_LOG_FILE_BYTES {
            let backup = path.with_extension("log.1");
            let _ = fs::rename(path, backup);
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        if let Ok(line) = serde_json::to_string(entry) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Records an event unconditionally (used for ERROR/WARN/FATAL, panics, and explicit client reports).
pub fn record_event_always(
    level: &str,
    source: &str,
    probe: Option<&str>,
    duration_us: Option<u64>,
    message: impl Into<String>,
    detail: Option<String>,
) {
    let entry = DebugLogEntry {
        seq: NEXT_SEQ.fetch_add(1, Ordering::Relaxed),
        timestamp_ms: now_ms(),
        level: level.to_uppercase(),
        source: truncate_utf8(source.to_string(), 128),
        probe: probe.map(str::to_string),
        duration_us,
        message: truncate_utf8(message.into(), MAX_CLIENT_MESSAGE_BYTES),
        detail: detail.map(|value| truncate_utf8(value, MAX_CLIENT_DETAIL_BYTES)),
    };
    if let Ok(mut guard) = sink().lock() {
        let entry_bytes = serialized_entry_bytes(&entry);
        while guard.ring.len() >= RING_CAPACITY
            || guard.ring_bytes.saturating_add(entry_bytes) > MAX_RING_BYTES
        {
            let Some(removed) = guard.ring.pop_front() else {
                break;
            };
            guard.ring_bytes = guard
                .ring_bytes
                .saturating_sub(serialized_entry_bytes(&removed));
        }
        guard.ring.push_back(entry.clone());
        guard.ring_bytes = guard.ring_bytes.saturating_add(entry_bytes);
    }
    let path = sink()
        .lock()
        .ok()
        .and_then(|guard| guard.log_file_path.clone());
    if let Some(path) = path {
        write_file_line(&path, &entry);
    }
}

/// Records a verbose DEBUG event only when Debug Mode is ON.
#[inline]
pub fn record_debug(
    source: &str,
    probe: Option<&str>,
    duration_us: Option<u64>,
    message: impl FnOnce() -> String,
    detail: Option<String>,
) {
    if !is_debug_enabled() {
        return;
    }
    record_event_always("DEBUG", source, probe, duration_us, message(), detail);
}

/// Ingests an error or diagnostic log from Flutter or Web UI into the unified Rust log stream.
pub fn record_frontend_log(
    level: &str,
    source: &str,
    message: String,
    stack_or_detail: Option<String>,
) {
    let source = match source {
        "flutter" | "web" | "rust.ffi" | "frontend" => source.to_string(),
        _ => "frontend.unknown".to_string(),
    };
    let upper = level.trim().to_uppercase();
    let normalized = match upper.as_str() {
        "FATAL" | "ERROR" | "WARN" | "INFO" | "DEBUG" => upper,
        _ => "INFO".to_string(),
    };
    if normalized == "DEBUG" && !is_debug_enabled() {
        return;
    }
    record_event_always(
        &normalized,
        &source,
        Some("frontend.error_hook"),
        None,
        message,
        stack_or_detail,
    );
}

/// Returns up to `limit` most recent log entries (oldest -> newest within the slice).
pub fn get_debug_logs(limit: usize) -> Vec<DebugLogEntry> {
    let cap = limit.clamp(1, RING_CAPACITY);
    let Ok(guard) = sink().lock() else {
        return Vec::new();
    };
    let len = guard.ring.len();
    let skip = len.saturating_sub(cap);
    guard.ring.iter().skip(skip).cloned().collect()
}

/// Clears the in-memory ring buffer.
pub fn clear_debug_logs() {
    let path = if let Ok(mut guard) = sink().lock() {
        guard.ring.clear();
        guard.ring_bytes = 0;
        guard.log_file_path.clone()
    } else {
        None
    };
    if let Some(path) = path {
        let _ = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path);
        let _ = fs::remove_file(path.with_extension("log.1"));
    }
}

fn serialized_entry_bytes(entry: &DebugLogEntry) -> usize {
    serde_json::to_vec(entry)
        .map(|bytes| bytes.len())
        .unwrap_or_default()
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

/// RAII microsecond performance probe.
/// When Debug Mode is OFF, `ProbeGuard::start` does a single atomic load and avoids `Instant::now()`.
pub struct ProbeGuard {
    source: &'static str,
    probe: &'static str,
    detail: Option<String>,
    started: Option<Instant>,
    bytes: u64,
}

impl ProbeGuard {
    #[inline]
    pub fn start(source: &'static str, probe: &'static str, detail: Option<String>) -> Self {
        let started = if is_debug_enabled() {
            Some(Instant::now())
        } else {
            None
        };
        Self {
            source,
            probe,
            detail,
            started,
            bytes: 0,
        }
    }

    #[inline]
    pub fn add_bytes(&mut self, bytes: u64) {
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            let dur_us = started.elapsed().as_micros() as u64;
            let msg = if self.bytes > 0 {
                let mb_s = if dur_us > 0 {
                    (self.bytes as f64 / (1024.0 * 1024.0)) / (dur_us as f64 / 1_000_000.0)
                } else {
                    0.0
                };
                format!(
                    "{} completed in {} us ({} bytes, {:.1} MiB/s)",
                    self.probe, dur_us, self.bytes, mb_s
                )
            } else {
                format!("{} completed in {} us", self.probe, dur_us)
            };
            record_event_always(
                "PROBE",
                self.source,
                Some(self.probe),
                Some(dur_us),
                msg,
                self.detail.take(),
            );
        }
    }
}
