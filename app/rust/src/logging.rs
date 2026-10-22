//! A log file in the app's private directory, which the status screen can
//! show. Android discards a native library's standard output, so without
//! this there would be no way to see what went wrong on a phone.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Start a fresh file once the old one grows past this.
const MAX_LOG_BYTES: u64 = 2 << 20;

pub fn path(dir: &Path) -> PathBuf {
    dir.join("osp.log")
}

/// Sends tracing output to the log file. Safe to call more than once.
pub fn init(dir: &Path) {
    let path = path(dir);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_env_filter("osp_client=debug,osp_mobile=debug,iroh=warn,warn")
        .try_init();
}

/// The last `lines` lines of the log.
pub fn tail(dir: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path(dir)).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
