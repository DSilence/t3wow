//! Settings changed while the game runs (from the tray's Settings window)
//! take effect right away: a bridge waiting for setup starts syncing, and a
//! syncing one starts over with the new folders.

mod support;

use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use t3wow::config::BridgeConfig;
use t3wow::run::{RunOptions, RunState, run_bridge};
use t3wow::wow::{INBOX_ADDON, install_addon};

use support::fake_t3::FakeT3;
use support::wait_for;

#[test]
fn applies_new_settings_while_the_game_runs() {
    let tmp = std::env::temp_dir().join(format!("t3wow-settings-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    // The only test in this binary, so nothing else reads the environment concurrently.
    unsafe { std::env::set_var("T3WOW_HOME", tmp.join("home")) };
    let retail = tmp.join("World of Warcraft").join("_retail_");
    let classic = tmp.join("World of Warcraft").join("_classic_");
    let inbox = |flavor: &std::path::Path| flavor.join("Interface/AddOns").join(INBOX_ADDON).join("Data.lua");
    let t3 = FakeT3::start();
    install_addon(&retail, false, 1, false).unwrap();
    install_addon(&classic, false, 1, false).unwrap();

    // Not signed in yet.
    let config = Mutex::new(BridgeConfig {
        server_url: None,
        wow_flavor_dirs: vec![retail.clone()],
        active_poll_ms: 50,
        idle_poll_ms: 100,
        ..BridgeConfig::default()
    });
    let states = Mutex::new(Vec::<RunState>::new());
    let stop = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let bridge = scope.spawn(|| {
            let load = || Ok(config.lock().unwrap().clone());
            let game_running = || true;
            let report = |state: RunState| {
                let mut states = states.lock().unwrap();
                if states.last() != Some(&state) {
                    states.push(state);
                }
            };
            run_bridge(&RunOptions {
                load_config: &load,
                game_running: Some(&game_running),
                stop: &stop,
                report: Some(&report),
            })
        });

        wait_for("the bridge to ask for setup", || {
            states.lock().unwrap().iter().any(|s| matches!(s, RunState::Setup(_))).then_some(())
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(!read(&inbox(&retail)).contains("Fix the login bug"));

        // Signed in from Settings: syncing starts without restarting the game.
        config.lock().unwrap().server_url = Some(t3.url.clone());
        wait_for("a snapshot after signing in", || read(&inbox(&retail)).contains("Fix the login bug").then_some(()));

        // Another game folder ticked in Settings: it gets snapshots too.
        config.lock().unwrap().wow_flavor_dirs.push(classic.clone());
        wait_for("a snapshot in the new folder", || read(&inbox(&classic)).contains("Fix the login bug").then_some(()));

        stop.store(true, Ordering::Relaxed);
        bridge.join().unwrap().unwrap();
    });
    let _ = fs::remove_dir_all(&tmp);
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
