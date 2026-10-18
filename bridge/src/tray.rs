//! The tray app: a menu bar (macOS) or notification area (Windows) icon that
//! runs the bridge while it is open. The bridge idles until World of Warcraft
//! starts, syncs while it runs, and stops when you quit the app.

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
use crate::watch::wow_running;

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq)]
enum TrayState {
    Starting,
    Run(RunState),
    /// Another bridge holds the lock, or the bridge stopped with an error.
    Blocked(String),
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

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Ring,
    Disc,
    RingWithDot,
}

fn shape_for(state: &TrayState) -> Shape {
    match state {
        TrayState::Starting | TrayState::Run(RunState::Waiting) => Shape::Ring,
        TrayState::Run(RunState::Syncing { .. }) => Shape::Disc,
        _ => Shape::RingWithDot,
    }
}

/// A 32x32 icon drawn in code: a ring while waiting, a disc while syncing, a
/// ring with a dot when something needs attention.
fn icon(shape: Shape) -> Icon {
    const SIZE: usize = 32;
    // macOS tints template images itself; Windows taskbars are usually dark.
    let rgb: [u8; 3] = if cfg!(target_os = "macos") { [0, 0, 0] } else { [235, 235, 240] };
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // Coverage with a one-pixel soft edge.
            let inside = |radius: f32| (radius - d + 0.5).clamp(0.0, 1.0);
            let alpha = match shape {
                Shape::Disc => inside(11.0),
                Shape::Ring => inside(11.0) - inside(8.0),
                Shape::RingWithDot => (inside(11.0) - inside(8.0)) + inside(4.0),
            };
            let i = (y * SIZE + x) * 4;
            rgba[i..i + 3].copy_from_slice(&rgb);
            rgba[i + 3] = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("valid icon")
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
    let lock = match BridgeLock::acquire() {
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
    let quit = MenuItem::new("Quit T3 WoW", true, None);
    menu.append_items(&[
        &heading,
        &status,
        &PredefinedMenuItem::separator(),
        &open_log,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| crate::error::Error::other(format!("tray menu: {e}")))?;

    let mut tray: Option<TrayIcon> = None;
    let mut menu = Some(menu);
    let mut shown: Option<TrayState> = None;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
        match event {
            // macOS wants the icon created once the event loop runs.
            Event::NewEvents(StartCause::Init) => {
                let builder = TrayIconBuilder::new().with_menu(Box::new(menu.take().expect("menu")));
                // A template image follows the menu bar's light or dark look.
                #[cfg(target_os = "macos")]
                let builder = builder.with_icon_templated(icon(Shape::Ring));
                #[cfg(not(target_os = "macos"))]
                let builder = builder.with_icon(icon(Shape::Ring));
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
                    STOP.store(true, Ordering::Relaxed);
                    // The bridge notices within a moment; do not wait on a slow request.
                    if let Some(handle) = bridge.take() {
                        let deadline = Instant::now() + Duration::from_secs(3);
                        while !handle.is_finished() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                    tray.take();
                    *flow = ControlFlow::Exit;
                    return;
                }
                if event.id == *open_log.id() {
                    open_file(&service::log_file());
                }
            }
            _ => {}
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
}
