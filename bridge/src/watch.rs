//! Is World of Warcraft running? Used by `run --wait-for-wow` so the bridge
//! only talks to T3 while the game is open. A process scan costs well under a
//! millisecond, so polling every few seconds is effectively free.

/// Retail, Classic, PTR and beta clients: `World of Warcraft` (macOS app
/// binaries), `Wow.exe`, `WowClassic.exe`, `WowClassicB.exe`, `WowT.exe`, ...
/// Not the helpers next to them: `WowVoiceProxy.exe` often outlives the game.
pub fn is_wow_process_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ["launcher", "error", "proxy", "crash"].iter().any(|helper| lower.contains(helper)) {
        return false;
    }
    lower.starts_with("world of warcraft") || (lower.starts_with("wow") && lower.ends_with(".exe"))
}

pub fn wow_running() -> bool {
    wow_process().is_some()
}

/// The name of a running game client, if any.
pub fn wow_process() -> Option<String> {
    process_names().into_iter().find(|name| is_wow_process_name(name))
}

#[cfg(target_os = "macos")]
fn process_names() -> Vec<String> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Room for processes started between the two calls.
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let found = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(found.max(0) as usize);
    let mut names = Vec::with_capacity(pids.len());
    let mut name = [0u8; 64];
    let mut path = vec![0u8; 4096];
    for pid in pids {
        let len = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
        if len <= 0 {
            continue;
        }
        let short = String::from_utf8_lossy(&name[..len as usize]).into_owned();
        // Names are cut to 16 characters ("World of Warcraf"); the executable
        // path tells the game from "World of Warcraft Launcher".
        if short.starts_with("World of Warcraf") {
            let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            if len > 0 {
                let full = String::from_utf8_lossy(&path[..len as usize]).into_owned();
                names.push(full.rsplit('/').next().unwrap_or_default().to_string());
                continue;
            }
        }
        names.push(short);
    }
    names
}

/// Wine and Proton clients show up with their Windows executable name.
#[cfg(all(unix, not(target_os = "macos")))]
fn process_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())))
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|name| name.trim_end().to_string())
        .collect()
}

/// Read the process list directly: polling `tasklist` from a hidden window
/// is what antivirus heuristics expect from malware doing reconnaissance.
#[cfg(windows)]
fn process_names() -> Vec<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };

    let mut names = Vec::new();
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return names;
    };
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_game_clients_only() {
        for name in ["World of Warcraft", "Wow.exe", "WowClassic.exe", "WowClassicB.exe", "WowT.exe"] {
            assert!(is_wow_process_name(name), "{name}");
        }
        for name in [
            "Battle.net",
            "Wowup",
            "World of Warcraft Launcher",
            "WowLauncher.exe",
            "WowError.exe",
            "t3wow",
            "WowVoiceProxy.exe",
            "WowVoiceProxy",
        ] {
            assert!(!is_wow_process_name(name), "{name}");
        }
    }

    #[test]
    fn lists_processes() {
        assert!(!process_names().is_empty());
    }
}
