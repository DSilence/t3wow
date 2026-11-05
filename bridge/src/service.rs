//! The login service earlier versions installed (a LaunchAgent on macOS, a
//! systemd user unit on Linux). The bridge now runs as a tray app you start
//! yourself; this only finds and removes the old service and its copy of the
//! binary.

use std::path::PathBuf;
#[cfg(unix)]
use std::{fs, path::Path, process::Command};

use crate::config::home_dir;
#[cfg(unix)]
use crate::config::user_home;
use crate::error::Result;

pub fn log_file() -> PathBuf {
    home_dir().join("bridge.log")
}

/// Where the service kept its own copy of the binary.
#[cfg(unix)]
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

/// Whether the old login service is still set up. No release ever set one
/// up on Windows, so it is not looked for there: the code that did (a Startup
/// folder script, stopped with wmic) only made the binary look like malware.
pub fn legacy_installed() -> bool {
    #[cfg(unix)]
    return service_file().exists();
    #[cfg(windows)]
    return false;
}

/// Stop and remove the old login service. Returns whether there was one.
#[cfg(windows)]
pub fn remove_legacy() -> Result<bool> {
    Ok(false)
}

/// Stop and remove the old login service. Returns whether there was one.
#[cfg(unix)]
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

#[cfg(unix)]
fn remove_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
