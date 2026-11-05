//! Something to launch the tray app from: `~/Applications/T3 WoW.app` on
//! macOS (a menu bar app without a Dock icon), a Start Menu shortcut on
//! Windows. Both run their own copy of the binary, so the download or build
//! folder can go away.

use std::fs;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", windows))]
use std::process::Command;

#[cfg(target_os = "macos")]
use crate::config::BRIDGE_VERSION;
use crate::config::user_home;
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
    let current = std::env::current_exe()?;
    // Reinstalling from the bundle itself: copy before removing it.
    let binary = fs::read(&current)?;
    if app.exists() {
        fs::remove_dir_all(&app)?;
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
    let _ = Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app).output();
    Ok(app)
}

#[cfg(target_os = "macos")]
pub fn open() -> Result<()> {
    let status = Command::new("open").arg(app_path()).status()?;
    if !status.success() {
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
    Command::new(installed_binary()).arg("tray").spawn()?;
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
    }

    #[test]
    fn embeds_the_app_icon() {
        assert_eq!(&ICNS[..4], b"icns");
    }
}
