//! Something to launch the tray app from: `~/Applications/T3 WoW.app` on
//! macOS (a menu bar app without a Dock icon), a Start Menu shortcut on
//! Windows. Both run their own copy of the binary, so the download or build
//! folder can go away.

use std::fs;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", windows))]
use std::process::Command;

#[cfg(target_os = "macos")]
use crate::config::BRIDGE_VERSION;
use crate::config::{create_private_dir, home_dir, user_home};
use crate::error::{Error, Result};

pub const APP_NAME: &str = "T3 WoW";

/// Made from `assets/icon/app-icon.svg` by `scripts/make-icons.py`.
#[cfg(target_os = "macos")]
const ICNS: &[u8] = include_bytes!("../assets/icon/AppIcon.icns");
#[cfg(windows)]
const ICO: &[u8] = include_bytes!("../assets/icon/t3wow.ico");

#[cfg(target_os = "macos")]
pub fn app_path() -> PathBuf {
    user_home().join("Applications").join(format!("{APP_NAME}.app"))
}

#[cfg(windows)]
pub fn app_path() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| user_home().join("AppData/Roaming"));
    appdata.join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{APP_NAME}.lnk"))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn app_path() -> PathBuf {
    user_home().join(".local/share/applications/t3wow.desktop")
}

/// True when this process runs from the macOS app bundle.
pub fn launched_from_bundle() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"))
}

#[cfg(target_os = "macos")]
fn info_plist() -> String {
    let env = std::env::var_os("T3WOW_HOME")
        .map(|home| {
            format!(
                "  <key>LSEnvironment</key>\n  <dict>\n    <key>T3WOW_HOME</key>\n    <string>{}</string>\n  </dict>\n",
                xml_escape(&home.to_string_lossy())
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>{APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>{APP_NAME}</string>
  <key>CFBundleIdentifier</key>
  <string>dev.t3wow.tray</string>
  <key>CFBundleExecutable</key>
  <string>t3wow</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{BRIDGE_VERSION}</string>
  <key>CFBundleVersion</key>
  <string>{BRIDGE_VERSION}</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>T3 WoW listens while you hold the dictation shortcut, so you can speak messages to your agents.</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>T3 WoW turns what you say while holding the dictation shortcut into text for the game.</string>
{env}</dict>
</plist>
"#
    )
}

#[cfg(target_os = "macos")]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<PathBuf> {
    let app = app_path();
    write_bundle(&app)?;
    Ok(app)
}

/// The binary the installed app runs.
#[cfg(target_os = "macos")]
pub fn installed_binary() -> PathBuf {
    app_path().join("Contents/MacOS/t3wow")
}

/// Write an app bundle running a copy of this binary at `app`. Release builds
/// use it to package `T3 WoW.app` (`t3wow app bundle <path>`).
#[cfg(target_os = "macos")]
pub fn write_bundle(app: &Path) -> Result<()> {
    let current = std::env::current_exe()?;
    // Reinstalling from the bundle itself: copy before removing it.
    let binary = fs::read(&current)?;
    if app.exists() {
        fs::remove_dir_all(app)?;
    }
    let macos = app.join("Contents/MacOS");
    fs::create_dir_all(&macos)?;
    let exe = macos.join("t3wow");
    fs::write(&exe, binary)?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(app.join("Contents/Info.plist"), info_plist())?;
    let resources = app.join("Contents/Resources");
    fs::create_dir_all(&resources)?;
    fs::write(resources.join("AppIcon.icns"), ICNS)?;
    // An ad-hoc signature keeps macOS from treating the bundle as damaged.
    let _ = Command::new("codesign").args(["--force", "--sign", "-"]).arg(app).output();
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn open() -> Result<()> {
    launch(&[])
}

/// Start the installed app with these arguments. A new instance: one that
/// finds the tray already running hands over to it (see `request_tray`).
#[cfg(target_os = "macos")]
pub fn launch(args: &[&str]) -> Result<()> {
    let mut command = Command::new("open");
    command.arg("-n").arg(app_path());
    if !args.is_empty() {
        command.arg("--args").args(args);
    }
    if !command.status()?.success() {
        return Err(Error::other("Could not open the app."));
    }
    Ok(())
}

#[cfg(windows)]
pub fn installed_binary() -> PathBuf {
    let local =
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| user_home().join("AppData/Local"));
    local.join("t3wow").join("t3wow.exe")
}

#[cfg(windows)]
pub fn install() -> Result<PathBuf> {
    let exe = installed_binary();
    fs::create_dir_all(exe.parent().unwrap())?;
    let current = std::env::current_exe()?;
    if current.canonicalize().ok() != exe.canonicalize().ok() {
        // The tray app may be running from it.
        crate::update::replace_file(&exe, &current)?;
    }
    let ico = exe.with_extension("ico");
    fs::write(&ico, ICO)?;
    let link = app_path();
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir)?;
    }
    let shortcut = crate::win::Shortcut {
        target: &exe,
        arguments: "tray",
        description: "T3 Code for World of Warcraft",
        icon: &ico,
    };
    crate::win::create_shortcut(&link, &shortcut)
        .map_err(|error| Error::other(format!("Could not create the Start Menu shortcut: {error}")))?;
    Ok(link)
}

