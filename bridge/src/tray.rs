//! The tray app: a menu bar (macOS) or notification area (Windows) icon that
//! runs the bridge while it is open. The bridge idles until World of Warcraft
//! starts, syncs while it runs, and stops when you quit the app. It also
//! installs new releases between game sessions (see `update.rs`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::{BRIDGE_VERSION, load_config};
use crate::error::Result;
use crate::lock::BridgeLock;
use crate::log;
use crate::run::{RunOptions, RunState, run_bridge};
use crate::service;
use crate::update;
use crate::watch::wow_running;

static STOP: AtomicBool = AtomicBool::new(false);
/// "Check for updates" was clicked.
static CHECK_NOW: AtomicBool = AtomicBool::new(false);
/// An update is installed: start the new version and quit.
static RESTART: AtomicBool = AtomicBool::new(false);

const FIRST_UPDATE_CHECK: Duration = Duration::from_secs(20);
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
enum TrayState {
    Starting,
    Run(RunState),
    /// Another bridge holds the lock, or the bridge stopped with an error.
    Blocked(String),
}

#[derive(Debug, Clone, PartialEq)]
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded; installs once World of Warcraft is closed.
    Ready(String),
    Installing(String),
    Failed(String),
}

enum UserEvent {
    Menu(MenuEvent),
}

fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    }
}

fn describe(state: &TrayState) -> String {
    match state {
        TrayState::Starting => "Starting…".into(),
        TrayState::Blocked(message) => shorten(message, 70),
        TrayState::Run(RunState::Waiting) => "Waiting for World of Warcraft".into(),
        TrayState::Run(RunState::Setup(message)) => format!("Setup needed: {}", shorten(message, 60)),
        TrayState::Run(RunState::SignInNeeded) => "Sign in again: run `t3wow login`".into(),
        TrayState::Run(RunState::Error(message)) => format!("Can't reach T3: {}", shorten(message, 56)),
        TrayState::Run(RunState::Syncing { server, running, read_only }) => {
            let mut text = format!("Syncing with {server}");
            if *running > 0 {
                text.push_str(&format!(", {running} running"));
            }
            if *read_only {
                text.push_str(" (read only)");
            }
            text
        }
    }
}

/// The update menu item's text, and whether clicking it checks for updates.
fn describe_update(state: &UpdateState) -> (String, bool) {
    match state {
        UpdateState::Idle => ("Check for updates".into(), true),
        UpdateState::Checking => ("Checking for updates…".into(), false),
        UpdateState::UpToDate => ("Up to date (check again)".into(), true),
        UpdateState::Downloading(version) => (format!("Downloading {version}…"), false),
        UpdateState::Ready(version) => (format!("Update {version} installs when WoW closes"), false),
        UpdateState::Installing(version) => (format!("Installing {version}…"), false),
        UpdateState::Failed(message) => (format!("Update failed: {} (retry)", shorten(message, 40)), true),
    }
}

