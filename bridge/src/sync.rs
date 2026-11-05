//! Pulls what the addon shows from T3 and keeps it cached between polls.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use serde_json::{Map, Value, json};

use crate::addon_state::AddonState;
use crate::config::BridgeConfig;
use crate::error::Error;
use crate::error::Result;
use crate::outbox::InstanceProgress;
use crate::t3::{
    CatalogProvider, ModelChoice, PendingRequest, Project, ReadOptions, T3Client, ThreadDetail, ThreadListItem,
    TimelineItem, is_active,
};

const MESSAGE_TYPES: &[&str] = &["user_message", "assistant_message", "proposed_plan"];
const SETTLED_ITEM_STATUSES: &[&str] = &["completed", "failed", "cancelled", "interrupted"];
const PROJECTS_REFRESH_SECS: u64 = 60;
const FULL_LIST_REFRESH_SECS: u64 = 10;
/// Timeline positions scanned first when catching up on a thread (grows until enough messages).
const INITIAL_WINDOW: i64 = 200;
const MAX_HOT_THREADS: usize = 12;
const CATALOG_REFRESH_SECS: u64 = 300;
/// Settled threads rarely change, so their lists refresh less often (and right after a settle).
const SETTLED_REFRESH_SECS: u64 = 30;
const SETTLED_PER_PROJECT: usize = 15;
/// Threads outside the hot set get a short preview, so opening them in game
/// shows something without a reload to tell the bridge.
const MAX_PREVIEWS: usize = 16;
const PREVIEW_MESSAGES: usize = 8;
const PREVIEW_CHARS: usize = 1_500;
/// Timeline positions scanned for a preview (one page).
const PREVIEW_WINDOW: i64 = 60;

fn is_settled(item: &TimelineItem) -> bool {
    SETTLED_ITEM_STATUSES.contains(&item.status.as_str())
}

#[derive(Default)]
struct ThreadCache {
    detail: Option<ThreadDetail>,
    /// Message items by timeline position, trimmed to the newest few.
    items: BTreeMap<i64, TimelineItem>,
    /// Highest timeline position read so far.
    last_position: i64,
    /// True when older messages exist than the ones kept.
    has_older: bool,
    /// `updatedAt` of the list entry when this cache was last refreshed.
    synced_updated_at: Option<String>,
    activity: Option<String>,
    questions: Vec<PendingRequest>,
    /// The thread's current model, shown and preselected in game.
    model: Option<ModelChoice>,
}

/// The last few messages of a thread nobody has opened recently.
struct Preview {
    updated_at: String,
    detail: ThreadDetail,
    items: Vec<TimelineItem>,
    has_older: bool,
}

#[derive(Debug, Clone)]
pub struct BridgeStatus {
    /// `ok`, `connecting`, `auth_required` or `error`.
    pub status: &'static str,
    pub message: Option<String>,
    pub server: Option<String>,
    pub can_operate: bool,
}

/// Seconds since the epoch for an ISO 8601 UTC timestamp (`2026-10-16T13:45:14.123Z`).
pub fn unix_seconds(iso: &str) -> i64 {
    let parse = || -> Option<i64> {
        let num = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
        let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
        // Days from civil (Howard Hinnant's algorithm).
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let mut seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
        // Offsets like +02:00 after the seconds and optional fraction.
        let rest = iso.get(19..).unwrap_or("");
        let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        if let Some(sign @ ('+' | '-')) = rest.chars().next() {
            let offset = rest.get(1..3)?.parse::<i64>().ok()? * 3600 + rest.get(4..6)?.parse::<i64>().ok()? * 60;
            seconds -= if sign == '+' { offset } else { -offset };
        }
        Some(seconds)
    };
    parse().unwrap_or(0)
}

pub struct SyncEngine {
    config: BridgeConfig,
    projects: Vec<Project>,
    projects_at: Option<Instant>,
    lists: HashMap<String, Vec<ThreadListItem>>,
    lists_at: Option<Instant>,
    /// Settled threads per project, shown in their own section in game.
    settled: HashMap<String, Vec<ThreadListItem>>,
    settled_at: Option<Instant>,
    can_settle: bool,
    threads: HashMap<String, ThreadCache>,
    previews: HashMap<String, Preview>,
    catalog: Vec<CatalogProvider>,
    catalog_at: Option<Instant>,
    /// Threads that must be refreshed on the next tick regardless of list state.
    forced: Vec<String>,
    focus_projects: HashSet<String>,
}

