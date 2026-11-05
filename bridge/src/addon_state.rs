use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::error::Result;
use crate::lua::parse_saved_variables;
use crate::t3::ModelChoice;

/// A command the player queued in game. Mirrors `T3WowDB.outbox` entries in Core.lua.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `model` switches the thread's model before the message is sent.
    Send {
        thread_id: String,
        text: String,
        mode: String,
        model: Option<ModelChoice>,
    },
    Launch {
        project_id: String,
        title: String,
        text: String,
        model: Option<ModelChoice>,
    },
    Interrupt {
        thread_id: String,
    },
    /// Settle a thread (`settled = false` brings it back to the active list).
    Settle {
        thread_id: String,
        settled: bool,
    },
    /// Answers keyed by question id: text, or a list of texts for a multi-select question.
    Answer {
        thread_id: String,
        request_id: String,
        answers: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxCommand {
    pub seq: i64,
    pub command: Command,
}

/// What the bridge reads from one account's `T3WowDB` SavedVariables.
#[derive(Debug, Clone)]
pub struct AddonState {
    pub file: PathBuf,
    pub instance_id: String,
    pub outbox: Vec<OutboxCommand>,
    pub focus_project_id: Option<String>,
    pub focus_thread_id: Option<String>,
    /// Threads opened in game recently, newest first.
    pub watched_thread_ids: Vec<String>,
    /// When the game wrote this (file time, or when a screenshot arrived).
    pub written: SystemTime,
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn list_of(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    }
}

/// `{ provider = "claudeAgent", model = "claude-opus-5-5", effortKey = "effort", effort = "high" }`
fn parse_model(raw: &Value) -> Option<ModelChoice> {
    let effort = match (str_of(&raw["effortKey"]), str_of(&raw["effort"])) {
        (Some(key), Some(value)) => Some((key, value)),
        _ => None,
    };
    Some(ModelChoice { provider: str_of(&raw["provider"])?, model: str_of(&raw["model"])?, effort })
}

/// Text, or a list of texts (Lua arrays arrive as JSON arrays).
fn parse_answer(raw: &Value) -> Option<Value> {
    match raw {
        Value::String(_) => Some(raw.clone()),
        Value::Array(items) => Some(Value::Array(items.iter().filter(|v| v.is_string()).cloned().collect())),
        _ => None,
    }
}

fn parse_command(raw: &Value) -> Option<OutboxCommand> {
    let seq = raw["seq"].as_f64()? as i64;
    let command = match raw["kind"].as_str()? {
        "send" => Command::Send {
            thread_id: str_of(&raw["threadId"])?,
            text: str_of(&raw["text"])?,
            mode: str_of(&raw["mode"]).unwrap_or_else(|| "auto".into()),
            model: parse_model(&raw["model"]),
        },
        "launch" => {
            let text = str_of(&raw["text"])?;
            let title = str_of(&raw["title"])
                .unwrap_or_else(|| text.lines().next().unwrap_or_default().chars().take(80).collect());
            Command::Launch { project_id: str_of(&raw["projectId"])?, title, text, model: parse_model(&raw["model"]) }
        }
        "interrupt" => Command::Interrupt { thread_id: str_of(&raw["threadId"])? },
        "settle" => {
            Command::Settle { thread_id: str_of(&raw["threadId"])?, settled: raw["settled"].as_bool().unwrap_or(true) }
        }
        "answer" => {
            let answers = raw["answers"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| parse_answer(v).map(|v| (k.clone(), v)))
                .collect();
            Command::Answer { thread_id: str_of(&raw["threadId"])?, request_id: str_of(&raw["requestId"])?, answers }
        }
        _ => return None,
    };
    Some(OutboxCommand { seq, command })
}

pub fn parse_addon_state(file: &Path, source: &[u8], written: SystemTime) -> Result<Option<AddonState>> {
    let vars = parse_saved_variables(source)?;
    Ok(vars.get("T3WowDB").and_then(|db| state_from_db(file, db, written)))
}

/// The bridge's view of a `T3WowDB` table, from SavedVariables or a screenshot.
pub fn state_from_db(file: &Path, db: &Value, written: SystemTime) -> Option<AddonState> {
    if !db.is_object() {
        return None;
    }
    let instance_id = str_of(&db["instanceId"])?;
    let mut outbox: Vec<OutboxCommand> = list_of(&db["outbox"]).into_iter().filter_map(parse_command).collect();
    outbox.sort_by_key(|c| c.seq);
    let mut watched: Vec<(&String, f64)> =
        db["watch"].as_object().into_iter().flatten().filter_map(|(id, at)| at.as_f64().map(|at| (id, at))).collect();
    watched.sort_by(|a, b| b.1.total_cmp(&a.1));
    Some(AddonState {
        file: file.to_path_buf(),
        instance_id,
        outbox,
        focus_project_id: str_of(&db["focus"]["projectId"]),
        focus_thread_id: str_of(&db["focus"]["threadId"]),
        watched_thread_ids: watched.into_iter().map(|(id, _)| id.clone()).collect(),
        written,
    })
}

/// Combine SavedVariables with states that arrived later by screenshot. A
/// newer SavedVariables file already holds everything a screenshot carried.
pub fn merge_states(saved: Vec<AddonState>, shots: &[AddonState]) -> Vec<AddonState> {
    let mut states = saved;
    for shot in shots {
        match states.iter_mut().find(|s| s.instance_id == shot.instance_id) {
            Some(state) if state.written >= shot.written => {}
            Some(state) => {
                let mut outbox = shot.outbox.clone();
                for command in &state.outbox {
                    if !outbox.iter().any(|c| c.seq == command.seq) {
                        outbox.push(command.clone());
                    }
                }
                outbox.sort_by_key(|c| c.seq);
                *state = AddonState { file: state.file.clone(), outbox, ..shot.clone() };
            }
            None => states.push(shot.clone()),
        }
    }
    states
}

/// Re-reads SavedVariables files only when WoW has rewritten them.
#[derive(Default)]
pub struct AddonStateReader {
    cache: HashMap<PathBuf, (SystemTime, u64, Option<AddonState>)>,
}

impl AddonStateReader {
    /// Returns current states and whether any file changed since the last call.
    pub fn read(&mut self, files: &[PathBuf]) -> (Vec<AddonState>, bool) {
        let mut changed = false;
        let mut states = Vec::new();
        for file in files {
            let Ok(meta) = fs::metadata(file) else { continue };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let fresh = self.cache.get(file).is_some_and(|(m, size, _)| *m == mtime && *size == meta.len());
            if !fresh {
                let parsed =
                    fs::read(file).map_err(Into::into).and_then(|bytes| parse_addon_state(file, &bytes, mtime));
                match parsed {
                    Ok(state) => {
                        self.cache.insert(file.clone(), (mtime, meta.len(), state));
                        changed = true;
                    }
                    Err(error) => {
                        // WoW may be mid-write; try again on the next poll.
                        crate::log!("could not parse {}: {error}", file.display());
                        continue;
                    }
                }
            }
            if let Some((_, _, Some(state))) = self.cache.get(file) {
                states.push(state.clone());
            }
        }
        (states, changed)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_the_outbox() {
        let source = br#"
T3WowDB = {
["instanceId"] = "inst-1",
["outbox"] = {
{ ["seq"] = 9, ["kind"] = "launch", ["projectId"] = "p1", ["text"] = "Build a raid planner\nwith calendars",
  ["model"] = { ["provider"] = "codex", ["model"] = "gpt-6-sol", ["effortKey"] = "reasoningEffort", ["effort"] = "high" } },
{ ["seq"] = 7, ["kind"] = "send", ["threadId"] = "t1", ["text"] = "hi" },
{ ["seq"] = 8, ["kind"] = "send", ["threadId"] = "t1" },
{ ["seq"] = 11, ["kind"] = "settle", ["threadId"] = "t1", ["settled"] = false },
{ ["seq"] = 10, ["kind"] = "answer", ["threadId"] = "t1", ["requestId"] = "r1",
  ["answers"] = { ["q1"] = "Yes", ["q2"] = { "a", "b" }, ["q3"] = 5 } },
},
["focus"] = { ["projectId"] = "p1" },
["watch"] = { ["old"] = 100, ["new"] = 200 },
}
"#;
        let state = parse_addon_state(Path::new("x"), source, SystemTime::UNIX_EPOCH).unwrap().unwrap();
        assert_eq!(state.instance_id, "inst-1");
        assert_eq!(
            state.outbox.iter().map(|c| c.seq).collect::<Vec<_>>(),
            [7, 9, 10, 11],
            "invalid commands are dropped"
        );
        assert_eq!(state.outbox[3].command, Command::Settle { thread_id: "t1".into(), settled: false });
        assert_eq!(
            state.outbox[0].command,
            Command::Send { thread_id: "t1".into(), text: "hi".into(), mode: "auto".into(), model: None }
        );
        let Command::Launch { title, model, .. } = &state.outbox[1].command else { panic!("not a launch") };
        assert_eq!(title, "Build a raid planner");
        assert_eq!(
            model.as_ref().map(|m| (m.provider.as_str(), m.model.as_str(), m.effort.clone())),
            Some(("codex", "gpt-6-sol", Some(("reasoningEffort".into(), "high".into()))))
        );
        let Command::Answer { answers, .. } = &state.outbox[2].command else { panic!("not an answer") };
        assert_eq!(
            answers,
            &BTreeMap::from([("q1".into(), json!("Yes")), ("q2".into(), json!(["a", "b"]))]),
            "multi-select answers are lists; other values are dropped"
        );
        assert_eq!(state.focus_project_id.as_deref(), Some("p1"));
        assert_eq!(state.focus_thread_id, None);
        assert_eq!(state.watched_thread_ids, ["new", "old"]);
    }

    #[test]
    fn merges_screenshots_with_saved_variables() {
        let t = |secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let db = |seqs: &[i64], thread: &str| {
            let outbox: Vec<Value> = seqs
                .iter()
                .map(|seq| serde_json::json!({ "seq": seq, "kind": "interrupt", "threadId": "t" }))
                .collect();
            serde_json::json!({ "instanceId": "i", "outbox": outbox, "focus": { "threadId": thread } })
        };
        let saved = state_from_db(Path::new("sv"), &db(&[1], "old"), t(10)).unwrap();
        let shot = state_from_db(Path::new("shot"), &db(&[2], "new"), t(20)).unwrap();
        let merged = merge_states(vec![saved.clone()], std::slice::from_ref(&shot));
        assert_eq!(merged[0].outbox.iter().map(|c| c.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(merged[0].focus_thread_id.as_deref(), Some("new"));
        assert_eq!(merged[0].file, Path::new("sv"));
        // SavedVariables written after the screenshot win.
        let later = AddonState { written: t(30), ..saved };
        assert_eq!(merge_states(vec![later], &[shot])[0].focus_thread_id.as_deref(), Some("old"));
    }

    #[test]
    fn ignores_files_without_an_instance() {
        assert!(parse_addon_state(Path::new("x"), b"T3WowDB = {}", SystemTime::UNIX_EPOCH).unwrap().is_none());
        assert!(parse_addon_state(Path::new("x"), b"Other = 1", SystemTime::UNIX_EPOCH).unwrap().is_none());
    }
}