fn sleep_unless_stopped(duration: Duration) {
    let deadline = Instant::now() + duration;
    while !STOP.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Checks for a new release now and then (when `autoUpdate` is on) and when
/// asked from the menu, downloads it, and installs it once the game is closed
/// and the bridge idle, since the addon is replaced too. Then asks the event
/// loop to restart the app.
fn run_updater(update: Arc<Mutex<UpdateState>>, bridge: Arc<Mutex<TrayState>>) {
    let set = |state: UpdateState| *update.lock().unwrap() = state;
    let mut next_check = Instant::now() + FIRST_UPDATE_CHECK;
    while !STOP.load(Ordering::Relaxed) {
        let manual = CHECK_NOW.swap(false, Ordering::Relaxed);
        let automatic = load_config().map(|config| config.auto_update).unwrap_or(true);
        if !manual && !(automatic && Instant::now() >= next_check) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        next_check = Instant::now() + UPDATE_INTERVAL;
        set(UpdateState::Checking);
        let release = match update::check(&update::latest_release_url(), BRIDGE_VERSION) {
            Ok(Some(release)) => release,
            Ok(None) => {
                set(UpdateState::UpToDate);
                continue;
            }
            Err(error) => {
                log!("update check failed: {error}");
                // Being offline now and then is no news; say so only when asked.
                set(if manual { UpdateState::Failed(error.to_string()) } else { UpdateState::Idle });
                continue;
            }
        };
        let version = release.version.clone();
        log!("downloading version {version}");
        set(UpdateState::Downloading(version.clone()));
        let file = match update::download(&release, &update::download_dir()) {
            Ok(file) => file,
            Err(error) => {
                log!("update download failed: {error}");
                set(UpdateState::Failed(error.to_string()));
                continue;
            }
        };
        set(UpdateState::Ready(version.clone()));
        let busy = || wow_running() || matches!(*bridge.lock().unwrap(), TrayState::Run(RunState::Syncing { .. }));
        while busy() && !STOP.load(Ordering::Relaxed) {
            sleep_unless_stopped(Duration::from_secs(3));
        }
        if STOP.load(Ordering::Relaxed) {
            return;
        }
        log!("installing version {version}");
        set(UpdateState::Installing(version.clone()));
        match update::install(&file) {
            Ok(output) => {
                log!("updated to {version}: {}", output.trim().replace('\n', "; "));
                RESTART.store(true, Ordering::Relaxed);
                return;
            }
            Err(error) => {
                log!("update failed: {error}");
                set(UpdateState::Failed(error.to_string()));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// An outlined speech bubble: waiting for World of Warcraft.
    Outline,
    /// A filled bubble with "…" cut out: syncing.
    Talking,
    /// An outlined bubble with a badge: something needs attention.
    Badge,
}

fn shape_for(state: &TrayState) -> Shape {
    match state {
        TrayState::Starting | TrayState::Run(RunState::Waiting) => Shape::Outline,
        TrayState::Run(RunState::Syncing { .. }) => Shape::Talking,
        _ => Shape::Badge,
    }
}

type Point = (f32, f32);

/// Signed distance from `p` to a rounded rectangle (left, top, right, bottom);
/// negative inside.
fn rounded_rect((x, y): Point, (left, top, right, bottom): (f32, f32, f32, f32), radius: f32) -> f32 {
    let qx = (x - (left + right) / 2.0).abs() - ((right - left) / 2.0 - radius);
    let qy = (y - (top + bottom) / 2.0).abs() - ((bottom - top) / 2.0 - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// Signed distance from `p` to a triangle; negative inside.
fn triangle((x, y): Point, corners: [Point; 3]) -> f32 {
    let mut distance = f32::MAX;
    let mut sides = [0.0; 3];
    for i in 0..3 {
        let (ax, ay) = corners[i];
        let (bx, by) = corners[(i + 1) % 3];
        let (ex, ey) = (bx - ax, by - ay);
        let (wx, wy) = (x - ax, y - ay);
        let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        distance = distance.min((wx - ex * t).hypot(wy - ey * t));
        sides[i] = ex * wy - ey * wx;
    }
    let inside = sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0);
    if inside { -distance } else { distance }
}

const ICON_SIZE: usize = 32;

fn icon(shape: Shape) -> Icon {
    Icon::from_rgba(icon_pixels(shape), ICON_SIZE as u32, ICON_SIZE as u32).expect("valid icon")
}

/// A 32x32 RGBA speech bubble drawn in code, like the app icon: outlined
/// while waiting, filled and "talking" while syncing, badged when something
/// needs attention.
fn icon_pixels(shape: Shape) -> Vec<u8> {
    const SIZE: usize = ICON_SIZE;
    const STROKE: f32 = 2.5;
    // macOS tints template images itself; Windows taskbars are usually dark.
    let rgb: [u8; 3] = if cfg!(target_os = "macos") { [0, 0, 0] } else { [235, 235, 240] };
    // A solid tail that starts inside the body, so the two join without a seam.
    let body = (2.0, 4.0, 30.0, 22.0);
    let hollow = (body.0 + STROKE, body.1 + STROKE, body.2 - STROKE, body.3 - STROKE);
    let tail = [(9.5, 19.0), (17.5, 19.0), (7.5, 29.0)];
    let (badge_x, badge_y, badge_r) = (26.0, 6.0, 4.5);
    // Coverage of a distance field with a one-pixel soft edge.
    let cover = |distance: f32| (0.5 - distance).clamp(0.0, 1.0);
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for py in 0..SIZE {
        for px in 0..SIZE {
            let p = (px as f32 + 0.5, py as f32 + 0.5);
            let bubble = rounded_rect(p, body, 6.0).min(triangle(p, tail));
            let outline = cover(bubble) * (1.0 - cover(rounded_rect(p, hollow, 6.0 - STROKE)));
            let alpha = match shape {
                Shape::Outline => outline,
                Shape::Talking => {
                    let dots = [10.0, 16.0, 22.0].into_iter().map(|dx| (p.0 - dx).hypot(p.1 - 13.0) - 2.2);
                    cover(bubble) * (1.0 - cover(dots.fold(f32::MAX, f32::min)))
                }
                Shape::Badge => {
                    let dot = (p.0 - badge_x).hypot(p.1 - badge_y) - badge_r;
                    // A gap around the badge keeps it apart from the outline.
                    (outline * (1.0 - cover(dot - 2.0))).max(cover(dot))
                }
            };
            let i = (py * SIZE + px) * 4;
            rgba[i..i + 3].copy_from_slice(&rgb);
            rgba[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    rgba
}

fn open_file(path: &std::path::Path) {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("notepad")
    };
    let _ = command.arg(path).spawn();
}

#[cfg(windows)]
fn detach_console() {
    // Started from the Start Menu, the console window would stay open.
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }
}

/// Start the bridge on a background thread. It holds the lock while it runs.
fn start_bridge(state: Arc<Mutex<TrayState>>) -> Option<std::thread::JoinHandle<()>> {
    // Right after an update the old app may still be letting go of the lock.
    let mut attempt = BridgeLock::acquire();
    for _ in 0..20 {
        if attempt.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        attempt = BridgeLock::acquire();
    }
    let lock = match attempt {
        Ok(lock) => lock,
        Err(error) => {
            log!("{error}");
            *state.lock().unwrap() = TrayState::Blocked(error.to_string());
            return None;
        }
    };
    Some(std::thread::spawn(move || {
        let _lock = lock;
        let report_state = state.clone();
        let report = move |run: RunState| *report_state.lock().unwrap() = TrayState::Run(run);
        let load = || load_config().map_err(Into::into);
        let game = || wow_running();
        let options = RunOptions { load_config: &load, game_running: Some(&game), stop: &STOP, report: Some(&report) };
        if let Err(error) = run_bridge(&options) {
            log!("bridge stopped: {error}");
            *state.lock().unwrap() = TrayState::Blocked(format!("Bridge stopped: {error}"));
        }
    }))
}

/// Stop the bridge thread, giving it a moment to finish; do not wait on a slow request.
fn stop_bridge(bridge: &mut Option<std::thread::JoinHandle<()>>) {
    STOP.store(true, Ordering::Relaxed);
    if let Some(handle) = bridge.take() {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

pub fn run_tray() -> Result<()> {
    #[cfg(windows)]
    detach_console();
    crate::log::log_to_file(service::log_file())?;
    log!("T3 WoW tray app {BRIDGE_VERSION}");
    let state = Arc::new(Mutex::new(TrayState::Starting));
    // The old login service runs its own bridge without the lock; two would
    // both deliver commands.
    let mut bridge = if service::legacy_installed() {
        log!("the old login service is still installed");
        *state.lock().unwrap() =
            TrayState::Blocked("Old login service still installed: run `t3wow app install`".into());
        None
    } else {
        start_bridge(state.clone())
    };
    let exe = std::env::current_exe()?;
    let update_state = Arc::new(Mutex::new(UpdateState::Idle));
    {
        let (update_state, state) = (update_state.clone(), state.clone());
        std::thread::spawn(move || run_updater(update_state, state));
    }

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.lock().unwrap().send_event(UserEvent::Menu(event));
    }));

    let menu = Menu::new();
    let heading = MenuItem::new(format!("T3 WoW bridge {BRIDGE_VERSION}"), false, None);
    let status = MenuItem::new(describe(&TrayState::Starting), false, None);
    let open_log = MenuItem::new("Open log", true, None);
    let (update_text, update_enabled) = describe_update(&UpdateState::Idle);
    let update_item = MenuItem::new(update_text, update_enabled, None);
    let quit = MenuItem::new("Quit T3 WoW", true, None);
    menu.append_items(&[
        &heading,
        &status,
        &PredefinedMenuItem::separator(),
        &update_item,
        &open_log,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| crate::error::Error::other(format!("tray menu: {e}")))?;

    let mut tray: Option<TrayIcon> = None;
    let mut menu = Some(menu);
    let mut shown: Option<TrayState> = None;
    let mut shown_update = UpdateState::Idle;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
        match event {
            // macOS wants the icon created once the event loop runs.
            Event::NewEvents(StartCause::Init) => {
                let builder = TrayIconBuilder::new().with_menu(Box::new(menu.take().expect("menu")));
                // A template image follows the menu bar's light or dark look.
                #[cfg(target_os = "macos")]
                let builder = builder.with_icon_templated(icon(Shape::Outline));
                #[cfg(not(target_os = "macos"))]
                let builder = builder.with_icon(icon(Shape::Outline));
                let built = builder.with_tooltip("T3 WoW").build();
                match built {
                    Ok(icon) => tray = Some(icon),
                    Err(error) => {
                        log!("could not create the tray icon: {error}");
                        *flow = ControlFlow::Exit;
                        return;
                    }
                }
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                if event.id == *quit.id() {
                    log!("quitting");
                    stop_bridge(&mut bridge);
                    tray.take();
                    *flow = ControlFlow::Exit;
                    return;
                }
                if event.id == *open_log.id() {
                    open_file(&service::log_file());
                }
                if event.id == *update_item.id() {
                    CHECK_NOW.store(true, Ordering::Relaxed);
                }
            }
            _ => {}
        }
        if RESTART.load(Ordering::Relaxed) {
            log!("restarting with the new version");
            stop_bridge(&mut bridge);
            if let Err(error) = update::relaunch(&exe) {
                log!("could not start the new version: {error}");
            }
            tray.take();
            *flow = ControlFlow::Exit;
            return;
        }
        let current_update = update_state.lock().unwrap().clone();
        if current_update != shown_update {
            let (text, enabled) = describe_update(&current_update);
            update_item.set_text(&text);
            update_item.set_enabled(enabled);
            shown_update = current_update;
        }
        let current = state.lock().unwrap().clone();
        if shown.as_ref() != Some(&current) {
            let text = describe(&current);
            status.set_text(&text);
            if let Some(tray) = &tray {
                let _ = tray.set_tooltip(Some(format!("T3 WoW: {text}")));
                if shown.as_ref().map(shape_for) != Some(shape_for(&current)) {
                    #[cfg(target_os = "macos")]
                    let _ = tray.set_icon_templated(Some(icon(shape_for(&current))));
                    #[cfg(not(target_os = "macos"))]
                    let _ = tray.set_icon(Some(icon(shape_for(&current))));
                }
            }
            shown = Some(current);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_states() {
        let syncing = TrayState::Run(RunState::Syncing { server: "box:3773".into(), running: 2, read_only: false });
        assert_eq!(describe(&syncing), "Syncing with box:3773, 2 running");
        assert_eq!(describe(&TrayState::Run(RunState::Waiting)), "Waiting for World of Warcraft");
        let long = TrayState::Run(RunState::Error("x".repeat(200)));
        assert!(describe(&long).chars().count() < 80);
    }

    #[test]
    fn describes_updates() {
        assert_eq!(describe_update(&UpdateState::Idle), ("Check for updates".into(), true));
        let (text, enabled) = describe_update(&UpdateState::Ready("0.2.0".into()));
        assert_eq!(text, "Update 0.2.0 installs when WoW closes");
        assert!(!enabled);
        assert!(describe_update(&UpdateState::Failed("x".repeat(200))).0.chars().count() < 70);
    }

    fn alpha(shape: Shape, x: usize, y: usize) -> u8 {
        icon_pixels(shape)[(y * ICON_SIZE + x) * 4 + 3]
    }

    #[test]
    fn draws_a_speech_bubble_per_state() {
        // Hollow while waiting, solid with "…" cut out while syncing.
        assert_eq!(alpha(Shape::Outline, 16, 12), 0);
        assert_eq!(alpha(Shape::Outline, 3, 12), 255);
        assert_eq!(alpha(Shape::Talking, 16, 7), 255);
        assert_eq!(alpha(Shape::Talking, 16, 12), 0);
        // The tail and the badge.
        assert_eq!(alpha(Shape::Outline, 10, 24), 255);
        assert_eq!(alpha(Shape::Badge, 26, 8), 255);
        assert_eq!(alpha(Shape::Outline, 26, 8), 0);
    }
}
