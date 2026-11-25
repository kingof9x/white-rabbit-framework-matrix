//! A small log file, shared by every thread of one program (the monitor writes
//! `WhiteRabbit-monitor.log`, the window `WhiteRabbit-window.log`).
//!
//! How it works:
//! - `init` remembers the file's path once, at start-up. Before that (and in the tests) lines go to
//!   standard error instead.
//! - Each line is written by opening the file in append mode, writing one whole line and closing it
//!   again. That costs a little per line, but the monitor logs only a few lines an hour once it runs,
//!   and it means the file can be opened, read, moved or deleted (Tray > Open log) at any moment.
//! - The file is kept short: once it passes [`MAX_BYTES`] it is renamed to `<name>.old` (replacing
//!   the previous one) and a new file starts. This is checked at start-up and on every write, because
//!   the monitor can run for weeks without a restart.
//! - A global lock keeps lines from two threads from mixing. If a thread ever panics while holding it,
//!   the other threads keep logging (a poisoned lock is taken over, not passed on as a second panic).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The largest the log file gets before it is moved aside and a new one starts.
pub const MAX_BYTES: u64 = 1_000_000;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn file() -> MutexGuard<'static, Option<PathBuf>> {
    FILE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `<name>.old`, where a full log is moved.
pub fn old_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Moves a log that has grown past `max` bytes aside, so writing starts a new one.
fn rotate_if_full(path: &Path, max: u64) {
    if std::fs::metadata(path).map(|m| m.len() > max).unwrap_or(false) {
        let old = old_path(path);
        let _ = std::fs::remove_file(&old);
        if std::fs::rename(path, &old).is_err() {
            // Renaming can fail on Windows while another program has the file open: start again in place
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Where to write from now on. Creates the folder, and moves a full log aside.
pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate_if_full(&path, MAX_BYTES);
    *file() = Some(path);
}

/// Writes one line: the local time, the level and the message.
pub fn write(level: &str, message: &str) {
    let line = format!("{} {level}: {message}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let guard = file();
    match guard.as_ref() {
        Some(path) => append(path, &line, MAX_BYTES),
        None => eprint!("{line}"),
    }
}

/// Appends a line to the log at `path`, starting a new file first when it is past `max` bytes.
fn append(path: &Path, line: &str, max: u64) {
    rotate_if_full(path, max);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::log::write("INFO", &format!($($t)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::log::write("WARNING", &format!($($t)*)) };
}

#[macro_export]
macro_rules! error {
    ($($t:tt)*) => { $crate::log::write("ERROR", &format!($($t)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_log_is_moved_aside_while_running() {
        let dir = std::env::temp_dir().join(format!("wr-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let line = "x".repeat(99) + "\n";
        // 1000 lines of 100 bytes with a 2000-byte limit: the file never grows much past the limit
        for _ in 0..1000 {
            append(&path, &line, 2000);
            assert!(std::fs::metadata(&path).unwrap().len() <= 2000 + 100);
        }
        assert!(old_path(&path).exists(), "the previous log is kept once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