#[cfg(windows)]
pub fn open() -> Result<()> {
    launch(&[])
}

/// Start the installed tray app with these extra arguments, without a console.
#[cfg(windows)]
pub fn launch(args: &[&str]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    Command::new(installed_binary()).arg("tray").args(args).creation_flags(DETACHED_PROCESS).spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install() -> Result<PathBuf> {
    Err(Error::other("The tray app is for macOS and Windows. On Linux run `t3wow run --wait-for-wow`."))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn open() -> Result<()> {
    install().map(|_| ())
}

pub fn installed() -> bool {
    app_path().exists()
}

/// The version of the installed app, if it is installed and starts.
#[cfg(any(target_os = "macos", windows))]
pub fn installed_version() -> Option<String> {
    #[allow(unused_mut)]
    let mut command = Command::new(installed_binary());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.arg("version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    output.status.success().then(|| text.trim().trim_start_matches("t3wow").trim().to_string())
}

/// Whether this process runs from the installed app (not a download).
#[cfg(any(target_os = "macos", windows))]
pub fn running_installed_copy() -> bool {
    let current = std::env::current_exe().and_then(|exe| exe.canonicalize());
    current.is_ok() && current.ok() == installed_binary().canonicalize().ok()
}

/// Open a URL in the default browser, or a file or folder in its default app.
pub fn open_external(target: &str) {
    #[cfg(windows)]
    {
        let _ = crate::win::shell_open(target);
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(target).spawn();
    }
}

/// What a second start of the app asks the running tray app to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayRequest {
    OpenSettings,
    /// The app was updated on disk: start the new version (with Settings open).
    Restart,
}

fn request_file() -> PathBuf {
    home_dir().join("tray-request")
}

/// Leave a request for the running tray app, which looks every second.
pub fn request_tray(request: TrayRequest) -> Result<()> {
    create_private_dir(&home_dir())?;
    let text = match request {
        TrayRequest::OpenSettings => "settings",
        TrayRequest::Restart => "restart",
    };
    fs::write(request_file(), text)?;
    Ok(())
}

/// Leave a request and wait for the running tray app to pick it up. False
/// when nothing did (the lock is held by `t3wow run`, not the tray app).
pub fn hand_over_to_tray(request: TrayRequest) -> Result<bool> {
    request_tray(request)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if !request_file().exists() {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = fs::remove_file(request_file());
    Ok(false)
}

/// Take the pending request, if any.
pub fn take_tray_request() -> Option<TrayRequest> {
    let text = fs::read_to_string(request_file()).ok()?;
    let _ = fs::remove_file(request_file());
    match text.trim() {
        "settings" => Some(TrayRequest::OpenSettings),
        "restart" => Some(TrayRequest::Restart),
        _ => None,
    }
}

pub fn uninstall() -> Result<bool> {
    let path = app_path();
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        fs::remove_dir_all(&path)?
    } else {
        fs::remove_file(&path)?
    }
    Ok(true)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn describes_a_menu_bar_app() {
        let plist = info_plist();
        assert!(plist.contains("<key>LSUIElement</key>\n  <true/>"), "no Dock icon");
        assert!(plist.contains("<string>t3wow</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key>\n  <string>AppIcon</string>"));
        // Without these macOS ends the app when dictation asks for access.
        assert!(plist.contains("<key>NSMicrophoneUsageDescription</key>"));
        assert!(plist.contains("<key>NSSpeechRecognitionUsageDescription</key>"));
    }

    #[test]
    fn embeds_the_app_icon() {
        assert_eq!(&ICNS[..4], b"icns");
    }
}
