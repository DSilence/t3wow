use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeConfig {
    /// The environment's MCP URL, e.g. https://<environment-address>/mcp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// WoW flavor folders (`_retail_`, `_classic_`, ...) the addon is installed in.
    pub wow_flavor_dirs: Vec<PathBuf>,
    /// Number of load-on-demand live slot addons generated per flavor.
    pub live_slots: u32,
    /// Loopback port for the OAuth redirect. Fixed so the registered client stays valid.
    pub oauth_port: u16,
    /// Poll interval while any visible thread is running.
    pub active_poll_ms: u64,
    /// Poll interval when everything is idle.
    pub idle_poll_ms: u64,
    /// Projects included in each snapshot.
    pub max_projects: usize,
    /// Threads listed per project.
    pub threads_per_project: usize,
    /// Threads whose messages are included in the snapshot (besides ones the addon watches).
    pub recent_threads_with_messages: usize,
    /// Messages kept per thread in the snapshot.
    pub messages_per_thread: usize,
    /// Characters kept per message in the snapshot.
    pub max_chars_per_message: usize,
    /// The tray app installs new releases on its own (while the game is closed).
    pub auto_update: bool,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            server_url: None,
            wow_flavor_dirs: Vec::new(),
            // Slots share one file elsewhere, so many cost nothing; Windows copies each.
            live_slots: if cfg!(windows) { 50 } else { 300 },
            oauth_port: 47823,
            active_poll_ms: 2_000,
            idle_poll_ms: 8_000,
            max_projects: 12,
            threads_per_project: 20,
            recent_threads_with_messages: 4,
            messages_per_thread: 30,
            max_chars_per_message: 6_000,
            auto_update: true,
        }
    }
}

pub fn home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("T3WOW_HOME") {
        return PathBuf::from(dir);
    }
    user_home().join(".t3wow")
}

pub fn user_home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    home_dir().join("config.json")
}

pub fn load_config() -> io::Result<BridgeConfig> {
    Ok(read_json_file(&config_path())?.unwrap_or_default())
}

pub fn save_config(config: &BridgeConfig) -> io::Result<()> {
    write_json_file(&config_path(), config, false)
}

/// Write JSON atomically; `secret` files are readable by the owner only.
pub fn write_json_file<T: Serialize + ?Sized>(file: &Path, value: &T, secret: bool) -> io::Result<()> {
    let dir = file.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    set_mode(&tmp, if secret { 0o600 } else { 0o644 })?;
    fs::rename(&tmp, file)
}

pub fn read_json_file<T: DeserializeOwned>(file: &Path) -> io::Result<Option<T>> {
    match fs::read(file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map(Some).map_err(|e| io::Error::other(format!("{}: {e}", file.display())))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}