impl SyncEngine {
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            config,
            projects: Vec::new(),
            projects_at: None,
            lists: HashMap::new(),
            lists_at: None,
            settled: HashMap::new(),
            settled_at: None,
            can_settle: false,
            threads: HashMap::new(),
            previews: HashMap::new(),
            catalog: Vec::new(),
            catalog_at: None,
            forced: Vec::new(),
            focus_projects: HashSet::new(),
        }
    }

    /// True while something visible is running, so the caller should poll fast.
    pub fn active(&self) -> bool {
        self.lists.values().flatten().any(|t| is_active(&t.status))
    }

    /// Listed threads that are running right now.
    pub fn running_count(&self) -> usize {
        self.lists.values().flatten().filter(|t| is_active(&t.status)).count()
    }

    pub fn touch(&mut self, thread_ids: impl IntoIterator<Item = String>) {
        for id in thread_ids {
            if !self.forced.contains(&id) {
                self.forced.push(id);
            }
        }
    }

    pub fn tick(&mut self, client: &mut T3Client, states: &[AddonState]) -> Result<()> {
        let stale = |at: Option<Instant>, secs: u64| at.is_none_or(|at| at.elapsed().as_secs() >= secs);
        if stale(self.projects_at, PROJECTS_REFRESH_SECS) || self.projects.is_empty() {
            self.projects = client.list_projects()?;
            self.projects_at = Some(Instant::now());
        }
        if stale(self.catalog_at, CATALOG_REFRESH_SECS) {
            self.catalog = match client.model_catalog() {
                Ok(catalog) => catalog,
                // Older environments or narrower grants: no model picker in game.
                Err(Error::Tool { .. }) => Vec::new(),
                Err(error) => return Err(error),
            };
            self.catalog_at = Some(Instant::now());
        }

        self.focus_projects = states.iter().filter_map(|s| s.focus_project_id.clone()).collect();
        let projects = self.visible_projects();
        let full_refresh = stale(self.lists_at, FULL_LIST_REFRESH_SECS) || !self.forced.is_empty();
        for project in &projects {
            if let Some(list) = self.lists.get(&project.id) {
                let busy = list.iter().any(|t| is_active(&t.status));
                if !full_refresh && !busy && !self.focus_projects.contains(&project.id) {
                    continue;
                }
            }
            let threads = client.list_threads(&project.id, self.config.threads_per_project, false)?;
            self.lists.insert(project.id.clone(), threads);
        }
        if full_refresh {
            self.lists_at = Some(Instant::now());
        }
        self.lists.retain(|id, _| projects.iter().any(|p| p.id == *id));
        self.can_settle = client.can_settle();
        let settled_due = stale(self.settled_at, SETTLED_REFRESH_SECS) || !self.forced.is_empty();
        for project in &projects {
            if settled_due || !self.settled.contains_key(&project.id) {
                let threads = client.list_threads(&project.id, SETTLED_PER_PROJECT, true)?;
                self.settled.insert(project.id.clone(), threads);
            }
        }
        if settled_due {
            self.settled_at = Some(Instant::now());
        }
        self.settled.retain(|id, _| projects.iter().any(|p| p.id == *id));

        let listed: HashMap<String, ThreadListItem> =
            self.lists.values().flatten().map(|t| (t.thread_id.clone(), t.clone())).collect();
        let hot = self.hot_thread_ids(states, &listed);
        for thread_id in &hot {
            // A settled thread opened in game is followed like any other.
            let item = listed.get(thread_id).or_else(|| self.settled_item(thread_id)).cloned();
            self.refresh_thread(client, thread_id, item.as_ref())?;
        }
        self.threads.retain(|id, _| hot.contains(id));
        let previews = self.preview_thread_ids(&hot);
        for thread_id in &previews {
            if let Some(item) = listed.get(thread_id) {
                self.refresh_preview(client, item)?;
            }
        }
        self.previews.retain(|id, _| previews.contains(id));
        self.forced.clear();
        Ok(())
    }

    fn settled_item(&self, thread_id: &str) -> Option<&ThreadListItem> {
        self.settled.values().flatten().find(|t| t.thread_id == thread_id)
    }

    /// Listed threads that are not hot: the focused projects' first, then the most recent.
    fn preview_thread_ids(&self, hot: &[String]) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut add = |t: &ThreadListItem| {
            if ids.len() < MAX_PREVIEWS && !hot.contains(&t.thread_id) && !ids.contains(&t.thread_id) {
                ids.push(t.thread_id.clone());
            }
        };
        for project in &self.focus_projects {
            for t in self.lists.get(project).into_iter().flatten() {
                add(t);
            }
        }
        let mut by_recent: Vec<&ThreadListItem> = self.lists.values().flatten().collect();
        by_recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.thread_id.cmp(&b.thread_id)));
        for t in by_recent {
            add(t);
        }
        ids
    }

    fn refresh_preview(&mut self, client: &mut T3Client, item: &ThreadListItem) -> Result<()> {
        if self.previews.get(&item.thread_id).is_some_and(|p| p.updated_at == item.updated_at) {
            return Ok(());
        }
        let from = (item.item_count - PREVIEW_WINDOW).max(-1);
        let options = ReadOptions {
            after_position: Some(from),
            limit: Some(PREVIEW_WINDOW + 1),
            max_chars_per_item: Some(PREVIEW_CHARS),
            ..Default::default()
        };
        let result = client.read_thread(&item.thread_id, options)?;
        let mut items: Vec<TimelineItem> =
            result.items.into_iter().filter(|i| MESSAGE_TYPES.contains(&i.kind.as_str())).collect();
        let has_older = from > -1 || items.len() > PREVIEW_MESSAGES;
        items.drain(..items.len().saturating_sub(PREVIEW_MESSAGES));
        let preview = Preview { updated_at: item.updated_at.clone(), detail: result.thread, items, has_older };
        self.previews.insert(item.thread_id.clone(), preview);
        Ok(())
    }

    fn visible_projects(&self) -> Vec<Project> {
        let mut sorted = self.projects.clone();
        sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let (mut picked, rest): (Vec<_>, Vec<_>) =
            sorted.into_iter().partition(|p| self.focus_projects.contains(&p.id));
        for project in rest {
            if picked.len() >= self.config.max_projects {
                break;
            }
            picked.push(project);
        }
        picked
    }

    fn hot_thread_ids(&self, states: &[AddonState], listed: &HashMap<String, ThreadListItem>) -> Vec<String> {
        let mut hot: Vec<String> = Vec::new();
        let mut add = |id: Option<&String>| {
            if let Some(id) = id
                && !hot.contains(id)
                && hot.len() < MAX_HOT_THREADS
            {
                hot.push(id.clone());
            }
        };
        for id in &self.forced {
            add(Some(id));
        }
        for state in states {
            add(state.focus_thread_id.as_ref());
        }
        let mut by_recent: Vec<&ThreadListItem> = listed.values().collect();
        by_recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.thread_id.cmp(&b.thread_id)));
        for t in &by_recent {
            if is_active(&t.status) {
                add(Some(&t.thread_id));
            }
        }
        for state in states {
            for id in state.watched_thread_ids.iter().take(6) {
                add(Some(id));
            }
        }
        for t in by_recent.iter().take(self.config.recent_threads_with_messages) {
            add(Some(&t.thread_id));
        }
        hot
    }

    fn refresh_thread(
        &mut self,
        client: &mut T3Client,
        thread_id: &str,
        list_item: Option<&ThreadListItem>,
    ) -> Result<()> {
        let active = list_item.is_some_and(|t| is_active(&t.status));
        let forced = self.forced.iter().any(|id| id == thread_id);
        if let Some(cache) = self.threads.get(thread_id) {
            let live_item = cache.items.values().any(|i| !is_settled(i));
            let unchanged = list_item.is_none_or(|t| Some(&t.updated_at) == cache.synced_updated_at.as_ref());
            if !forced && !active && !live_item && unchanged {
                return Ok(());
            }
        }

        let max_chars = self.config.max_chars_per_message;
        let keep = self.config.messages_per_thread;
        let changed = forced
            || self
                .threads
                .get(thread_id)
                .is_none_or(|c| list_item.is_some_and(|t| Some(&t.updated_at) != c.synced_updated_at.as_ref()));
        let mut cache = match self.threads.remove(thread_id) {
            Some(mut cache) => {
                // Re-read from the oldest item still being written, else from the last one seen.
                let from = cache
                    .items
                    .values()
                    .filter(|i| !is_settled(i))
                    .map(|i| i.position - 1)
                    .min()
                    .unwrap_or(cache.last_position);
                for item in read_messages(client, thread_id, from, max_chars, &mut cache)? {
                    cache.items.insert(item.position, item);
                }
                cache
            }
            None => {
                let mut cache = ThreadCache { last_position: -1, ..Default::default() };
                // Catch up from the end of the timeline, widening until enough messages are found.
                let total = match list_item {
                    Some(t) => t.item_count,
                    None => {
                        let options = ReadOptions { limit: Some(1), max_chars_per_item: Some(1), ..Default::default() };
                        client.read_thread(thread_id, options)?.thread.item_count
                    }
                };
                let mut window = INITIAL_WINDOW;
                loop {
                    let from = (total - window).max(-1);
                    let items = read_messages(client, thread_id, from, max_chars, &mut cache)?;
                    if items.len() >= keep || from == -1 {
                        cache.has_older = from > -1 || items.len() > keep;
                        for item in items {
                            cache.items.insert(item.position, item);
                        }
                        break;
                    }
                    window *= 4;
                }
                cache
            }
        };
        while cache.items.len() > keep {
            cache.items.pop_first();
            cache.has_older = true;
        }
        if let Some((&last, _)) = cache.items.last_key_value() {
            cache.last_position = cache.last_position.max(last);
        }

        let pending = cache.detail.as_ref().map_or(0, |d| d.pending_request_count);
        cache.questions = if pending > 0 { client.pending_questions(thread_id)? } else { Vec::new() };
        let status = cache.detail.as_ref().map(|d| d.status.clone()).or_else(|| list_item.map(|t| t.status.clone()));
        cache.activity = match status {
            Some(_) if !cache.questions.is_empty() => Some(WAITING_FOR_ANSWER.to_string()),
            Some(s) if is_active(&s) => latest_activity(client, thread_id, &cache)?,
            _ => None,
        };
        if changed || cache.model.is_none() {
            cache.model = match client.thread_model(thread_id) {
                Ok(model) => model,
                Err(Error::Tool { .. }) => None,
                Err(error) => return Err(error),
            };
        }
        cache.synced_updated_at =
            list_item.map(|t| t.updated_at.clone()).or_else(|| cache.detail.as_ref().map(|d| d.updated_at.clone()));
        self.threads.insert(thread_id.to_string(), cache);
        Ok(())
    }

    pub fn snapshot(&self, bridge: &BridgeStatus, acks: &BTreeMap<String, InstanceProgress>) -> Value {
        let projects: Vec<Value> = self
            .visible_projects()
            .iter()
            .filter_map(|p| {
                let list = self.lists.get(&p.id)?;
                let threads: Vec<Value> = list.iter().map(list_item_json).collect();
                let settled: Vec<Value> = self.settled.get(&p.id).into_iter().flatten().map(list_item_json).collect();
                let default_model =
                    p.default_model_selection.as_ref().map(|m| json!({ "provider": m.instance_id, "model": m.model }));
                Some(json!({
                    "id": p.id,
                    "title": p.title,
                    "root": p.workspace_root,
                    "defaultModel": default_model,
                    "threads": threads,
                    "settled": settled,
                }))
            })
            .collect();

        let mut threads = Map::new();
        for (id, cache) in &self.threads {
            let d = cache.detail.as_ref();
            let messages: Vec<Value> = cache.items.values().map(message_json).collect();
            let questions: Vec<Value> = cache
                .questions
                .iter()
                .flat_map(|request| {
                    request.questions.iter().map(|q| {
                        let options: Vec<Value> = q
                            .options
                            .iter()
                            .map(|o| {
                                json!({
                                    "label": o.label,
                                    "value": o.value.as_deref().unwrap_or(&o.label),
                                    "description": o.description,
                                })
                            })
                            .collect();
                        json!({
                            "requestId": request.request_id,
                            "id": q.id,
                            "header": q.header,
                            "question": q.question,
                            "multi": q.multi_select.unwrap_or(false),
                            "custom": q.allow_custom_answer.unwrap_or(true),
                            "options": options,
                        })
                    })
                })
                .collect();
            threads.insert(
                id.clone(),
                json!({
                    "id": id,
                    "projectId": d.and_then(|d| d.project_id.clone()),
                    "title": d.and_then(|d| d.title.clone()),
                    "status": d.map_or("idle", |d| d.status.as_str()),
                    "model": d.and_then(|d| d.model.clone()),
                    "branch": d.and_then(|d| d.branch.clone()),
                    "activity": cache.activity,
                    "pending": d.map_or(0, |d| d.pending_request_count),
                    "more": cache.has_older,
                    "messages": messages,
                    "questions": questions,
                    "selection": cache.model.as_ref().map(choice_json),
                }),
            );
        }
        for (id, preview) in &self.previews {
            if threads.contains_key(id) {
                continue;
            }
            let d = &preview.detail;
            threads.insert(
                id.clone(),
                json!({
                    "id": id,
                    "projectId": d.project_id,
                    "title": d.title,
                    "status": d.status,
                    "model": d.model,
                    "branch": d.branch,
                    "pending": d.pending_request_count,
                    "more": preview.has_older,
                    "partial": true,
                    "messages": preview.items.iter().map(message_json).collect::<Vec<_>>(),
                }),
            );
        }

        let mut ack_table = Map::new();
        for (instance_id, progress) in acks {
            let results: Vec<Value> = progress
                .results
                .iter()
                .map(|r| json!({ "seq": r.seq, "ok": r.ok, "message": r.message, "threadId": r.thread_id }))
                .collect();
            ack_table.insert(instance_id.clone(), json!({ "seq": progress.ack_seq, "results": results }));
        }

        json!({
            "bridge": {
                "status": bridge.status,
                "message": bridge.message,
                "server": bridge.server,
                "canOperate": bridge.can_operate,
                "canSettle": bridge.can_operate && self.can_settle,
                // Reads commands from screenshots (addon/T3Wow/FastSend.lua).
                "shots": true,
                "active": self.active(),
            },
            "acks": ack_table,
            "projects": projects,
            "threads": threads,
            "models": self.catalog.iter().map(catalog_json).collect::<Vec<_>>(),
        })
    }
}

