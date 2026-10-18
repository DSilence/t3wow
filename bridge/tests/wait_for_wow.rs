//! `run --wait-for-wow`: idle while the game is closed, sync while it runs,
//! and deliver what the game wrote on logout before going back to sleep.

mod support;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use t3wow::config::BridgeConfig;
use t3wow::run::{RunOptions, RunState, run_bridge};
use t3wow::wow::{INBOX_ADDON, install_addon};

use support::fake_t3::FakeT3;
use support::wait_for;

const LOGOUT_SAVE: &str = r#"
T3WowDB = {
["instanceId"] = "inst-1",
["outbox"] = {
{ ["seq"] = 1, ["kind"] = "send", ["threadId"] = "t1", ["text"] = "queued at logout" },
},
}
"#;

#[test]
fn syncs_only_while_the_game_runs() {
    let tmp = std::env::temp_dir().join(format!("t3wow-wait-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    // The only test in this binary, so nothing else reads the environment concurrently.
    unsafe { std::env::set_var("T3WOW_HOME", tmp.join("home")) };
    let flavor = tmp.join("World of Warcraft").join("_classic_");
    let sv_file = flavor.join("WTF/Account/ACC/SavedVariables/T3Wow.lua");
    let inbox_file = flavor.join("Interface/AddOns").join(INBOX_ADDON).join("Data.lua");
    let t3 = FakeT3::start();
    install_addon(&flavor, false, 1, false).unwrap();

    let config = BridgeConfig {
        server_url: Some(t3.url.clone()),
        wow_flavor_dirs: vec![flavor.clone()],
        active_poll_ms: 50,
        idle_poll_ms: 100,
        ..BridgeConfig::default()
    };
    let running = AtomicBool::new(false);
    let states = std::sync::Mutex::new(Vec::<RunState>::new());
    let stop = AtomicBool::new(false);
    let calls = || t3.state.lock().unwrap().calls.len();

    std::thread::scope(|scope| {
        let bridge = scope.spawn(|| {
            let load = || Ok(config.clone());
            let game_running = || running.load(Ordering::Relaxed);
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

        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(calls(), 0, "nothing happens while the game is closed");

        running.store(true, Ordering::Relaxed);
        wait_for("a snapshot once the game starts", || read(&inbox_file).contains("Fix the login bug").then_some(()));

        // Logging out writes SavedVariables, then the process exits.
        fs::create_dir_all(sv_file.parent().unwrap()).unwrap();
        fs::write(&sv_file, LOGOUT_SAVE).unwrap();
        running.store(false, Ordering::Relaxed);
        wait_for("the logout command", || (t3.state.lock().unwrap().count("t3_thread_send") == 1).then_some(()));

        // Back to sleep: no more calls.
        std::thread::sleep(Duration::from_millis(300));
        let settled = calls();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(calls(), settled, "the bridge stops polling after the game closes");

        stop.store(true, Ordering::Relaxed);
        bridge.join().unwrap().unwrap();
        // What the tray menu showed along the way.
        let states = states.lock().unwrap();
        assert_eq!(states.first(), Some(&RunState::Waiting));
        assert!(states.iter().any(|s| matches!(s, RunState::Syncing { .. })));
        assert_eq!(states.last(), Some(&RunState::Waiting), "back to waiting after the game closed");
    });
    let _ = fs::remove_dir_all(&tmp);
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
