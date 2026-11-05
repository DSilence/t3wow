//! Windows shell calls made directly, not through PowerShell or rundll32:
//! antivirus heuristics treat an unsigned binary spawning those as a dropper.

use std::ffi::OsStr;
use std::path::Path;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, SW_SHOWNORMAL};
use windows::core::{HSTRING, Interface, w};

use crate::error::{Error, Result};

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Error::Other(error.message())
    }
}

/// COM on this thread for as long as it lives.
struct Com {
    initialized: bool,
}

impl Com {
    fn init() -> Self {
        // Fails with RPC_E_CHANGED_MODE when the thread already has COM in
        // another mode, which works for these calls too.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Com { initialized: result.is_ok() }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

pub struct Shortcut<'a> {
    pub target: &'a Path,
    pub arguments: &'a str,
    pub description: &'a str,
    pub icon: &'a Path,
}

/// Write a `.lnk` file at `link`.
pub fn create_shortcut(link: &Path, shortcut: &Shortcut) -> Result<()> {
    let _com = Com::init();
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell_link.SetPath(&HSTRING::from(shortcut.target.as_os_str()))?;
        shell_link.SetArguments(&HSTRING::from(shortcut.arguments))?;
        shell_link.SetDescription(&HSTRING::from(shortcut.description))?;
        shell_link.SetIconLocation(&HSTRING::from(shortcut.icon.as_os_str()), 0)?;
        // The tray app drops its console right away; minimized, it does not flash up.
        shell_link.SetShowCmd(SW_SHOWMINNOACTIVE)?;
        if let Some(dir) = shortcut.target.parent() {
            shell_link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        let file: IPersistFile = shell_link.cast()?;
        file.Save(&HSTRING::from(link.as_os_str()), true)?;
    }
    Ok(())
}

/// Open a URL in the default browser, or a file in its default app.
pub fn shell_open(target: impl AsRef<OsStr>) -> Result<()> {
    let _com = Com::init();
    let target = HSTRING::from(target.as_ref());
    let instance = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    // Anything above 32 is success.
    if instance.0 as isize <= 32 {
        return Err(Error::other(format!("Could not open {target}")));
    }
    Ok(())
}

/// Let go of the console: started from the Start Menu or Explorer, its
/// window would stay open.
pub fn detach_console() {
    let _ = unsafe { FreeConsole() };
}

/// Whether this process has its console to itself: started by double-click
/// in Explorer rather than from a terminal, which shares its console.
pub fn launched_from_explorer() -> bool {
    let mut processes = [0u32; 4];
    let count = unsafe { GetConsoleProcessList(&mut processes) };
    count == 1
}