fn list_item_json(t: &ThreadListItem) -> Value {
    let mut item = json!({
        "id": t.thread_id,
        "title": t.title,
        "status": t.status,
        "updatedAt": unix_seconds(&t.updated_at),
        "model": t.model,
        "provider": t.provider_instance_id,
        "mode": t.runtime_mode,
        "parentId": t.parent_thread_id,
    });
    if let Some(at) = t.settled_at.as_deref().filter(|_| t.settled) {
        item["settledAt"] = json!(unix_seconds(at));
    }
    item
}

fn message_json(item: &TimelineItem) -> Value {
    let role = match item.kind.as_str() {
        "user_message" => "user",
        "proposed_plan" => "plan",
        _ => "assistant",
    };
    json!({
        "role": role,
        "text": item.text.as_deref().unwrap_or(""),
        "status": item.status,
        "at": unix_seconds(&item.updated_at),
        "cut": item.text_truncated,
    })
}

fn choice_json(choice: &ModelChoice) -> Value {
    let (key, effort) = choice.effort.clone().unzip();
    json!({ "provider": choice.provider, "model": choice.model, "effortKey": key, "effort": effort })
}

fn catalog_json(provider: &CatalogProvider) -> Value {
    let models: Vec<Value> = provider
        .models
        .iter()
        .map(|m| {
            let (key, efforts, default) = match &m.effort {
                Some(e) => (Some(e.key.as_str()), e.values.as_slice(), e.default.as_deref()),
                None => (None, &[][..], None),
            };
            let efforts: Vec<Value> = efforts.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect();
            json!({ "id": m.id, "label": m.label, "effortKey": key, "efforts": efforts, "defaultEffort": default })
        })
        .collect();
    json!({ "id": provider.id, "name": provider.name, "models": models })
}

