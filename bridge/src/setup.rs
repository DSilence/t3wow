//! Setup steps shared by the command line and the settings window: the
//! environment's MCP URL, the sign-in state, and installing the addon into
//! World of Warcraft folders.

use std::path::{Path, PathBuf};

use crate::config::BridgeConfig;
use crate::error::{Error, Result};
use crate::{oauth, url, wow};

/// The most live slots an install may have: slot folders are numbered with
/// three digits (`T3Wow_Live001`..`T3Wow_Live999`).
pub const MAX_LIVE_SLOTS: u32 = 999;

/// Accepts a bare host, a URL without `/mcp`, or the full MCP URL.
pub fn normalize_mcp_url(input: &str) -> String {
    let input = input.trim();
    let with_scheme = if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let (origin, rest) = url::split(&with_scheme).unwrap_or((&with_scheme, ""));
    let path = url::path(&with_scheme).trim_end_matches('/');
    let query = &rest[rest.find(['?', '#']).unwrap_or(rest.len())..];
    let path = if path.ends_with("/mcp") { path.to_string() } else { format!("{path}/mcp") };
    format!("{}{path}{query}", origin.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignIn {
    No,
    Expired,
    /// Days left, when the token says when it expires.
    Yes(Option<i64>),
}

pub fn sign_in_state(server_url: &str) -> SignIn {
    let auth = oauth::stored_auth(server_url).ok().flatten().unwrap_or_default();
    if !auth["tokens"]["access_token"].is_string() {
        return SignIn::No;
    }
    match auth["tokensExpireAt"].as_i64() {
        Some(at) if at <= oauth::now_ms() => SignIn::Expired,
        Some(at) => SignIn::Yes(Some((at - oauth::now_ms()) / 86_400_000)),
        None => SignIn::Yes(None),
    }
}

fn is_flavor_dir(dir: &Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()).is_some_and(wow::is_flavor_name)
}

/// The flavor folders (`_retail_`, `_classic_`, ...) in a World of Warcraft
/// folder, or the folder itself when it is one.
pub fn flavors_in(dir: &Path) -> Vec<PathBuf> {
    if is_flavor_dir(dir) { vec![dir.to_path_buf()] } else { wow::flavor_dirs_in(dir) }
}

/// Flavor folders of the World of Warcraft installs found in the usual places.
pub fn detected_flavors() -> Vec<PathBuf> {
    wow::detect_wow_roots().iter().flat_map(|root| flavors_in(root)).collect()
}

/// "Retail", "Classic Era", "Classic Beta", "PTR" for `_retail_`, ...
pub fn flavor_label(flavor_dir: &Path) -> String {
    let name = flavor_dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.trim_matches('_')
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| match word {
            "ptr" | "xptr" => word.to_ascii_uppercase(),
            _ => {
                let mut chars = word.chars();
                chars.next().map(|c| c.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn check_live_slots(slots: u32) -> Result<u32> {
    if slots > MAX_LIVE_SLOTS {
        return Err(Error::other(format!("Live updates must be 0-{MAX_LIVE_SLOTS}.")));
    }
    Ok(slots)
}

/// Install the addon into `flavor` with the configured slot count and
/// remember the folder.
pub fn install_into(config: &mut BridgeConfig, flavor: &Path, link: bool, copy_slots: bool) -> Result<()> {
    wow::install_addon(flavor, link, config.live_slots, copy_slots)?;
    if !config.wow_flavor_dirs.iter().any(|dir| dir == flavor) {
        config.wow_flavor_dirs.push(flavor.to_path_buf());
    }
    Ok(())
}

/// Remove the addon from `flavor` and forget the folder.
pub fn remove_from(config: &mut BridgeConfig, flavor: &Path) -> Result<()> {
    wow::uninstall_addon(flavor)?;
    config.wow_flavor_dirs.retain(|dir| dir != flavor);
    Ok(())
}

/// Install the addon again wherever it is installed, with the configured
/// slot count. Keeps copied slots copied, and a linked addon (`install
/// --link`) linked; that one is left alone by a binary built elsewhere, which
/// does not have the checkout. Returns the folders it updated.
pub fn reinstall_everywhere(config: &BridgeConfig) -> Result<Vec<PathBuf>> {
    let mut updated = Vec::new();
    for flavor in &config.wow_flavor_dirs {
        let linked = wow::addon_linked(flavor);
        if !wow::is_installed(flavor) || (linked && !wow::addon_source_dir().is_dir()) {
            continue;
        }
        let copy_slots = wow::installed_live_slots(flavor) > 0 && !wow::slots_linked(flavor);
        wow::install_addon(flavor, linked, config.live_slots, copy_slots)?;
        updated.push(flavor.clone());
    }
    Ok(updated)
}

/// Whether the bridge has what it needs to sync: a signed-in environment and
/// the addon installed somewhere.
pub fn is_complete(config: &BridgeConfig) -> bool {
    let signed_in = config.server_url.as_deref().is_some_and(|url| sign_in_state(url) != SignIn::No);
    signed_in && config.wow_flavor_dirs.iter().any(|dir| wow::is_installed(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mcp_urls() {
        assert_eq!(normalize_mcp_url("http://127.0.0.1:3773/mcp"), "http://127.0.0.1:3773/mcp");
        assert_eq!(normalize_mcp_url("box.tail1234.ts.net"), "https://box.tail1234.ts.net/mcp");
        assert_eq!(normalize_mcp_url("https://Host/"), "https://host/mcp");
        assert_eq!(normalize_mcp_url("https://host/env/mcp/"), "https://host/env/mcp");
    }

    #[test]
    fn labels_flavors() {
        assert_eq!(flavor_label(Path::new("/w/_retail_")), "Retail");
        assert_eq!(flavor_label(Path::new("/w/_classic_era_")), "Classic Era");
        assert_eq!(flavor_label(Path::new("/w/_classic_beta_")), "Classic Beta");
        assert_eq!(flavor_label(Path::new("/w/_xptr_")), "XPTR");
    }

    #[test]
    fn limits_live_slots() {
        assert_eq!(check_live_slots(300).unwrap(), 300);
        assert_eq!(check_live_slots(999).unwrap(), 999);
        assert!(check_live_slots(1000).is_err());
    }
}
