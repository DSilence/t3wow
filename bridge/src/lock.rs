//! One bridge at a time: two would both deliver new-thread commands, and
//! those have no retry key. The lock is held for the life of the process and
//! released by the OS when it exits, even after a crash.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::config::{create_private_dir, home_dir};
use crate::error::{Error, Result};

pub struct BridgeLock {
    _file: File,
}

pub fn lock_path() -> PathBuf {
    home_dir().join("bridge.lock")
}

impl BridgeLock {
    pub fn acquire() -> Result<Self> {
        Self::acquire_at(&lock_path())
    }

    pub fn acquire_at(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let file = open_exclusive(path)
            .map_err(|_| Error::other("Another t3wow bridge is already running (the tray app or `t3wow run`)."))?;
        Ok(Self { _file: file })
    }

    /// Whether some process holds the lock right now.
    pub fn held() -> bool {
        Self::acquire().is_err()
    }
}

#[cfg(unix)]
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    use std::os::unix::io::AsRawFd;
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // No sharing: a second open fails while this handle lives.
    OpenOptions::new().create(true).truncate(false).write(true).share_mode(0).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_one_holder_at_a_time() {
        let dir = std::env::temp_dir().join(format!("t3wow-lock-{}", std::process::id()));
        let path = dir.join("bridge.lock");
        let first = BridgeLock::acquire_at(&path).unwrap();
        assert!(BridgeLock::acquire_at(&path).is_err());
        drop(first);
        assert!(BridgeLock::acquire_at(&path).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