/// All message items after `after_position`, following pagination. Updates the cached detail.
fn read_messages(
    client: &mut T3Client,
    thread_id: &str,
    after_position: i64,
    max_chars: usize,
    cache: &mut ThreadCache,
) -> Result<Vec<TimelineItem>> {
    let mut items = Vec::new();
    let mut after = after_position;
    for _ in 0..20 {
        let options = ReadOptions {
            after_position: Some(after),
            limit: Some(100),
            max_chars_per_item: Some(max_chars),
            ..Default::default()
        };
        let result = client.read_thread(thread_id, options)?;
        if let Some(last) = result.items.last() {
            cache.last_position = cache.last_position.max(last.position);
        }
        cache.detail = Some(result.thread);
        items.extend(result.items.into_iter().filter(|i| MESSAGE_TYPES.contains(&i.kind.as_str())));
        match result.next_position {
            Some(next) if result.has_more => after = next,
            _ => break,
        }
    }
    Ok(items)
}

fn latest_activity(client: &mut T3Client, thread_id: &str, cache: &ThreadCache) -> Result<Option<String>> {
    let total = cache.detail.as_ref().map_or(0, |d| d.item_count);
    let options = ReadOptions {
        view: Some("activity"),
        after_position: Some((total - 4).max(-1)),
        limit: Some(10),
        max_chars_per_item: Some(200),
    };
    let result = client.read_thread(thread_id, options)?;
    Ok(result.items.last().map(activity_label))
}

