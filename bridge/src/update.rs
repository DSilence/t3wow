//! Updates from GitHub releases: find the latest release, download this
//! platform's binary, check it against the release's `SHA256SUMS`, and swap
//! it in for the running one. The new binary then reinstalls the addon (it is
//! built into the binary) and the app (`t3wow post-update`).
//!
//! The tray app does this on its own (see `tray.rs`); `t3wow update` does it
//! from a terminal.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{BRIDGE_VERSION, create_private_dir, home_dir, load_config};
use crate::error::{Error, Result};
use crate::{app, setup};

/// Where releases are published, from `repository` in Cargo.toml.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const SUMS_ASSET: &str = "SHA256SUMS";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Without the leading "v".
    pub version: String,
    pub binary_url: String,
    pub sums_url: String,
}

/// The GitHub API URL for the latest release. `T3WOW_UPDATE_URL` overrides it
/// (for forks and tests).
pub fn latest_release_url() -> String {
    if let Some(url) = std::env::var_os("T3WOW_UPDATE_URL") {
        return url.to_string_lossy().into_owned();
    }
    let slug = REPOSITORY.trim_end_matches('/').trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{slug}/releases/latest")
}

/// The release asset built for this platform.
pub fn asset_name() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        format!("t3wow-{arch}-apple-darwin")
    } else if cfg!(windows) {
        format!("t3wow-{arch}-pc-windows-msvc.exe")
    } else {
        format!("t3wow-{arch}-unknown-linux-gnu")
    }
}

/// `1.2.3` or `v1.2.3`; pre-releases (`1.2.3-beta`) are not offered.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .user_agent(format!("t3wow/{BRIDGE_VERSION}"))
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = agent.get(url).header("Accept", "application/vnd.github+json").call()?;
    if !response.status().is_success() {
        return Err(Error::other(format!("{url} answered {}", response.status())));
    }
    Ok(response.body_mut().with_config().limit(limit).read_to_vec()?)
}

/// The latest release, when it is newer than `current`.
pub fn check(api_url: &str, current: &str) -> Result<Option<Release>> {
    let agent = agent(Duration::from_secs(30));
    let mut response = agent.get(api_url).header("Accept", "application/vnd.github+json").call()?;
    // No release published yet.
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(Error::other(format!("GitHub answered {}", response.status())));
    }
    let json: Value = serde_json::from_slice(&response.body_mut().with_config().limit(4 * 1024 * 1024).read_to_vec()?)?;
    parse_release(&json, current)
}

pub fn parse_release(json: &Value, current: &str) -> Result<Option<Release>> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = json["tag_name"].as_str().ok_or_else(|| Error::other("release without a tag"))?;
    if parse_version(tag).is_none() || !is_newer(tag, current) {
        return Ok(None);
    }
    let asset_url = |name: &str| {
        json["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|asset| asset["name"].as_str() == Some(name))
            .and_then(|asset| asset["browser_download_url"].as_str().map(String::from))
    };
    let asset = asset_name();
    let binary_url = asset_url(&asset)
        .ok_or_else(|| Error::other(format!("Release {tag} has no build for this computer ({asset})")))?;
    let sums_url = asset_url(SUMS_ASSET).ok_or_else(|| Error::other(format!("Release {tag} has no {SUMS_ASSET}")))?;
    Ok(Some(Release { version: tag.trim_start_matches('v').to_string(), binary_url, sums_url }))
}

/// The checksum listed for `name` in a `sha256sum` style file.
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn download_dir() -> PathBuf {
    home_dir().join("update")
}

/// Download the release's binary into `dir` and check its checksum.
pub fn download(release: &Release, dir: &Path) -> Result<PathBuf> {
    let agent = agent(Duration::from_secs(15 * 60));
    let sums = String::from_utf8_lossy(&get(&agent, &release.sums_url, 1024 * 1024)?).into_owned();
    let asset = asset_name();
    let expected =
        expected_sha256(&sums, &asset).ok_or_else(|| Error::other(format!("{SUMS_ASSET} does not list {asset}")))?;
    let bytes = get(&agent, &release.binary_url, 256 * 1024 * 1024)?;
    if hex(&Sha256::digest(&bytes)) != expected {
        return Err(Error::other(format!("The download of {} does not match its checksum", release.version)));
    }
    create_private_dir(dir)?;
    let file = dir.join(&asset);
    let _ = fs::remove_file(&file);
    fs::write(&file, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))?;
    }
    Ok(file)
}

