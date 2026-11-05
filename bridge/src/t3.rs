//! Typed helpers for the T3 MCP tools the bridge uses. The structs are subsets
//! of the tool results (packages/contracts/src/orchestratorMcp.ts in T3 Code).

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::mcp::McpSession;
use crate::oauth;

pub const ACTIVE_STATUSES: &[&str] = &["preparing", "queued", "starting", "running", "waiting"];

pub fn is_active(status: &str) -> bool {
    ACTIVE_STATUSES.contains(&status)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub instance_id: String,
    pub model: String,
}

/// Option ids T3 uses for a model's reasoning level.
const EFFORT_OPTIONS: &[&str] = &["effort", "reasoningEffort"];

/// A model picked in game: provider instance, model and optional reasoning level.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
    /// (option id, value), e.g. ("effort", "high").
    pub effort: Option<(String, String)>,
}

impl ModelChoice {
    pub fn to_selection(&self) -> Value {
        let mut selection = json!({ "instanceId": self.provider, "model": self.model });
        if let Some((id, value)) = &self.effort {
            selection["options"] = json!([{ "id": id, "value": value }]);
        }
        selection
    }

    pub fn from_selection(selection: &Value) -> Option<Self> {
        let model = selection["model"].as_str()?.to_string();
        let provider = selection["instanceId"].as_str().or(selection["provider"].as_str())?.to_string();
        let effort = selection["options"].as_array().into_iter().flatten().find_map(|option| {
            let id = option["id"].as_str().filter(|id| EFFORT_OPTIONS.contains(id))?;
            Some((id.to_string(), option["value"].as_str()?.to_string()))
        });
        Some(Self { provider, model, effort })
    }
}

