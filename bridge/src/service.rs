//! The login service earlier versions installed (a LaunchAgent on macOS, a
//! systemd user unit on Linux, a Startup script on Windows). The bridge now
//! runs as a tray app you start yourself; this only finds and removes the old
//! service and its copy of the binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{home_dir, user_home};
use crate::error::Result;

pub fn log_file() -> PathBuf {
    home_dir().join("bridge.log")
}

/// Where the service kept its own copy of the binary.
fn service_binary_dir() -> PathBuf {
    home_dir().join("bin")
}

#[cfg(target_os = "macos")]
const LABEL: &str = "dev.t3wow.bridge";

#[cfg(target_os = "macos")]
fn service_file() -> PathBuf {
    user_home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn stop_service() {
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/{LABEL}")]).output();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn service_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| user_home().join(".config"));
    config.join("systemd/user/t3wow.service")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn stop_service() {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "t3wow.service"]).output();
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).output();
}

#[cfg(windows)]
fn service_file() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| user_home().join("AppData/Roaming"));
    appdata.join(r"Microsoft\Windows\Start Menu\Programs\Startup\t3wow.vbs")
}

#[cfg(windows)]
fn stop_service() {
    use std::os::windows::process::CommandExt;
    // The Startup script ran ~/.t3wow/bin/t3wow.exe; stop that copy only.
    let exe = service_binary_dir().join("t3wow.exe");
    let filter = format!("ExecutablePath='{}'", exe.display().to_string().replace('\\', "\\\\"));
    let _ = Command::new("wmic")
        .args(["process", "where", &filter, "call", "terminate"])
        .creation_flags(0x0800_0000)
        .output();
}

/// Whether the old login service is still set up.
pub fn legacy_installed() -> bool {
    service_file().exists()
}

/// Stop and remove the old login service. Returns whether there was one.
pub fn remove_legacy() -> Result<bool> {
    let file = service_file();
    if !file.exists() {
        return Ok(false);
    }
    stop_service();
    remove_if_present(&file)?;
    let _ = fs::remove_dir_all(service_binary_dir());
    Ok(true)
}

fn remove_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
