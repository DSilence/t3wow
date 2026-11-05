use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::addon_state::{AddonState, Command, OutboxCommand};
use crate::config::{home_dir, read_json_file, write_json_file};
use crate::error::{Error, Result};
use crate::t3::{SearchMatch, T3Client};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub seq: i64,
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceProgress {
    /// Highest outbox seq the bridge has finished with (delivered or permanently failed).
    pub ack_seq: i64,
    pub results: Vec<CommandResult>,
    /// The latest search from this addon install.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchResults>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResults {
    pub seq: i64,
    pub query: String,
    pub matches: Vec<SearchMatch>,
    /// Why the search failed, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    instances: BTreeMap<String, InstanceProgress>,
}

const MAX_RESULTS: usize = 20;
const MAX_SEARCH_MATCHES: usize = 30;

fn state_path() -> PathBuf {
    home_dir().join("state.json")
}

/// Delivers queued in-game commands exactly once per addon install. The addon
/// drops outbox entries once the snapshot acknowledges them; until then the
/// bridge sees them on every read, so progress is persisted here.
pub struct OutboxProcessor {
    progress: BTreeMap<String, InstanceProgress>,
}

impl OutboxProcessor {
    pub fn load() -> Result<Self> {
        let state: Option<StateFile> = read_json_file(&state_path())?;
        Ok(Self { progress: state.unwrap_or_default().instances })
    }

    pub fn acks(&self) -> &BTreeMap<String, InstanceProgress> {
        &self.progress
    }

    pub fn has_pending(&self, states: &[AddonState]) -> bool {
        states.iter().any(|state| {
            let acked = self.progress.get(&state.instance_id).map_or(0, |p| p.ack_seq);
            state.outbox.iter().any(|c| c.seq > acked)
        })
    }

    /// Returns the ids of threads that commands touched.
    pub fn process(&mut self, client: &mut T3Client, states: &[AddonState]) -> Result<Vec<String>> {
        let mut touched = Vec::new();
        for state in states {
            for command in &state.outbox {
                let acked = self.progress.get(&state.instance_id).map_or(0, |p| p.ack_seq);
                if command.seq <= acked {
                    continue;
                }
                if let Command::Search { query } = &command.command {
                    let search = search(client, command.seq, query)?;
                    let result = CommandResult {
                        seq: command.seq,
                        ok: true,
                        message: format!("{} matches", search.matches.len()),
                        thread_id: None,
                    };
                    crate::log!("search #{}: {}", command.seq, result.message);
                    let progress = self.progress.entry(state.instance_id.clone()).or_default();
                    progress.search = Some(search);
                    self.finish(&state.instance_id, result)?;
                    continue;
                }
                let result = match execute(client, &state.instance_id, command) {
                    Ok(result) => result,
                    Err(Error::Tool { message, .. }) => {
                        CommandResult { seq: command.seq, ok: false, message, thread_id: None }
                    }
                    // Transient: retry on the next tick.
                    Err(error) => return Err(error),
                };
                crate::log!(
                    "{} #{}: {} - {}",
                    kind(&command.command),
                    command.seq,
                    if result.ok { "ok" } else { "failed" },
                    result.message
                );
                if let Some(id) = &result.thread_id {
                    touched.push(id.clone());
                }
                self.finish(&state.instance_id, result)?;
            }
        }
        Ok(touched)
    }

    /// Record a command as done and persist the progress.
    fn finish(&mut self, instance_id: &str, result: CommandResult) -> Result<()> {
        let progress = self.progress.entry(instance_id.to_string()).or_default();
        progress.ack_seq = result.seq;
        progress.results.push(result);
        let excess = progress.results.len().saturating_sub(MAX_RESULTS);
        progress.results.drain(..excess);
        let file = StateFile { instances: self.progress.clone() };
        write_json_file(&state_path(), &file, false)?;
        Ok(())
    }
}

/// A failed search is shown with its results instead of as an undelivered command.
fn search(client: &mut T3Client, seq: i64, query: &str) -> Result<SearchResults> {
    let mut results = SearchResults { seq, query: query.to_string(), ..Default::default() };
    if !client.can_search() {
        results.error = Some("This T3 Code environment cannot search threads".into());
        return Ok(results);
    }
    match client.search_threads(query, MAX_SEARCH_MATCHES) {
        Ok(matches) => results.matches = matches,
        Err(Error::Tool { message, .. }) => results.error = Some(message),
        Err(error) => return Err(error),
    }
    Ok(results)
}

fn kind(command: &Command) -> &'static str {
    match command {
        Command::Send { .. } => "send",
        Command::Launch { .. } => "launch",
        Command::Interrupt { .. } => "interrupt",
        Command::Settle { .. } => "settle",
        Command::Answer { .. } => "answer",
        Command::Search { .. } => "search",
    }
}

fn execute(client: &mut T3Client, instance_id: &str, command: &OutboxCommand) -> Result<CommandResult> {
    // Stable per command, so a retry after a lost response is not delivered twice.
    let request_id = format!("t3wow:{instance_id}:{}", command.seq);
    let done = |message: &str, thread_id: &str| CommandResult {
        seq: command.seq,
        ok: true,
        message: message.to_string(),
        thread_id: Some(thread_id.to_string()),
    };
    Ok(match &command.command {
        Command::Send { thread_id, text, mode, model } => {
            if let Some(model) = model {
                client.configure_thread(thread_id, model)?;
            }
            let delivery = client.send_to_thread(thread_id, text, mode, &request_id)?;
            done(&describe_delivery(&delivery), thread_id)
        }
        Command::Launch { project_id, title, text, model } => {
            let thread_id = client.launch_thread(project_id, title, text, model.as_ref())?;
            done("Thread started", &thread_id)
        }
        Command::Interrupt { thread_id } => {
            let status = client.interrupt_thread(thread_id, &request_id)?;
            done(if status == "interrupt_requested" { "Stopping" } else { "Nothing to stop" }, thread_id)
        }
        Command::Settle { thread_id, settled } => {
            client.settle_thread(thread_id, *settled)?;
            done(if *settled { "Settled" } else { "Back in the thread list" }, thread_id)
        }
        Command::Answer { thread_id, request_id: question_id, answers } => {
            client.answer_question(thread_id, question_id, answers)?;
            done("Answer sent", thread_id)
        }
        Command::Search { .. } => unreachable!("searches are handled in process"),
    })
}

fn describe_delivery(delivery: &str) -> String {
    match delivery {
        "started" => "Sent",
        "queued" => "Queued after the current turn",
        "steered" => "Sent to the running turn",
        "restarted" => "Restarted the turn",
        other => other,
    }
    .to_string()
}