/// Replace `target` with a copy of `source`. The running binary can be
/// replaced too: unix keeps running the old file, and Windows lets a running
/// executable be renamed (to `.old`, removed on the next start) though not
/// overwritten.
pub fn replace_file(target: &Path, source: &Path) -> Result<()> {
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("t3wow");
    let staged = target.with_file_name(format!(".{name}.new"));
    fs::copy(source, &staged)?;
    if cfg!(windows) && target.exists() {
        let old = target.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(target, &old)?;
    }
    fs::rename(&staged, target)?;
    Ok(())
}

/// Remove what an update on Windows left behind.
pub fn clean_up() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = fs::remove_file(exe.with_extension("old"));
    }
}

fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        // No console window flashing up from the tray app.
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Swap `new_binary` in for the running binary, then let it reinstall the
/// addon and the app. Call it while World of Warcraft is closed. Returns
/// what the reinstall printed.
pub fn install(new_binary: &Path) -> Result<String> {
    // Make sure it runs on this computer before replacing anything.
    let probe = command(new_binary).arg("version").output()?;
    if !probe.status.success() {
        return Err(Error::other("The downloaded bridge does not start on this computer"));
    }
    let current = std::env::current_exe()?;
    replace_file(&current, new_binary)?;
    let output = command(&current).arg("post-update").output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::other(format!("Reinstalling after the update failed: {detail}")));
    }
    let _ = fs::remove_file(new_binary);
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run by the new binary after an update: reinstall the addon where it is
/// installed (it ships inside the binary) and the app, if it is installed.
pub fn post_update() -> Result<()> {
    let config = load_config()?;
    for flavor in setup::reinstall_everywhere(&config)? {
        println!("Updated the addon in {}", flavor.display());
    }
    if cfg!(any(target_os = "macos", windows)) && app::installed() {
        let path = app::install()?;
        println!("Updated {}", path.display());
    }
    Ok(())
}

/// Start the updated tray app with extra arguments. `exe` is the binary the
/// old one ran from.
pub fn relaunch(exe: &Path, args: &[&str]) -> Result<()> {
    if cfg!(target_os = "macos") && app::launched_from_bundle() {
        // A new instance: the old one is still quitting.
        let mut open = Command::new("open");
        open.arg("-n").arg(app::app_path());
        if !args.is_empty() {
            open.arg("--args").args(args);
        }
        open.spawn()?;
    } else {
        command(exe).arg("tray").args(args).spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), None);
        assert_eq!(parse_version("1.2"), None);
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn points_at_the_repository() {
        assert!(
            latest_release_url().starts_with("https://api.github.com/repos/")
                || std::env::var_os("T3WOW_UPDATE_URL").is_some()
        );
        assert!(latest_release_url().ends_with("/releases/latest"));
    }

    fn release(tag: &str, assets: &[&str]) -> Value {
        let assets: Vec<Value> = assets
            .iter()
            .map(|name| json!({ "name": name, "browser_download_url": format!("https://example.com/{name}") }))
            .collect();
        json!({ "tag_name": tag, "draft": false, "prerelease": false, "assets": assets })
    }

    #[test]
    fn picks_this_platforms_asset() {
        let asset = asset_name();
        let found = parse_release(&release("v0.2.0", &[&asset, "SHA256SUMS", "other"]), "0.1.0").unwrap().unwrap();
        assert_eq!(found.version, "0.2.0");
        assert_eq!(found.binary_url, format!("https://example.com/{asset}"));
        assert_eq!(found.sums_url, "https://example.com/SHA256SUMS");
        assert_eq!(parse_release(&release("v0.1.0", &[&asset, "SHA256SUMS"]), "0.1.0").unwrap(), None);
        assert!(parse_release(&release("v0.2.0", &["SHA256SUMS"]), "0.1.0").is_err());
        let mut pre = release("v0.2.0", &[&asset, "SHA256SUMS"]);
        pre["prerelease"] = json!(true);
        assert_eq!(parse_release(&pre, "0.1.0").unwrap(), None);
    }

    #[test]
    fn reads_checksum_files() {
        let sums = "ABC123  t3wow-aarch64-apple-darwin\nfff *t3wow-x86_64-pc-windows-msvc.exe\n";
        assert_eq!(expected_sha256(sums, "t3wow-aarch64-apple-darwin").as_deref(), Some("abc123"));
        assert_eq!(expected_sha256(sums, "t3wow-x86_64-pc-windows-msvc.exe").as_deref(), Some("fff"));
        assert_eq!(expected_sha256(sums, "t3wow-x86_64-unknown-linux-gnu"), None);
    }
}
