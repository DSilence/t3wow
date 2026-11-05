//! The whole loop: game SavedVariables -> bridge -> T3 MCP -> snapshot files -> addon.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use t3wow::config::BridgeConfig;
use t3wow::run::{RunOptions, run_bridge};
use t3wow::wow::{INBOX_ADDON, install_addon, installed_live_slots, live_slot_name};

use support::addon_vm::AddonVm;
use support::fake_t3::FakeT3;
use support::wait_for;

fn saved_variables(outbox: &str) -> String {
    format!(
        r#"
T3WowDB = {{
["instanceId"] = "inst-1",
["nextSeq"] = 9,
["outbox"] = {{{outbox}}},
["focus"] = {{
["projectId"] = "p1",
["threadId"] = "t1",
}},
}}
"#
    )
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn bridge_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("t3wow-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let home = tmp.join("home");
    // The only test in this binary, so nothing else reads the environment concurrently.
    unsafe { std::env::set_var("T3WOW_HOME", &home) };
    let flavor = tmp.join("World of Warcraft").join("_retail_");
    let addons = flavor.join("Interface").join("AddOns");
    let sv_file: PathBuf = flavor.join("WTF/Account/TESTACCOUNT/SavedVariables/T3Wow.lua");
    let inbox_file = addons.join(INBOX_ADDON).join("Data.lua");

    let t3 = FakeT3::start();
    {
        // A thread nobody opened: it gets a preview of its last messages only.
        let mut state = t3.state.lock().unwrap();
        let messages: Vec<(&str, String)> = (1..=12).map(|n| ("assistant_message", format!("old note {n}"))).collect();
        let items: Vec<(&str, &str)> = messages.iter().map(|(k, t)| (*k, t.as_str())).collect();
        state.add_thread("tp", "p1", "Preview me", &items);
    }
    install_addon(&flavor, false, 3, false).unwrap();
    fs::create_dir_all(sv_file.parent().unwrap()).unwrap();
    fs::write(
        &sv_file,
        saved_variables(
            r#"
{
["seq"] = 7,
["kind"] = "send",
["threadId"] = "t1",
["text"] = "hello from Azeroth",
["mode"] = "auto",
["model"] = { ["provider"] = "claudeAgent", ["model"] = "claude-haiku-4-5" },
},
{
["seq"] = 8,
["kind"] = "send",
["threadId"] = "missing",
["text"] = "nobody home",
},"#,
        ),
    )
    .unwrap();

    // Installs the addon with its data addons.
    assert_eq!(installed_live_slots(&flavor), 3);
    let toc = read(&addons.join(live_slot_name(1)).join(format!("{}.toc", live_slot_name(1))));
    assert!(toc.contains("## LoadOnDemand: 1"));
    assert!(toc.contains("## Dependencies: T3Wow"));
    assert!(toc.contains("## Interface: 16001"));
    assert!(addons.join("T3Wow/Core.lua").exists(), "the embedded addon is written out");

    let config = BridgeConfig {
        server_url: Some(t3.url.clone()),
        wow_flavor_dirs: vec![flavor.clone()],
        active_poll_ms: 50,
        idle_poll_ms: 100,
        recent_threads_with_messages: 0,
        ..BridgeConfig::default()
    };
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let bridge = scope.spawn(|| {
            let load = || Ok(config.clone());
            run_bridge(&RunOptions { load_config: &load, game_running: None, stop: &stop, report: None })
        });
        let _stop = support::StopOnDrop(&stop);

        // Delivers queued messages once and publishes the reply with acks.
        let inbox = wait_for("an inbox with the acknowledgement", || {
            let text = read(&inbox_file);
            (text.contains("Echo: hello from Azeroth") && text.contains("seq = 8,")).then_some(text)
        });
        // Threads outside the hot set get a short preview.
        assert!(inbox.contains("partial = true"));
        assert!(inbox.contains("old note 12") && inbox.contains("old note 5"));
        assert!(!inbox.contains("old note 4,") && !inbox.contains("\"old note 4\""), "previews keep the last 8");
        // Every live slot carries the same snapshot.
        let live = read(&addons.join(live_slot_name(3)).join("Data.lua"));
        assert_eq!(live.replace("\"live\") end", ""), inbox.replace("\"inbox\") end", ""));

        // Let several more polls run against the unchanged SavedVariables.
        std::thread::sleep(Duration::from_millis(400));
        {
            let state = t3.state.lock().unwrap();
            assert_eq!(state.count("t3_thread_send"), 2, "each command is sent once");
            let first = state.calls.iter().find(|(name, _)| name == "t3_thread_send").unwrap();
            assert_eq!(first.1["clientRequestId"], "t3wow:inst-1:7");
        }
        {
            // The picked model is applied before the message goes out.
            let state = t3.state.lock().unwrap();
            let names: Vec<&str> = state.calls.iter().map(|(name, _)| name.as_str()).collect();
            let configure = names.iter().position(|n| *n == "t3_thread_configure").expect("configure called");
            let send = names.iter().position(|n| *n == "t3_thread_send").unwrap();
            assert!(configure < send);
            assert_eq!(
                state.threads["t1"].model_selection,
                json!({ "instanceId": "claudeAgent", "model": "claude-haiku-4-5" })
            );
        }
        // The catalog and the thread's current model reach the game.
        wait_for("the new model in the snapshot", || read(&inbox_file).contains("claude-haiku-4-5").then_some(()));
        let snapshot = read(&inbox_file);
        assert!(snapshot.contains("defaultEffort = \"medium\""), "catalog published");
        assert!(!snapshot.contains("Cursor"), "disabled providers are left out");

        let progress: Value = serde_json::from_str(&read(&home.join("state.json"))).unwrap();
        assert_eq!(progress["instances"]["inst-1"]["ackSeq"], 8);
        let results: Vec<(i64, bool)> = progress["instances"]["inst-1"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["seq"].as_i64().unwrap(), r["ok"].as_bool().unwrap()))
            .collect();
        assert_eq!(results, [(7, true), (8, false)]);

        // Feeds the game a snapshot the addon understands.
        let vm = AddonVm::load(&read(&sv_file));
        vm.run_addon_file(&read(&inbox_file), "T3Wow_Inbox/Data.lua");
        vm.run(r#"__stub.fire("ADDON_LOADED", "T3Wow"); __stub.fire("PLAYER_LOGIN")"#);
        assert_eq!(vm.number("#T3WowDB.outbox"), 0.0, "acknowledged commands are dropped");
        let prints = vm.string(r#"table.concat(__stub.prints, "\n")"#);
        assert!(prints.contains("Not delivered:") && prints.contains("No such thread"), "{prints}");
        assert_eq!(vm.string("T3Wow.snapshot.bridge.status"), "ok");
        vm.run(r#"T3WowFrame:Show(); T3Wow:Select("p1", "t1")"#);
        let text = vm.string("T3Wow:MessagesText()");
        assert!(text.contains("The login button does nothing"), "{text}");
        assert!(text.contains("Echo: hello from Azeroth"), "{text}");
        assert!(!text.contains("thinking"), "only messages are shown, not reasoning");
        assert_eq!(vm.string("T3Wow.rows[1].title:GetText()"), "Fix the login bug");

        // Picks up new commands when WoW rewrites SavedVariables.
        fs::write(
            &sv_file,
            saved_variables(
                r#"
{
["seq"] = 9,
["kind"] = "launch",
["projectId"] = "p1",
["title"] = "Raid planner",
["text"] = "Build a raid planner",
["model"] = { ["provider"] = "claudeAgent", ["model"] = "claude-opus-5-5", ["effortKey"] = "effort", ["effort"] = "high" },
},"#,
            ),
        )
        .unwrap();
        wait_for("the launched thread in the inbox", || read(&inbox_file).contains("Raid planner").then_some(()));
        let launch = t3.state.lock().unwrap().calls.iter().find(|(name, _)| name == "t3_thread_launch").cloned();
        assert_eq!(
            launch.unwrap().1,
            json!({
                "projectId": "p1",
                "title": "Raid planner",
                "message": "Build a raid planner",
                "modelSelection": {
                    "instanceId": "claudeAgent",
                    "model": "claude-opus-5-5",
                    "options": [{ "id": "effort", "value": "high" }],
                },
            })
        );

        // Sends without a reload: a screenshot carries the command instead.
        let shots_dir = flavor.join("Screenshots");
        let encoder = support::shots::ShotEncoder::new();
        let payload = encoder.json(
            r#"{ v = 1, instanceId = "inst-1", focus = { projectId = "p1", threadId = "t1" },
                 outbox = { { seq = 10, kind = "send", threadId = "t1", text = "sent by screenshot", mode = "auto" } } }"#,
        );
        support::shots::write_screenshots(&shots_dir, &payload, 77);
        fs::write(shots_dir.join("WoWScrnShot_own.jpg"), support::shots::jpeg(&[], 8.0, 90)).unwrap();
        wait_for("the screenshot command", || read(&inbox_file).contains("Echo: sent by screenshot").then_some(()));
        assert!(read(&inbox_file).contains("seq = 10,"), "acknowledged like any command");
        let left: Vec<String> =
            fs::read_dir(&shots_dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["WoWScrnShot_own.jpg"], "data screenshots are deleted, the player's own kept");

        stop.store(true, Ordering::Relaxed);
        bridge.join().unwrap().unwrap();
    });
    let _ = fs::remove_dir_all(&tmp);
}
