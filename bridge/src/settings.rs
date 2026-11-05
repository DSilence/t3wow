//! The settings window the tray app opens: a small page in a native web view
//! (WKWebView on macOS, WebView2 on Windows). The page sends commands as IPC
//! messages; slow work (signing in, installing the addon, looking for World
//! of Warcraft) runs on threads that update `Jobs` and wake the event loop,
//! which then sends the page the new state.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebView, WebViewBuilder};

use crate::config::{BRIDGE_VERSION, BridgeConfig, load_config, save_config};
use crate::dictation::{self, Access};
use crate::error::{Error, Result};
use crate::setup::{self, SignIn};
use crate::t3::T3Client;
use crate::{app, log, oauth, url, watch, wow};

const PAGE: &str = include_str!("../assets/settings.html");
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the tray shows, for the page's header and update row.
pub struct TrayInfo {
    pub status: String,
    pub attention: bool,
    pub update_text: String,
    pub update_enabled: bool,
}

/// Something the page asked for that the tray does.
pub enum Action {
    CheckForUpdates,
    OpenLog,
}

struct SignInJob {
    url: String,
    /// Known once the environment answered; the page can open it again.
    authorization_url: Option<String>,
    cancel: Arc<AtomicBool>,
}

/// Work in progress and its outcome, shared with the worker threads.
#[derive(Default)]
struct Jobs {
    sign_in: Option<SignInJob>,
    sign_in_error: Option<String>,
    sign_in_note: Option<String>,
    /// What the addon install is doing right now.
    wow_busy: Option<String>,
    wow_error: Option<String>,
    wow_note: Option<String>,
    /// Flavor folders found by looking in the usual places; None until looked.
    detected: Option<Vec<PathBuf>>,
}

struct Open {
    window: Window,
    webview: WebView,
    /// The page has loaded and can take state.
    ready: bool,
    shown: String,
    #[cfg(windows)]
    _context: wry::WebContext,
}

pub struct Settings {
    open: Option<Open>,
    jobs: Arc<Mutex<Jobs>>,
    /// Wakes the event loop so it sends the page the new state.
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Hands a page message to the event loop.
    ipc: Arc<dyn Fn(String) + Send + Sync>,
}

impl Settings {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, ipc: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        Settings { open: None, jobs: Arc::default(), wake, ipc }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.open.as_ref().map(|open| open.window.id())
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether an addon change is under way (the page disables its controls).
    pub fn addon_busy(&self) -> bool {
        self.jobs.lock().unwrap().wow_busy.is_some()
    }

    /// Show the window, or bring it to the front.
    pub fn show<T>(&mut self, target: &EventLoopWindowTarget<T>) {
        if let Some(open) = &self.open {
            open.window.set_minimized(false);
            open.window.set_focus();
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
            // A Dock icon and a menu bar (for copy and paste) while the window is open.
            target.set_activation_policy_at_runtime(ActivationPolicy::Regular);
            mac_menu().init_for_nsapp();
        }
        log!("opening settings");
        match self.build(target) {
            Ok(open) => {
                open.window.set_focus();
                self.open = Some(open);
            }
            Err(error) => {
                log!("could not open settings: {error}");
                let mut message = format!("Could not open the settings window: {error}");
                if cfg!(windows) {
                    message.push_str(
                        "\n\nIt needs the Microsoft Edge WebView2 Runtime, which comes with Windows 11 \
                         and current Windows 10 updates.",
                    );
                }
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("T3 WoW")
                    .set_description(message)
                    .show();
                self.after_close(target);
                return;
            }
        }
        if self.jobs.lock().unwrap().detected.is_none() {
            self.look_for_wow(false);
        }
    }

