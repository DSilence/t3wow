//! Finding World of Warcraft, installing the addon, and the files the bridge
//! exchanges with the game.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::user_home;
use crate::error::{Error, Result};

pub const ADDON_NAME: &str = "T3Wow";
pub const INBOX_ADDON: &str = "T3Wow_Inbox";
pub const SAVED_VARIABLES_FILE: &str = "T3Wow.lua";
/// The snapshot every live slot loads. Kept in the inbox folder but not listed
/// in its TOC; slots link to it so one write updates them all.
const SHARED_LIVE_FILE: &str = "Live.lua";

/// The addon ships inside the binary, so installing needs nothing else.
pub const ADDON_FILES: &[(&str, &[u8])] = &[
    ("T3Wow.toc", include_bytes!("../../addon/T3Wow/T3Wow.toc")),
    ("Bindings.xml", include_bytes!("../../addon/T3Wow/Bindings.xml")),
    ("Util.lua", include_bytes!("../../addon/T3Wow/Util.lua")),
    ("Markdown.lua", include_bytes!("../../addon/T3Wow/Markdown.lua")),
    ("Shot.lua", include_bytes!("../../addon/T3Wow/Shot.lua")),
    ("Core.lua", include_bytes!("../../addon/T3Wow/Core.lua")),
    ("MessageView.lua", include_bytes!("../../addon/T3Wow/MessageView.lua")),
    ("UI.lua", include_bytes!("../../addon/T3Wow/UI.lua")),
    ("Notify.lua", include_bytes!("../../addon/T3Wow/Notify.lua")),
    ("FastSend.lua", include_bytes!("../../addon/T3Wow/FastSend.lua")),
    ("Dictation.lua", include_bytes!("../../addon/T3Wow/Dictation.lua")),
    ("Fonts/JetBrainsMono-Regular.ttf", include_bytes!("../../addon/T3Wow/Fonts/JetBrainsMono-Regular.ttf")),
    ("Fonts/OFL.txt", include_bytes!("../../addon/T3Wow/Fonts/OFL.txt")),
];

/// The addon in this checkout, for `install --link` during development.
pub fn addon_source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("addon").join(ADDON_NAME)
}

pub fn live_slot_name(index: u32) -> String {
    format!("T3Wow_Live{index:03}")
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs
}

