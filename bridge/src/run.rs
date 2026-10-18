//! The bridge loop: read SavedVariables, deliver commands, sync from T3 and
//! publish the snapshot files the addon loads.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::addon_state::{AddonState, AddonStateReader, merge_states, state_from_db};
use crate::config::{BRIDGE_VERSION, BridgeConfig};
use crate::error::{Error, Result};
use crate::log;
use crate::lua::to_lua;
use crate::oauth::now_ms;
use crate::outbox::OutboxProcessor;
use crate::shot::ShotReader;
use crate::sync::{BridgeStatus, SyncEngine};
use crate::t3::T3Client;
use crate::url;
use crate::wow::{installed_live_slots, is_installed, publish_snapshot, saved_variables_files};

/// Republish unchanged data this often so the addon can tell the bridge is alive.
const HEARTBEAT: Duration = Duration::from_secs(20);
const SAVED_VARIABLES_POLL: Duration = Duration::from_millis(750);
const GAME_POLL: Duration = Duration::from_secs(3);
const STEP: Duration = Duration::from_millis(250);

/// What the bridge is doing, for the tray menu.
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    /// Waiting for World of Warcraft to start.
    Waiting,
    /// Login or install is missing.
    Setup(String),
    Syncing {
        server: String,
        running: usize,
        read_only: bool,
    },
    SignInNeeded,
    Error(String),
}

pub struct RunOptions<'a> {
    /// Reloaded for every game session, so `login`/`install` take effect without a restart.
    pub load_config: &'a dyn Fn() -> Result<BridgeConfig>,
    /// When set, the bridge only syncs while this says the game is running.
    pub game_running: Option<&'a dyn Fn() -> bool>,
    pub stop: &'a AtomicBool,
    pub report: Option<&'a dyn Fn(RunState)>,
}