    fn build<T>(&self, target: &EventLoopWindowTarget<T>) -> Result<Open> {
        let size = LogicalSize::new(600.0, 720.0);
        let mut builder = WindowBuilder::new()
            .with_title("T3 WoW Settings")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(480.0, 480.0));
        // Centered on the main screen.
        if let Some(monitor) = target.primary_monitor() {
            let scale = monitor.scale_factor();
            let screen = monitor.size();
            let origin = monitor.position();
            let (width, height) = (size.width * scale, size.height * scale);
            let x = origin.x as f64 + (screen.width as f64 - width).max(0.0) / 2.0;
            let y = origin.y as f64 + (screen.height as f64 - height).max(0.0) / 3.0;
            builder = builder.with_position(PhysicalPosition::new(x, y));
        }
        let window = builder.build(target).map_err(|e| Error::other(e.to_string()))?;
        #[cfg(windows)]
        let mut context = wry::WebContext::new(Some(crate::config::home_dir().join("webview")));
        #[cfg(windows)]
        let builder = WebViewBuilder::new_with_web_context(&mut context);
        #[cfg(not(windows))]
        let builder = WebViewBuilder::new();
        let ipc = self.ipc.clone();
        let webview = builder
            .with_html(PAGE)
            .with_ipc_handler(move |request| ipc(request.body().clone()))
            // Links open in the browser (through IPC), never in this window.
            .with_navigation_handler(|url| url.starts_with("about:") || url.starts_with("data:"))
            .build(&window)
            .map_err(|e| Error::other(e.to_string()))?;
        Ok(Open {
            window,
            webview,
            ready: false,
            shown: String::new(),
            #[cfg(windows)]
            _context: context,
        })
    }

    pub fn close<T>(&mut self, target: &EventLoopWindowTarget<T>) {
        if self.open.take().is_some() {
            self.after_close(target);
        }
    }

    #[allow(unused_variables)]
    fn after_close<T>(&self, target: &EventLoopWindowTarget<T>) {
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
            target.set_activation_policy_at_runtime(ActivationPolicy::Accessory);
        }
    }

    /// Send the page the current state if it changed.
    pub fn refresh(&mut self, info: &TrayInfo) {
        let Some(open) = &mut self.open else { return };
        if !open.ready {
            return;
        }
        let config = load_config().unwrap_or_default();
        let state = state_json(&config, &self.jobs.lock().unwrap(), info, watch::wow_running()).to_string();
        if state != open.shown {
            if let Err(error) = open.webview.evaluate_script(&format!("window.render({state})")) {
                log!("settings page: {error}");
            }
            open.shown = state;
        }
    }

    /// A message from the page.
    pub fn handle(&mut self, message: &str, info: &TrayInfo) -> Option<Action> {
        let Ok(message) = serde_json::from_str::<Value>(message) else { return None };
        let text = |key: &str| message[key].as_str().unwrap_or_default().to_string();
        match message["cmd"].as_str().unwrap_or_default() {
            "ready" => {
                if let Some(open) = &mut self.open {
                    open.ready = true;
                    open.shown.clear();
                }
            }
            "signIn" => self.sign_in(&text("url")),
            "cancelSignIn" => {
                if let Some(job) = &self.jobs.lock().unwrap().sign_in {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            "openSignIn" => {
                if let Some(url) = self.jobs.lock().unwrap().sign_in.as_ref().and_then(|j| j.authorization_url.clone())
                {
                    app::open_external(&url);
                }
            }
            "signOut" => self.sign_out(),
            "lookForWow" => self.look_for_wow(false),
            "addWow" => self.add_wow_folder(),
            "setFlavor" => {
                let on = message["on"].as_bool().unwrap_or(false);
                self.set_flavor(PathBuf::from(text("path")), on);
            }
            "setSlots" => match message["slots"].as_u64().map(|n| setup::check_live_slots(n as u32)) {
                Some(Ok(slots)) => self.set_slots(slots),
                _ => {
                    let problem = format!("Enter a number from 0 to {}.", setup::MAX_LIVE_SLOTS);
                    self.jobs.lock().unwrap().wow_error = Some(problem);
                }
            },
            "setAutoUpdate" => {
                let on = message["on"].as_bool().unwrap_or(true);
                if let Err(error) = edit_config(|config| config.auto_update = on) {
                    log!("could not save settings: {error}");
                }
            }
            "setDictation" => {
                let on = message["on"].as_bool().unwrap_or(false);
                if let Err(error) = edit_config(|config| config.dictation.enabled = on) {
                    log!("could not save settings: {error}");
                }
            }
            "setDictationShortcut" => match dictation::shortcut_label(&text("shortcut")) {
                Some(label) => {
                    if let Err(error) = edit_config(|config| config.dictation.hotkey = label) {
                        log!("could not save settings: {error}");
                    }
                }
                None => log!("settings page sent an invalid shortcut {:?}", text("shortcut")),
            },
            "setDictationLanguage" => {
                let language = text("language").trim().to_string();
                let language = (!language.is_empty()).then_some(language);
                if let Err(error) = edit_config(|config| config.dictation.language = language) {
                    log!("could not save settings: {error}");
                }
            }
            "checkForUpdates" => return Some(Action::CheckForUpdates),
            "openLog" => return Some(Action::OpenLog),
            "openHelp" => app::open_external(&format!("{}#readme", env!("CARGO_PKG_REPOSITORY"))),
            "showFolder" => app::open_external(&text("path")),
            other => log!("settings page sent an unknown command {other:?}"),
        }
        self.refresh(info);
        None
    }

    fn spawn(&self, work: impl FnOnce(&Mutex<Jobs>) + Send + 'static) {
        let (jobs, wake) = (self.jobs.clone(), self.wake.clone());
        std::thread::spawn(move || {
            work(&jobs);
            wake();
        });
    }

    fn sign_in(&self, input: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.sign_in.is_some() {
            return;
        }
        if input.trim().is_empty() {
            jobs.sign_in_error = Some("Paste the environment's MCP URL first.".into());
            return;
        }
        let server_url = setup::normalize_mcp_url(input);
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.sign_in = Some(SignInJob { url: server_url.clone(), authorization_url: None, cancel: cancel.clone() });
        jobs.sign_in_error = None;
        jobs.sign_in_note = None;
        drop(jobs);
        let wake = self.wake.clone();
        self.spawn(move |jobs| {
            let result = sign_in(&server_url, &cancel, &|authorization_url| {
                if let Some(job) = &mut jobs.lock().unwrap().sign_in {
                    job.authorization_url = Some(authorization_url.to_string());
                }
                wake();
            });
            let mut jobs = jobs.lock().unwrap();
            jobs.sign_in = None;
            match result {
                Ok(note) => jobs.sign_in_note = Some(note),
                Err(_) if cancel.load(Ordering::Relaxed) => {}
                Err(error) => {
                    log!("sign-in failed: {error}");
                    jobs.sign_in_error = Some(error.to_string());
                }
            }
        });
    }

    fn sign_out(&self) {
        let result = load_config().map_err(Error::from).and_then(|config| match &config.server_url {
            Some(server_url) => oauth::forget_auth(server_url),
            None => Ok(()),
        });
        let mut jobs = self.jobs.lock().unwrap();
        jobs.sign_in_note = None;
        jobs.sign_in_error = result.err().map(|e| e.to_string());
    }

    /// Look for World of Warcraft in the usual places. With `install`, also
    /// install the addon into every flavor found (the first start after
    /// installing the app).
    pub fn look_for_wow(&self, install: bool) {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.wow_busy.is_some() {
            return;
        }
        jobs.wow_busy = Some("Looking for World of Warcraft…".into());
        drop(jobs);
        self.spawn(move |jobs| {
            let found = setup::detected_flavors();
            let mut installed = Vec::new();
            let mut error = None;
            if install && !found.is_empty() {
                jobs.lock().unwrap().wow_busy = Some("Installing the addon…".into());
                let result = edit_config(|config| {
                    for flavor in &found {
                        match setup::install_into(config, flavor, false, false) {
                            Ok(()) => installed.push(flavor.clone()),
                            Err(e) => error = Some(format!("{}: {e}", flavor.display())),
                        }
                    }
                });
                if let Err(e) = result {
                    error = Some(e.to_string());
                }
            }
            let mut jobs = jobs.lock().unwrap();
            jobs.wow_busy = None;
            if !installed.is_empty() {
                log!("installed the addon into {} folder(s)", installed.len());
                jobs.wow_note = Some(installed_note(installed.len()));
            }
            if let Some(error) = error {
                log!("installing the addon failed: {error}");
                jobs.wow_error = Some(error);
            }
            jobs.detected = Some(found);
        });
    }

    fn add_wow_folder(&self) {
        let mut dialog = rfd::FileDialog::new().set_title("Choose your World of Warcraft folder");
        if let Some(open) = &self.open {
            dialog = dialog.set_parent(&open.window);
        }
        let Some(dir) = dialog.pick_folder() else { return };
        let flavors = setup::flavors_in(&dir);
        if flavors.is_empty() {
            self.jobs.lock().unwrap().wow_error = Some(format!(
                "{} has no game folders (_retail_, _classic_, …). Choose the World of Warcraft folder itself.",
                dir.display()
            ));
            return;
        }
        {
            let mut jobs = self.jobs.lock().unwrap();
            let detected = jobs.detected.get_or_insert_with(Vec::new);
            for flavor in &flavors {
                if !detected.contains(flavor) {
                    detected.push(flavor.clone());
                }
            }
        }
        self.change_addon("Installing the addon…", move |config| {
            for flavor in &flavors {
                setup::install_into(config, flavor, false, false)?;
            }
            Ok(Some(installed_note(flavors.len())))
        });
    }

    fn set_flavor(&self, flavor: PathBuf, on: bool) {
        let busy = if on { "Installing the addon…" } else { "Removing the addon…" };
        self.change_addon(busy, move |config| {
            if on {
                setup::install_into(config, &flavor, false, false)?;
                Ok(Some(installed_note(1)))
            } else {
                setup::remove_from(config, &flavor)?;
                Ok(None)
            }
        });
    }

    fn set_slots(&self, slots: u32) {
        self.change_addon("Updating the addon…", move |config| {
            config.live_slots = slots;
            setup::reinstall_everywhere(config)?;
            Ok(None)
        });
    }

    /// Change the addon install (on a thread) while the game is closed: the
    /// game only picks up new addon folders when it starts.
    fn change_addon(
        &self,
        busy: &str,
        change: impl FnOnce(&mut BridgeConfig) -> Result<Option<String>> + Send + 'static,
    ) {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.wow_busy.is_some() {
            return;
        }
        if watch::wow_running() {
            jobs.wow_error = Some("Close World of Warcraft first: it only loads addon changes when it starts.".into());
            return;
        }
        jobs.wow_busy = Some(busy.to_string());
        jobs.wow_error = None;
        jobs.wow_note = None;
        drop(jobs);
        self.spawn(move |jobs| {
            let mut config = match load_config() {
                Ok(config) => config,
                Err(error) => {
                    let mut jobs = jobs.lock().unwrap();
                    jobs.wow_busy = None;
                    jobs.wow_error = Some(error.to_string());
                    return;
                }
            };
            let result = change(&mut config);
            // Save what was done even if a later step failed.
            let saved = save_config(&config);
            let mut jobs = jobs.lock().unwrap();
            jobs.wow_busy = None;
            match result.and_then(|note| saved.map(|()| note).map_err(Error::from)) {
                Ok(note) => jobs.wow_note = note,
                Err(error) => {
                    log!("changing the addon failed: {error}");
                    jobs.wow_error = Some(error.to_string());
                }
            }
        });
    }
}

fn installed_note(count: usize) -> String {
    let folders = if count == 1 { "one game folder".to_string() } else { format!("{count} game folders") };
    format!("Installed the addon into {folders}. Type /t3 in game to open it.")
}

fn edit_config(change: impl FnOnce(&mut BridgeConfig)) -> Result<()> {
    let mut config = load_config()?;
    change(&mut config);
    save_config(&config)?;
    Ok(())
}

/// Sign in to `server_url` (unless already signed in), remember it, and
/// describe the account.
fn sign_in(server_url: &str, cancel: &AtomicBool, started: &dyn Fn(&str)) -> Result<String> {
    oauth::check_sign_in_supported(server_url)?;
    let mut client = match T3Client::connect(server_url) {
        Ok(client) => client,
        Err(Error::AuthRequired(_)) => {
            let port = load_config()?.oauth_port;
            let pending = oauth::begin_login(server_url, port)?;
            started(&pending.authorization_url);
            app::open_external(&pending.authorization_url);
            pending.finish_unless(SIGN_IN_TIMEOUT, cancel)?;
            T3Client::connect(server_url)?
        }
        Err(error) => return Err(error),
    };
    let projects = client.list_projects()?;
    edit_config(|config| config.server_url = Some(server_url.to_string()))?;
    log!("signed in to {server_url}");
    let access =
        if client.can_operate() { "can send messages" } else { "read only: you can watch threads but not send" };
    Ok(format!("Signed in to {}: {} project(s), {access}.", url::host(server_url), projects.len()))
}

/// Everything the page shows.
fn state_json(config: &BridgeConfig, jobs: &Jobs, info: &TrayInfo, game_running: bool) -> Value {
    let (sign_in, days_left) = match config.server_url.as_deref().map(setup::sign_in_state) {
        None | Some(SignIn::No) => ("no", None),
        Some(SignIn::Expired) => ("expired", None),
        Some(SignIn::Yes(days)) => ("yes", days),
    };
    let mut flavors: Vec<PathBuf> = config.wow_flavor_dirs.clone();
    for flavor in jobs.detected.iter().flatten() {
        if !flavors.contains(flavor) {
            flavors.push(flavor.clone());
        }
    }
    let flavors: Vec<Value> = flavors.iter().map(|flavor| flavor_json(flavor)).collect();
    json!({
        "version": BRIDGE_VERSION,
        "platform": if cfg!(windows) { "windows" } else { "macos" },
        "status": { "text": info.status, "attention": info.attention },
        "env": {
            "url": jobs.sign_in.as_ref().map(|job| job.url.clone()).or(config.server_url.clone()),
            "signIn": sign_in,
            "daysLeft": days_left,
            "signingIn": jobs.sign_in.is_some(),
            "authUrl": jobs.sign_in.as_ref().and_then(|job| job.authorization_url.clone()),
            "error": jobs.sign_in_error,
            "note": jobs.sign_in_note,
        },
        "wow": {
            "running": game_running,
            "busy": jobs.wow_busy,
            "error": jobs.wow_error,
            "note": jobs.wow_note,
            "looked": jobs.detected.is_some(),
            "flavors": flavors,
        },
        "slots": config.live_slots,
        "slotsMax": setup::MAX_LIVE_SLOTS,
        "update": { "auto": config.auto_update, "text": info.update_text, "enabled": info.update_enabled },
        "dictation": dictation_json(&config.dictation, &dictation::status()),
    })
}

fn dictation_json(config: &crate::config::DictationConfig, status: &dictation::Status) -> Value {
    let access = match status.access {
        Access::Unknown => "unknown",
        Access::Asking => "asking",
        Access::Granted => "granted",
        Access::Denied(_) => "denied",
    };
    json!({
        "enabled": config.enabled,
        "shortcut": dictation::shortcut_label(&config.hotkey).unwrap_or_else(|| config.hotkey.clone()),
        "language": config.language,
        "access": access,
        "problem": matches!(status.access, Access::Denied(_)).then(|| status.access_problem()),
        "shortcutError": status.shortcut_error,
        "lastError": status.last_error,
    })
}

fn flavor_json(flavor: &Path) -> Value {
    let installed = wow::is_installed(flavor);
    json!({
        "path": flavor,
        "label": setup::flavor_label(flavor),
        "root": flavor.parent().unwrap_or(flavor),
        "installed": installed,
        "slots": if installed { wow::installed_live_slots(flavor) } else { 0 },
        "linked": installed && wow::addon_linked(flavor),
    })
}

/// The menu bar while the window is open: copy and paste only work in the
/// page through an Edit menu.
#[cfg(target_os = "macos")]
fn mac_menu() -> &'static tray_icon::menu::Menu {
    use std::sync::OnceLock;
    use tray_icon::menu::{Menu, PredefinedMenuItem, Submenu};

    struct MainThreadMenu(Menu);
    // Only touched from the event loop, on the main thread.
    unsafe impl Send for MainThreadMenu {}
    unsafe impl Sync for MainThreadMenu {}
    static MENU: OnceLock<MainThreadMenu> = OnceLock::new();
    &MENU
        .get_or_init(|| {
            let menu = Menu::new();
            let app_menu = Submenu::with_items(
                app::APP_NAME,
                true,
                &[
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::close_window(None),
                ],
            );
            let edit = Submenu::with_items(
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(None),
                    &PredefinedMenuItem::redo(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::cut(None),
                    &PredefinedMenuItem::copy(None),
                    &PredefinedMenuItem::paste(None),
                    &PredefinedMenuItem::select_all(None),
                ],
            );
            if let (Ok(app_menu), Ok(edit)) = (app_menu, edit) {
                let _ = menu.append_items(&[&app_menu, &edit]);
            }
            MainThreadMenu(menu)
        })
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> TrayInfo {
        TrayInfo {
            status: "Waiting for World of Warcraft".into(),
            attention: false,
            update_text: "Check for updates".into(),
            update_enabled: true,
        }
    }

    #[test]
    fn describes_the_state_for_the_page() {
        let jobs = Jobs {
            detected: Some(vec![PathBuf::from("/nowhere/World of Warcraft/_retail_")]),
            wow_busy: Some("Installing the addon…".into()),
            ..Jobs::default()
        };
        let config = BridgeConfig { live_slots: 120, ..BridgeConfig::default() };
        let state = state_json(&config, &jobs, &info(), true);
        assert_eq!(state["version"], BRIDGE_VERSION);
        assert_eq!(state["status"]["text"], "Waiting for World of Warcraft");
        assert_eq!(state["wow"]["busy"], "Installing the addon…");
        let flavors = state["wow"]["flavors"].as_array().unwrap();
        let retail = flavors.iter().find(|f| f["label"] == "Retail").unwrap();
        assert_eq!(retail["installed"], false);
        assert_eq!(retail["root"], "/nowhere/World of Warcraft");
        assert_eq!(state["wow"]["running"], true);
        assert_eq!(state["env"]["signIn"], "no");
        assert_eq!(state["slots"], 120);
        assert_eq!(state["update"]["text"], "Check for updates");
        assert_eq!(state["dictation"]["enabled"], false);
        assert_eq!(state["dictation"]["shortcut"], "Ctrl+Shift+Space");
    }

    #[test]
    fn describes_dictation_problems() {
        let config = crate::config::DictationConfig { enabled: true, ..Default::default() };
        let status = dictation::Status {
            access: Access::Denied("Allow T3 WoW under Microphone".into()),
            shortcut_error: Some("taken".into()),
            last_error: None,
        };
        let state = dictation_json(&config, &status);
        assert_eq!(state["access"], "denied");
        assert_eq!(state["problem"], "Allow T3 WoW under Microphone");
        assert_eq!(state["shortcutError"], "taken");
    }

    #[test]
    fn the_page_renders_state_and_sends_commands() {
        assert!(PAGE.contains("window.render"));
        for command in [
            "ready",
            "signIn",
            "cancelSignIn",
            "signOut",
            "addWow",
            "setFlavor",
            "setSlots",
            "setAutoUpdate",
            "setDictation",
            "setDictationShortcut",
            "setDictationLanguage",
        ] {
            assert!(PAGE.contains(&format!("'{command}'")), "page never sends {command}");
        }
    }
}