/// A model's reasoning setting: option id, (value, label) choices and the default.
#[derive(Debug, Clone, PartialEq)]
pub struct EffortOption {
    pub key: String,
    pub values: Vec<(String, String)>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogModel {
    pub id: String,
    pub label: String,
    pub effort: Option<EffortOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    pub models: Vec<CatalogModel>,
}

/// Usable providers from `orchestrator_capabilities` (disabled or missing ones list constraints).
fn parse_catalog(capabilities: &Value) -> Vec<CatalogProvider> {
    let str_of = |v: &Value| v.as_str().unwrap_or_default().to_string();
    capabilities["providers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["constraints"].as_array().is_none_or(|c| c.is_empty()))
        .map(|p| CatalogProvider {
            id: str_of(&p["providerInstanceId"]),
            name: p["displayName"].as_str().map_or_else(|| str_of(&p["providerInstanceId"]), String::from),
            models: p["models"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| CatalogModel {
                    id: str_of(&m["id"]),
                    label: m["label"].as_str().map_or_else(|| str_of(&m["id"]), String::from),
                    effort: m["options"].as_array().into_iter().flatten().find_map(|option| {
                        let id = option["id"].as_str().filter(|id| EFFORT_OPTIONS.contains(id))?;
                        let choices: Vec<&Value> = option["options"].as_array()?.iter().collect();
                        let default = choices
                            .iter()
                            .find(|c| c["isDefault"] == true)
                            .and_then(|c| c["id"].as_str())
                            .map(String::from);
                        let values = choices
                            .iter()
                            .filter_map(|c| Some((c["id"].as_str()?.to_string(), str_of(&c["label"]))))
                            .collect();
                        Some(EffortOption { key: id.to_string(), values, default })
                    }),
                })
                .filter(|m| !m.id.is_empty())
                .collect(),
        })
        .filter(|p| !p.id.is_empty() && !p.models.is_empty())
        .collect()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub workspace_root: String,
    pub default_model_selection: Option<ModelSelection>,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadListItem {
    pub thread_id: String,
    pub title: String,
    pub status: String,
    pub provider_instance_id: String,
    pub model: String,
    pub runtime_mode: String,
    pub parent_thread_id: Option<String>,
    pub item_count: i64,
    pub updated_at: String,
    pub settled: bool,
    pub settled_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadDetail {
    pub thread_id: String,
    pub project_id: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub pending_request_count: i64,
    pub item_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimelineItem {
    pub position: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub text_truncated: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadReadResult {
    pub thread: ThreadDetail,
    pub items: Vec<TimelineItem>,
    pub next_position: Option<i64>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: Option<bool>,
    pub allow_custom_answer: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingRequest {
    pub request_id: String,
    pub questions: Vec<PendingQuestion>,
}

#[derive(Default)]
pub struct ReadOptions {
    /// Below 0 reads from the start (T3 only accepts non-negative positions).
    pub after_position: Option<i64>,
    pub limit: Option<i64>,
    pub view: Option<&'static str>,
    pub max_chars_per_item: Option<usize>,
}

pub struct T3Client {
    session: McpSession,
    pub tools: HashSet<String>,
}

impl T3Client {
    /// Connect with the stored sign-in for `server_url`, if there is one.
    pub fn connect(server_url: &str) -> Result<Self> {
        let token = oauth::access_token(server_url)?;
        let mut session = McpSession::connect(server_url, token)?;
        let tools = session.list_tools()?.into_iter().collect();
        Ok(Self { session, tools })
    }

    /// Read-only grants only list read tools, so this tells us what the sign-in allows.
    pub fn can_operate(&self) -> bool {
        self.tools.contains("t3_thread_send")
    }

    pub fn call<T: DeserializeOwned>(&mut self, name: &str, args: Value) -> Result<T> {
        let result = self.session.call_tool(name, args)?;
        if result.is_error {
            let message = result.payload["message"]
                .as_str()
                .map(String::from)
                .filter(|m| !m.is_empty())
                .or_else(|| (!result.text.is_empty()).then(|| result.text.clone()))
                .unwrap_or_else(|| "Tool call failed".into());
            let code = result.payload["code"].as_str().unwrap_or("error").to_string();
            return Err(Error::Tool { tool: name.into(), code, message });
        }
        serde_json::from_value(result.payload).map_err(|e| Error::other(format!("{name}: unexpected result: {e}")))
    }

    pub fn list_projects(&mut self) -> Result<Vec<Project>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            projects: Vec<Project>,
            next_cursor: Option<i64>,
        }
        let mut projects = Vec::new();
        let mut cursor = Some(0);
        while let Some(c) = cursor {
            let page: Page = self.call("t3_project_list", json!({ "cursor": c, "limit": 100 }))?;
            projects.extend(page.projects);
            cursor = page.next_cursor;
        }
        projects.retain(|p| p.deleted_at.is_none());
        Ok(projects)
    }

    /// A project's threads, newest first: the active ones, or the settled ones.
    pub fn list_threads(&mut self, project_id: &str, limit: usize, settled: bool) -> Result<Vec<ThreadListItem>> {
        #[derive(Deserialize)]
        struct Page {
            threads: Vec<ThreadListItem>,
        }
        let page: Page =
            self.call("t3_thread_list", json!({ "projectId": project_id, "settled": settled, "limit": limit }))?;
        Ok(page.threads)
    }

    /// Settling needs `t3_thread_organize` (newer environments, operate grants).
    pub fn can_settle(&self) -> bool {
        self.tools.contains("t3_thread_organize")
    }

    /// Settle a thread (T3 applies it when a running turn ends), or bring it back.
    pub fn settle_thread(&mut self, thread_id: &str, settled: bool) -> Result<()> {
        let action = if settled { "settle" } else { "unsettle" };
        let _: Value = self.call("t3_thread_organize", json!({ "threadId": thread_id, "action": action }))?;
        Ok(())
    }

    pub fn read_thread(&mut self, thread_id: &str, options: ReadOptions) -> Result<ThreadReadResult> {
        let mut args = json!({ "threadId": thread_id });
        if let Some(after) = options.after_position.filter(|p| *p >= 0) {
            args["afterPosition"] = json!(after);
        }
        if let Some(limit) = options.limit {
            args["limit"] = json!(limit);
        }
        if let Some(view) = options.view {
            args["view"] = json!(view);
        }
        if let Some(max) = options.max_chars_per_item {
            args["maxCharsPerItem"] = json!(max);
        }
        self.call("t3_thread_read", args)
    }

    pub fn send_to_thread(&mut self, thread_id: &str, message: &str, mode: &str, request_id: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Sent {
            #[serde(default)]
            delivery: String,
        }
        let sent: Sent = self.call(
            "t3_thread_send",
            json!({ "threadId": thread_id, "message": message, "mode": mode, "clientRequestId": request_id }),
        )?;
        Ok(sent.delivery)
    }

    /// Start a thread with the chosen model, else the project's default. Projects
    /// without a default need an explicit model, so reuse the one its most recent
    /// thread runs.
    pub fn launch_thread(
        &mut self,
        project_id: &str,
        title: &str,
        message: &str,
        choice: Option<&ModelChoice>,
    ) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Launched {
            thread_id: String,
        }
        let mut args = json!({ "projectId": project_id, "title": title, "message": message });
        if let Some(choice) = choice {
            args["modelSelection"] = choice.to_selection();
        } else {
            let project: Project = self.call("t3_project_read", json!({ "projectId": project_id }))?;
            if project.default_model_selection.is_none() {
                let threads = self.list_threads(project_id, 1, false)?;
                let Some(latest) = threads.first() else {
                    return Err(Error::Tool {
                        tool: "t3_thread_launch".into(),
                        code: "invalid_request".into(),
                        message:
                            "This project has no default model yet. Pick a model or start a thread in T3 Code first."
                                .into(),
                    });
                };
                args["modelSelection"] = json!({ "instanceId": latest.provider_instance_id, "model": latest.model });
            }
        }
        let launched: Launched = self.call("t3_thread_launch", args)?;
        Ok(launched.thread_id)
    }

    /// Switch the model a thread uses from its next turn on.
    pub fn configure_thread(&mut self, thread_id: &str, choice: &ModelChoice) -> Result<()> {
        let _: Value = self
            .call("t3_thread_configure", json!({ "threadId": thread_id, "modelSelection": choice.to_selection() }))?;
        Ok(())
    }

    /// The model a thread currently uses, if the tool is available.
    pub fn thread_model(&mut self, thread_id: &str) -> Result<Option<ModelChoice>> {
        if !self.tools.contains("t3_thread_configuration") {
            return Ok(None);
        }
        let config: Value = self.call("t3_thread_configuration", json!({ "threadId": thread_id }))?;
        Ok(ModelChoice::from_selection(&config["modelSelection"]))
    }

    /// Providers that can run threads, with their models and reasoning levels.
    pub fn model_catalog(&mut self) -> Result<Vec<CatalogProvider>> {
        if !self.tools.contains("orchestrator_capabilities") {
            return Ok(Vec::new());
        }
        let capabilities: Value = self.call("orchestrator_capabilities", json!({}))?;
        Ok(parse_catalog(&capabilities))
    }

    /// Returns the interrupt status (`interrupt_requested`, `no_active_run`, ...).
    pub fn interrupt_thread(&mut self, thread_id: &str, request_id: &str) -> Result<String> {
        let result: Value = self.call(
            "t3_thread_interrupt",
            json!({ "threadId": thread_id, "clientRequestId": request_id, "reason": "Stopped from World of Warcraft" }),
        )?;
        Ok(result["status"].as_str().unwrap_or_default().to_string())
    }

    pub fn pending_questions(&mut self, thread_id: &str) -> Result<Vec<PendingRequest>> {
        if !self.tools.contains("t3_pending_request_list") {
            return Ok(Vec::new());
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Ids {
            request_ids: Vec<String>,
        }
        let ids: Ids = self.call("t3_pending_request_list", json!({ "threadId": thread_id }))?;
        let mut requests = Vec::new();
        for request_id in ids.request_ids.iter().take(3) {
            requests
                .push(self.call("t3_pending_request_read", json!({ "threadId": thread_id, "requestId": request_id }))?);
        }
        Ok(requests)
    }

    pub fn answer_question(
        &mut self,
        thread_id: &str,
        request_id: &str,
        answers: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let _: Value = self.call(
            "t3_pending_request_respond",
            json!({ "threadId": thread_id, "requestId": request_id, "answers": answers }),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_model_catalog() {
        let capabilities = json!({ "providers": [
            { "providerInstanceId": "claudeAgent", "displayName": "Claude", "constraints": [], "models": [
                { "id": "claude-opus-5-5", "label": "Claude Opus 5.5", "options": [
                    { "id": "effort", "type": "select", "options": [
                        { "id": "low", "label": "Low" }, { "id": "high", "label": "High", "isDefault": true } ] },
                    { "id": "fastMode", "type": "boolean" } ] },
                { "id": "claude-haiku-4-5", "label": "Claude Haiku 4.5", "options": [] } ] },
            { "providerInstanceId": "cursor", "displayName": "Cursor", "constraints": ["Provider is disabled."],
              "models": [{ "id": "x", "label": "X" }] },
            { "providerInstanceId": "empty", "displayName": "Empty", "constraints": [], "models": [] },
        ]});
        let catalog = parse_catalog(&capabilities);
        assert_eq!(catalog.len(), 1, "disabled and empty providers are left out");
        let opus = &catalog[0].models[0];
        let effort = opus.effort.clone().unwrap();
        assert_eq!((effort.key.as_str(), effort.values.len(), effort.default.as_deref()), ("effort", 2, Some("high")));
        assert_eq!(catalog[0].models[1].effort, None);
    }

    #[test]
    fn converts_model_selections() {
        let selection = json!({ "instanceId": "codex", "model": "gpt-6-sol",
            "options": [{ "id": "serviceTier", "value": "default" }, { "id": "reasoningEffort", "value": "high" }] });
        let choice = ModelChoice::from_selection(&selection).unwrap();
        assert_eq!(choice.effort, Some(("reasoningEffort".into(), "high".into())));
        assert_eq!(
            choice.to_selection(),
            json!({ "instanceId": "codex", "model": "gpt-6-sol",
            "options": [{ "id": "reasoningEffort", "value": "high" }] })
        );
        assert!(ModelChoice::from_selection(&Value::Null).is_none());
    }
}