pub fn run_bridge(options: &RunOptions) -> Result<()> {
    let report = |state: RunState| {
        if let Some(report) = options.report {
            report(state);
        }
    };
    let Some(game_running) = options.game_running else {
        let result = (options.load_config)().and_then(|config| session(&config, None, options.stop, &report));
        if let Err(error) = &result {
            report(RunState::Setup(error.to_string()));
        }
        return result;
    };
    log!("T3 WoW bridge {BRIDGE_VERSION} waiting for World of Warcraft");
    report(RunState::Waiting);
    while !options.stop.load(Ordering::Relaxed) {
        if !game_running() {
            sleep_unless_stopped(GAME_POLL, options.stop);
            continue;
        }
        log!("World of Warcraft started");
        let result =
            (options.load_config)().and_then(|config| session(&config, Some(game_running), options.stop, &report));
        if let Err(error) = result {
            // Nothing to do until the setup is fixed; try again next time the game starts.
            log!("{error}");
            report(RunState::Setup(error.to_string()));
            while game_running() && !options.stop.load(Ordering::Relaxed) {
                sleep_unless_stopped(GAME_POLL, options.stop);
            }
        }
        if !options.stop.load(Ordering::Relaxed) {
            log!("World of Warcraft closed, waiting for it to start");
            report(RunState::Waiting);
        }
    }
    Ok(())
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(STEP.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Sync until stopped or, with `game_running`, until the game has closed.
fn session(
    config: &BridgeConfig,
    game_running: Option<&dyn Fn() -> bool>,
    stop: &AtomicBool,
    report: &dyn Fn(RunState),
) -> Result<()> {
    let server_url = config
        .server_url
        .clone()
        .ok_or_else(|| Error::other("No T3 environment configured. Run `t3wow login <mcp-url>` first."))?;
    let flavors: Vec<PathBuf> = config.wow_flavor_dirs.iter().filter(|f| is_installed(f)).cloned().collect();
    if flavors.is_empty() {
        return Err(Error::other("The addon is not installed anywhere. Run `t3wow install` first."));
    }

    let mut engine = SyncEngine::new(config.clone());
    let mut outbox = OutboxProcessor::load()?;
    let mut reader = AddonStateReader::default();
    let mut shots = ShotReader::new(flavors.iter().map(|f| f.join("Screenshots")).collect());
    // The latest state each addon install sent by screenshot.
    let mut shot_states: Vec<AddonState> = Vec::new();
    let slots: HashMap<PathBuf, u32> = flavors.iter().map(|f| (f.clone(), installed_live_slots(f))).collect();
    let mut status = BridgeStatus {
        status: "connecting",
        message: None,
        server: Some(url::host(&server_url).to_string()),
        can_operate: false,
    };
    let mut client: Option<T3Client> = None;
    let mut publisher = Publisher::default();
    let mut failures: u32 = 0;
    let mut game_closed = false;

    log!("T3 WoW bridge {BRIDGE_VERSION} -> {server_url}");
    for flavor in &flavors {
        log!("  {} ({} live slots)", flavor.display(), slots[flavor]);
    }
    let sv_files = || flavors.iter().flat_map(|f| saved_variables_files(f)).collect::<Vec<_>>();

    while !stop.load(Ordering::Relaxed) {
        let (saved, mut changed) = reader.read(&sv_files());
        for message in shots.poll() {
            let Some(state) = state_from_db(Path::new("screenshot"), &message, SystemTime::now()) else { continue };
            log!("screenshot from {}: outbox={}", state.instance_id, state.outbox.len());
            shot_states.retain(|s| s.instance_id != state.instance_id);
            shot_states.push(state);
            changed = true;
        }
        let states = merge_states(saved, &shot_states);
        if changed {
            let summary: Vec<String> =
                states.iter().map(|s| format!("{} outbox={}", account_name(&s.file), s.outbox.len())).collect();
            log!("game state: {}", if summary.is_empty() { "none".into() } else { summary.join(", ") });
        }
        let result = (|| -> Result<()> {
            let t3 = match &mut client {
                Some(t3) => t3,
                None => {
                    let t3 = T3Client::connect(&server_url)?;
                    log!(
                        "connected ({} tools, {})",
                        t3.tools.len(),
                        if t3.can_operate() { "can send messages" } else { "read only" }
                    );
                    client.insert(t3)
                }
            };
            let touched = outbox.process(t3, &states)?;
            engine.touch(touched);
            engine.tick(t3, &states)?;
            status.can_operate = t3.can_operate();
            Ok(())
        })();
        match result {
            Ok(()) => {
                failures = 0;
                status.status = "ok";
                status.message = (!status.can_operate).then(|| "Read-only sign-in: sending is disabled".to_string());
                report(RunState::Syncing {
                    server: status.server.clone().unwrap_or_default(),
                    running: engine.running_count(),
                    read_only: !status.can_operate,
                });
            }
            Err(error) => {
                failures += 1;
                client = None;
                if let Error::AuthRequired(_) = error {
                    status.status = "auth_required";
                    status.message = Some("Bridge needs to sign in again: run `t3wow login`".into());
                } else {
                    status.status = "error";
                    status.message = Some(error.to_string());
                }
                log!("{}: {}", status.status, status.message.as_deref().unwrap_or(""));
                report(match status.status {
                    "auth_required" => RunState::SignInNeeded,
                    _ => RunState::Error(status.message.clone().unwrap_or_default()),
                });
            }
        }
        let snapshot = engine.snapshot(&status, outbox.acks());
        publisher.publish(snapshot, &flavors, &slots);

        if let Some(game_running) = game_running
            && !game_running()
        {
            // WoW writes SavedVariables while logging out: one more pass delivers
            // what was queued last.
            if game_closed {
                break;
            }
            game_closed = true;
            continue;
        }

        let backoff = if failures == 0 { 0 } else { (2_000u64 << (failures - 1).min(5)).min(60_000) };
        let interval =
            if engine.active() || outbox.has_pending(&states) { config.active_poll_ms } else { config.idle_poll_ms };
        let wait = if status.status == "auth_required" { 30_000 } else { interval.max(backoff) };
        // Folders change when WoW adds a screenshot.
        let mut watched = sv_files();
        watched.extend(shots.dirs().iter().cloned());
        wait_for_change(Duration::from_millis(wait), &watched, stop);
    }
    Ok(())
}

fn account_name(file: &Path) -> String {
    file.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Sleep, but wake up early when WoW rewrites a SavedVariables file.
fn wait_for_change(duration: Duration, files: &[PathBuf], stop: &AtomicBool) {
    let mtimes = |files: &[PathBuf]| -> Vec<Option<SystemTime>> {
        files.iter().map(|f| fs::metadata(f).and_then(|m| m.modified()).ok()).collect()
    };
    let before = mtimes(files);
    let deadline = Instant::now() + duration;
    let mut next_check = Instant::now() + SAVED_VARIABLES_POLL;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(STEP.min(deadline.saturating_duration_since(Instant::now())));
        if Instant::now() >= next_check {
            if mtimes(files) != before {
                return;
            }
            next_check = Instant::now() + SAVED_VARIABLES_POLL;
        }
    }
}

#[derive(Default)]
struct Publisher {
    last_body: String,
    last_at: Option<Instant>,
    last_error: Option<String>,
}

impl Publisher {
    fn publish(&mut self, snapshot: Value, flavors: &[PathBuf], slots: &HashMap<PathBuf, u32>) {
        let body = to_lua(&snapshot);
        if body == self.last_body && self.last_at.is_some_and(|at| at.elapsed() < HEARTBEAT) {
            return;
        }
        let now = now_ms();
        let mut full = snapshot;
        if let Value::Object(map) = &mut full {
            map.insert("v".into(), json!(1));
            map.insert("seq".into(), json!(now));
            map.insert("generatedAt".into(), json!(now / 1000));
            map.insert("version".into(), json!(BRIDGE_VERSION));
        }
        let chunk = to_lua(&full);
        let mut error = None;
        for flavor in flavors {
            if let Err(e) = publish_snapshot(flavor, &chunk, slots.get(flavor).copied().unwrap_or(0)) {
                let hint = if e.kind() == std::io::ErrorKind::PermissionDenied && cfg!(target_os = "macos") {
                    " (allow t3wow under System Settings > Privacy & Security > Files and Folders or Full Disk Access)"
                } else {
                    ""
                };
                error = Some(format!("cannot write to {}: {e}{hint}", flavor.display()));
            }
        }
        if error.is_some() && error != self.last_error {
            log!("{}", error.as_deref().unwrap_or_default());
        }
        self.last_error = error;
        self.last_body = body;
        self.last_at = Some(Instant::now());
    }
}