const WAITING_FOR_ANSWER: &str = "Waiting for your answer";

/// One line about a timeline item. Tool calls and questions carry JSON text,
/// whose first line is only `{` or `[`.
fn activity_label(item: &TimelineItem) -> String {
    if item.kind == "user_input_request" {
        return WAITING_FOR_ANSWER.to_string();
    }
    let text = item.title.as_deref().or(item.text.as_deref()).unwrap_or_default();
    let first = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    let label = if first.starts_with(['{', '[']) {
        match json_string_field(text, "toolName") {
            Some(tool) => format!("Using {tool}"),
            None => item.kind.replace('_', " "),
        }
    } else if first.is_empty() {
        item.kind.replace('_', " ")
    } else {
        first.to_string()
    };
    label.chars().take(160).collect()
}

/// A string field from JSON text that may be cut off (items are truncated).
fn json_string_field(text: &str, field: &str) -> Option<String> {
    let rest = &text[text.find(&format!("\"{field}\""))? + field.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let value = &rest[..rest.find('"')?];
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{TimelineItem, activity_label, unix_seconds};

    #[test]
    fn labels_activity_without_raw_json() {
        let item =
            |kind: &str, text: &str| TimelineItem { kind: kind.into(), text: Some(text.into()), ..Default::default() };
        assert_eq!(activity_label(&item("user_input_request", "[\n  {\n    \"id\": \"q\"")), "Waiting for your answer");
        assert_eq!(
            activity_label(&item("dynamic_tool", "{\n  \"toolName\": \"AskUserQuestion\",\n  \"input\": {")),
            "Using AskUserQuestion"
        );
        assert_eq!(activity_label(&item("dynamic_tool", "{\n  \"inp")), "dynamic tool");
        assert_eq!(activity_label(&item("command_execution", "$ ls -la\ntotal 3")), "$ ls -la");
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), 0);
        assert_eq!(unix_seconds("2026-10-16T13:45:14.123Z"), 1_792_158_314);
        assert_eq!(unix_seconds("2026-10-16T15:45:14+02:00"), 1_792_158_314);
        assert_eq!(unix_seconds(""), 0);
    }
}
