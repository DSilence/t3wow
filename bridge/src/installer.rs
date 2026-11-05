//! What double-clicking the downloaded app does: install it (`~/Applications`
//! on macOS, `%LOCALAPPDATA%\t3wow` plus a Start Menu shortcut on Windows),
//! or update an older install, then start the installed tray app with
//! Settings open. The first time, the tray app also installs the addon into
//! the World of Warcraft folders it finds.

use crate::app::{self, TrayRequest};
use crate::config::BRIDGE_VERSION;
use crate::error::Result;
use crate::lock::BridgeLock;
use crate::{log, service, update};

pub fn install_and_start() -> Result<()> {
    #[cfg(windows)]
    crate::win::detach_console();
    crate::log::log_to_file(service::log_file())?;
    // None when the app is not installed (or its binary does not start).
    let installed = app::installed_version().filter(|_| app::installed());
    let first_run = installed.is_none();
    // Never replace a newer install with an older download.
    let replace = installed.as_deref().is_none_or(|version| update::is_newer(BRIDGE_VERSION, version));
    if replace {
        log!("installing the app {BRIDGE_VERSION} (installed: {})", installed.as_deref().unwrap_or("none"));
        service::remove_legacy()?;
        app::install()?;
    }
    if BridgeLock::held() {
        // The tray app is running: it shows Settings, or restarts into the new version.
        let request = if replace && installed.is_some() { TrayRequest::Restart } else { TrayRequest::OpenSettings };
        if app::hand_over_to_tray(request)? {
            return Ok(());
        }
    }
    let args: &[&str] = if first_run { &["--settings", "--first-run"] } else { &["--settings"] };
    app::launch(args)
}

/// Show what went wrong; there is no terminal to print it to.
pub fn show_error(message: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("T3 WoW")
        .set_description(format!("Could not install T3 WoW: {message}"))
        .show();
}
