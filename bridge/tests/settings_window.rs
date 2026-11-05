//! The commands the settings page sends, without a window: finding the game,
//! ticking game folders, the live update count and automatic updates.
#![cfg(any(target_os = "macos", windows))]

mod support;

use std::fs;
use std::sync::Arc;

use t3wow::config::load_config;
use t3wow::settings::{Settings, TrayInfo};
use t3wow::wow::{installed_live_slots, is_installed};

use support::wait_for;

fn info() -> TrayInfo {
    TrayInfo { status: String::new(), attention: false, update_text: String::new(), update_enabled: true }
}

#[test]
fn changes_the_setup_from_the_page() {
    if t3wow::watch::wow_running() {
        eprintln!("skipped: World of Warcraft is running, so the addon cannot change");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("t3wow-settings-window-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let wow = tmp.join("World of Warcraft");
    let retail = wow.join("_retail_");
    let era = wow.join("_classic_era_");
    fs::create_dir_all(&retail).unwrap();
    fs::create_dir_all(&era).unwrap();
    // The only test in this binary, so nothing else reads the environment concurrently.
    unsafe {
        std::env::set_var("T3WOW_HOME", tmp.join("home"));
        std::env::set_var("WOW_DIR", &wow);
    }
    let mut settings = Settings::new(Arc::new(|| {}), Arc::new(|_| {}));

    // First start (`tray --first-run`): the addon goes into every game folder found.
    settings.look_for_wow(true);
    wait_for("the first install", || (is_installed(&retail) && is_installed(&era)).then_some(()));
    wait_for("the folders in the config", || (load_config().unwrap().wow_flavor_dirs.len() == 2).then_some(()));
    // Like the page: one change at a time.
    let mut send = |message: &str| {
        wait_for("the last change to finish", || (!settings.addon_busy()).then_some(()));
        settings.handle(message, &info());
    };

    // Unticking a folder removes the addon there.
    send(&format!(r#"{{"cmd":"setFlavor","path":{},"on":false}}"#, serde_json::json!(era)));
    wait_for("the addon removed", || (!is_installed(&era)).then_some(()));
    wait_for("the folder forgotten", || (load_config().unwrap().wow_flavor_dirs == vec![retail.clone()]).then_some(()));

    // A new live update count reinstalls where the addon is.
    send(r#"{"cmd":"setSlots","slots":20}"#);
    wait_for("20 live slots", || (installed_live_slots(&retail) == 20).then_some(()));
    wait_for("the count saved", || (load_config().unwrap().live_slots == 20).then_some(()));
    assert!(!is_installed(&era));

    // Out of range: nothing changes.
    send(r#"{"cmd":"setSlots","slots":5000}"#);
    send(r#"{"cmd":"setSlots","slots":-1}"#);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(load_config().unwrap().live_slots, 20);
    assert_eq!(installed_live_slots(&retail), 20);

    send(r#"{"cmd":"setAutoUpdate","on":false}"#);
    assert!(!load_config().unwrap().auto_update);

    let _ = fs::remove_dir_all(&tmp);
}