/// `_retail_`, `_classic_`, `_classic_era_`, `_classic_beta_`, `_ptr_`, ...
pub fn is_flavor_name(name: &str) -> bool {
    name.len() > 2
        && name.starts_with('_')
        && name.ends_with('_')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

pub fn flavor_dirs_in(wow_root: &Path) -> Vec<PathBuf> {
    child_dirs(wow_root)
        .into_iter()
        .filter(|dir| dir.file_name().and_then(|n| n.to_str()).is_some_and(is_flavor_name))
        .collect()
}

/// Best-effort search for World of Warcraft installs in the usual places.
/// `WOW_DIR` names the install instead (and keeps tests off the real one).
pub fn detect_wow_roots() -> Vec<PathBuf> {
    let name = "World of Warcraft";
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("WOW_DIR") {
        candidates.push(dir.into());
    } else if cfg!(target_os = "macos") {
        candidates.push(Path::new("/Applications").join(name));
        candidates.push(user_home().join("Applications").join(name));
        for volume in child_dirs(Path::new("/Volumes")) {
            candidates.push(volume.join(name));
            for child in child_dirs(&volume) {
                candidates.push(child.join(name));
                for grandchild in child_dirs(&child) {
                    candidates.push(grandchild.join(name));
                }
            }
        }
    } else if cfg!(windows) {
        for drive in "CDEFGH".chars() {
            let root = PathBuf::from(format!("{drive}:\\"));
            for sub in ["Program Files (x86)", "Program Files", "", "Games", "Battle.net"] {
                candidates.push(root.join(sub).join(name));
            }
        }
    } else {
        candidates.push(user_home().join("Games/world-of-warcraft/drive_c/Program Files (x86)").join(name));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in candidates {
        if dir.is_dir() && !flavor_dirs_in(&dir).is_empty() && !found.contains(&dir) {
            found.push(dir);
        }
    }
    found
}

pub fn addons_dir(flavor_dir: &Path) -> PathBuf {
    flavor_dir.join("Interface").join("AddOns")
}

fn toc_interface() -> Result<String> {
    let toc = std::str::from_utf8(ADDON_FILES[0].1).unwrap_or_default();
    toc.lines()
        .find_map(|line| line.strip_prefix("## Interface:"))
        .map(|v| v.trim().to_string())
        .ok_or_else(|| Error::other("T3Wow.toc has no ## Interface line"))
}

/// Write a file so the game never reads a half-written one.
pub fn write_file_atomic(file: &Path, contents: &str) -> io::Result<()> {
    let tmp = file.with_extension("lua.tmp");
    fs::write(&tmp, contents)?;
    if let Err(error) = fs::rename(&tmp, file) {
        // Windows refuses to replace a file another process has open; fall back.
        fs::write(file, contents)?;
        let _ = fs::remove_file(&tmp);
        if error.kind() != io::ErrorKind::PermissionDenied {
            return Err(error);
        }
    }
    Ok(())
}

fn generated_toc(title: &str, interface: &str, load_on_demand: bool) -> String {
    let mut lines = vec![
        format!("## Interface: {interface}"),
        format!("## Title: |cff8080a0{title}|r"),
        "## Notes: Generated by the T3 WoW bridge. Do not edit.".into(),
        "## Author: t3wow".into(),
        format!("## Dependencies: {ADDON_NAME}"),
        format!("## Group: {ADDON_NAME}"),
    ];
    if load_on_demand {
        lines.push("## LoadOnDemand: 1".into());
    }
    lines.extend(["".into(), "Data.lua".into(), "".into()]);
    lines.join("\n")
}

const EMPTY_DATA: &str = "-- Written by the T3 WoW bridge.\n";

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path).or_else(|_| fs::remove_dir(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn symlink_dir(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn symlink_dir(source: &Path, target: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
}

#[cfg(unix)]
fn symlink_file(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

/// Windows only allows symlinks with developer mode or admin rights.
#[cfg(windows)]
fn symlink_file(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no file symlinks on Windows"))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Install the addon and generate the bridge-owned data addons:
/// T3Wow_Inbox (read at every UI load) and T3Wow_LiveNNN (load-on-demand,
/// each loadable once per UI session, which is how updates arrive without /reload).
/// Slot Data.lua files are symlinks to one shared file unless `copy_slots` is set
/// or the system cannot link.
pub fn install_addon(flavor_dir: &Path, link: bool, live_slots: u32, copy_slots: bool) -> Result<()> {
    let addons = addons_dir(flavor_dir);
    fs::create_dir_all(&addons)?;
    let interface = toc_interface()?;

    let target = addons.join(ADDON_NAME);
    remove_path(&target)?;
    if link {
        let source = addon_source_dir().canonicalize().map_err(|_| {
            Error::other("--link needs the addon sources next to the bridge (run it from the t3wow checkout)")
        })?;
        symlink_dir(&source, &target)?;
    } else {
        fs::create_dir_all(&target)?;
        for (name, bytes) in ADDON_FILES {
            let file = target.join(name);
            fs::create_dir_all(file.parent().unwrap())?;
            fs::write(file, bytes)?;
        }
    }

    let make_data_addon = |name: &str, title: &str, load_on_demand: bool| -> io::Result<()> {
        let dir = addons.join(name);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(format!("{name}.toc")), generated_toc(title, &interface, load_on_demand))?;
        let data = dir.join("Data.lua");
        if !data.exists() {
            fs::write(data, EMPTY_DATA)?;
        }
        Ok(())
    };
    make_data_addon(INBOX_ADDON, "T3Wow inbox", false)?;
    let shared = addons.join(INBOX_ADDON).join(SHARED_LIVE_FILE);
    if !shared.exists() {
        fs::write(&shared, EMPTY_DATA)?;
    }
    let shared_relative = Path::new("..").join(INBOX_ADDON).join(SHARED_LIVE_FILE);
    let mut linked = !copy_slots;
    for i in 1..=live_slots {
        let name = live_slot_name(i);
        let data = addons.join(&name).join("Data.lua");
        if !linked && is_symlink(&data) {
            fs::remove_file(&data)?;
        }
        make_data_addon(&name, &format!("T3Wow live {i:03}"), true)?;
        if linked && !is_symlink(&data) {
            fs::remove_file(&data)?;
            if symlink_file(&shared_relative, &data).is_err() {
                fs::write(&data, EMPTY_DATA)?;
                linked = false;
            }
        }
    }
    // Drop slots from an earlier install with a larger slot count.
    for dir in child_dirs(&addons) {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if let Some(n) = name.strip_prefix("T3Wow_Live").and_then(|n| n.parse::<u32>().ok())
            && n > live_slots
        {
            remove_path(&dir)?;
        }
    }
    Ok(())
}

fn is_bridge_addon(name: &str) -> bool {
    name == ADDON_NAME
        || name == INBOX_ADDON
        || name.strip_prefix("T3Wow_Live").is_some_and(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn uninstall_addon(flavor_dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(addons_dir(flavor_dir)) else { return Ok(()) };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(is_bridge_addon) {
            remove_path(&entry.path())?;
        }
    }
    Ok(())
}

pub fn is_installed(flavor_dir: &Path) -> bool {
    addons_dir(flavor_dir).join(INBOX_ADDON).join(format!("{INBOX_ADDON}.toc")).exists()
}

pub fn installed_live_slots(flavor_dir: &Path) -> u32 {
    let addons = addons_dir(flavor_dir);
    let mut count = 0;
    while addons.join(live_slot_name(count + 1)).exists() {
        count += 1;
    }
    count
}

/// Every account's copy of the addon's SavedVariables in this flavor.
pub fn saved_variables_files(flavor_dir: &Path) -> Vec<PathBuf> {
    child_dirs(&flavor_dir.join("WTF").join("Account"))
        .into_iter()
        .map(|account| account.join("SavedVariables").join(SAVED_VARIABLES_FILE))
        .filter(|file| file.exists())
        .collect()
}

/// The Data.lua a bridge-owned addon runs: hands the snapshot to the main addon.
pub fn data_chunk(lua_table: &str, source: &str) -> String {
    format!(
        "-- Written by the T3 WoW bridge. Do not edit.\nlocal snapshot = {lua_table}\n\
         if T3Wow_Receive then T3Wow_Receive(snapshot, \"{source}\") end\n"
    )
}

/// Whether the addon folder links to a checkout (`install --link`).
pub fn addon_linked(flavor_dir: &Path) -> bool {
    is_symlink(&addons_dir(flavor_dir).join(ADDON_NAME))
}

/// Whether this flavor's live slots link to the shared file (one write per update).
pub fn slots_linked(flavor_dir: &Path) -> bool {
    is_symlink(&addons_dir(flavor_dir).join(live_slot_name(1)).join("Data.lua"))
}

/// Publish a snapshot to the inbox and every live slot of a flavor.
pub fn publish_snapshot(flavor_dir: &Path, lua_table: &str, live_slots: u32) -> io::Result<()> {
    let addons = addons_dir(flavor_dir);
    let inbox = addons.join(INBOX_ADDON);
    write_file_atomic(&inbox.join("Data.lua"), &data_chunk(lua_table, "inbox"))?;
    let live = data_chunk(lua_table, "live");
    write_file_atomic(&inbox.join(SHARED_LIVE_FILE), &live)?;
    for i in 1..=live_slots {
        let data = addons.join(live_slot_name(i)).join("Data.lua");
        if !is_symlink(&data) {
            write_file_atomic(&data, &live)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_every_addon_file() {
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
                if entry.path().is_dir() {
                    walk(&entry.path(), &format!("{name}/"), out);
                } else {
                    out.push(name);
                }
            }
        }
        let mut on_disk = Vec::new();
        walk(&addon_source_dir(), "", &mut on_disk);
        let mut embedded: Vec<String> = ADDON_FILES.iter().map(|(name, _)| name.to_string()).collect();
        on_disk.sort();
        embedded.sort();
        assert_eq!(embedded, on_disk);
        assert!(toc_interface().unwrap().starts_with("16001"));
    }

    #[test]
    fn recognizes_names() {
        assert!(is_flavor_name("_classic_beta_"));
        assert!(!is_flavor_name("_"));
        assert!(!is_flavor_name("Data"));
        assert!(is_bridge_addon("T3Wow_Live007"));
        assert!(!is_bridge_addon("T3Wow_Live7"));
        assert!(!is_bridge_addon("T3WowExtras"));
    }
}
