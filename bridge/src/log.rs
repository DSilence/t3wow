//! Timestamped log lines to stdout, or to `~/.t3wow/bridge.log` when the
//! bridge runs as a service.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

static LOG_FILE: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);

/// Send log lines to `path` instead of stdout. Keeps one older file around.
pub fn log_to_file(path: PathBuf) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    *LOG_FILE.lock().unwrap() = Some((path, file));
    Ok(())
}

pub fn write_line(message: &str) {
    let line = format!("{} {message}\n", local_time());
    let mut guard = LOG_FILE.lock().unwrap();
    let Some((path, file)) = guard.as_mut() else {
        print!("{line}");
        return;
    };
    if file.metadata().is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&*path, path.with_extension("log.old"));
        if let Ok(fresh) = OpenOptions::new().create(true).append(true).open(&*path) {
            *file = fresh;
        }
    }
    let _ = file.write_all(line.as_bytes());
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write_line(&format!($($arg)*)) };
}

#[cfg(unix)]
fn local_time() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(not(unix))]
fn local_time() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02} UTC", day / 3600, day / 60 % 60, day % 60)
}
