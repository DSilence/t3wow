//! A stand-in for a T3 Code environment's /mcp endpoint, serving the handful
//! of tools the bridge uses with an in-memory thread model.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use tiny_http::{Header, Response, Server};

const TOOLS: &[&str] = &[
    "t3_project_list",
    "t3_project_read",
    "t3_thread_list",
    "t3_thread_read",
    "t3_thread_send",
    "t3_thread_launch",
    "t3_thread_interrupt",
    "orchestrator_capabilities",
    "t3_thread_configuration",
    "t3_thread_configure",
];

pub struct Item {
    pub position: i64,
    pub kind: String,
    pub status: String,
    pub text: String,
}

pub struct Thread {
    pub thread_id: String,
    pub project_id: String,
    pub title: String,
    pub status: String,
    pub items: Vec<Item>,
    pub updated: u64,
    pub model_selection: Value,
}

#[derive(Default)]
pub struct State {
    pub calls: Vec<(String, Value)>,
    pub threads: BTreeMap<String, Thread>,
    seen_request_ids: HashSet<String>,
    clock: u64,
}

fn iso(n: u64) -> String {
    // Distinct, increasing timestamps within one day.
    format!("2026-10-16T{:02}:{:02}:{:02}.000Z", n / 3600 % 24, n / 60 % 60, n % 60)
}

impl State {
    pub fn add_thread(&mut self, thread_id: &str, project_id: &str, title: &str, items: &[(&str, &str)]) -> String {
        let thread = Thread {
            thread_id: thread_id.into(),
            project_id: project_id.into(),
            title: title.into(),
            status: "completed".into(),
            items: Vec::new(),
            updated: 0,
            model_selection: json!({ "instanceId": "claudeAgent", "model": "claude-opus-5-5" }),
        };
        self.threads.insert(thread_id.into(), thread);
        for (kind, text) in items {
            self.append(thread_id, kind, text);
        }
        thread_id.into()
    }

    pub fn append(&mut self, thread_id: &str, kind: &str, text: &str) {
        self.clock += 1;
        let clock = self.clock;
        let thread = self.threads.get_mut(thread_id).unwrap();
        thread.items.push(Item {
            position: thread.items.len() as i64,
            kind: kind.into(),
            status: "completed".into(),
            text: text.into(),
        });
        thread.updated = clock;
    }

    pub fn count(&self, tool: &str) -> usize {
        self.calls.iter().filter(|(name, _)| name == tool).count()
    }

    fn list_item(t: &Thread) -> Value {
        json!({
            "threadId": t.thread_id,
            "title": t.title,
            "status": t.status,
            "latestRunId": null,
            "providerInstanceId": "claudeAgent",
            "model": "claude-opus-5-5",
            "runtimeMode": "full-access",
            "interactionMode": "default",
            "settled": false,
            "snoozed": false,
            "parentThreadId": null,
            "itemCount": t.items.len(),
            "createdAt": iso(0),
            "updatedAt": iso(t.updated),
        })
    }

    /// Ok(result) | Err(Some(failure payload)) | Err(None) for invalid params.
    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, Option<Value>> {
        let failure = |code: &str, message: &str| {
            Err(Some(json!({ "_tag": "OrchestratorMcpFailure", "code": code, "message": message })))
        };
        let project = json!({
            "id": "p1", "title": "Demo", "workspaceRoot": "/work/demo", "defaultModelSelection": null,
            "updatedAt": iso(0), "deletedAt": null, "scripts": [],
        });
        match name {
            "t3_project_list" => Ok(json!({ "projects": [project], "nextCursor": null })),
            "t3_project_read" => Ok(project),
            "t3_thread_list" => {
                let threads: Vec<Value> =
                    self.threads.values().filter(|t| args["projectId"] == t.project_id).map(Self::list_item).collect();
                Ok(
                    json!({ "projectId": args["projectId"], "threads": threads, "nextCursor": null, "total": threads.len() }),
                )
            }
            "t3_thread_read" => {
                let Some(t) = self.threads.get(args["threadId"].as_str().unwrap_or_default()) else {
                    return failure("thread_not_found", "No such thread");
                };
                if args["afterPosition"].as_i64().is_some_and(|p| p < 0) {
                    return Err(None);
                }
                let after = args["afterPosition"].as_i64().unwrap_or(-1);
                let view = args["view"].as_str().unwrap_or("messages");
                let limit = args["limit"].as_u64().unwrap_or(50) as usize;
                let matching: Vec<&Item> = t
                    .items
                    .iter()
                    .filter(|i| {
                        i.position > after
                            && (view == "activity"
                                || ["user_message", "assistant_message", "proposed_plan"].contains(&i.kind.as_str()))
                    })
                    .collect();
                let page: Vec<Value> = matching
                    .iter()
                    .take(limit)
                    .map(|i| {
                        json!({
                            "position": i.position, "itemId": format!("{}-{}", t.thread_id, i.position), "runId": null,
                            "type": i.kind, "status": i.status, "title": null, "text": i.text, "textTruncated": false,
                            "createdBy": null, "updatedAt": iso(t.updated),
                        })
                    })
                    .collect();
                let mut thread = Self::list_item(t);
                for (k, v) in [
                    ("projectId", json!(t.project_id)),
                    ("activeRunId", Value::Null),
                    ("branch", json!("main")),
                    ("worktreePath", Value::Null),
                    ("pendingRequestCount", json!(0)),
                    ("archived", json!(false)),
                ] {
                    thread[k] = v;
                }
                let next = page.last().map(|i| i["position"].clone()).unwrap_or(Value::Null);
                Ok(
                    json!({ "thread": thread, "recentRuns": [], "items": page, "nextPosition": next, "hasMore": matching.len() > limit }),
                )
            }
            "t3_thread_send" => {
                let thread_id = args["threadId"].as_str().unwrap_or_default().to_string();
                if !self.threads.contains_key(&thread_id) {
                    return failure("thread_not_found", "No such thread");
                }
                let request_id = args["clientRequestId"].as_str().unwrap_or_default().to_string();
                let message = args["message"].as_str().unwrap_or_default().to_string();
                if self.seen_request_ids.insert(request_id) {
                    self.append(&thread_id, "user_message", &message);
                    self.append(&thread_id, "assistant_message", &format!("Echo: {message}"));
                }
                Ok(
                    json!({ "threadId": thread_id, "messageId": "m", "runId": "r", "status": "completed", "delivery": "started" }),
                )
            }
            "t3_thread_launch" => {
                let id = format!("t{}", self.threads.len() + 1);
                let project_id = args["projectId"].as_str().unwrap_or_default().to_string();
                let title = args["title"].as_str().unwrap_or_default().to_string();
                let message = args["message"].as_str().unwrap_or_default().to_string();
                self.add_thread(&id, &project_id, &title, &[("user_message", &message)]);
                Ok(
                    json!({ "threadId": id, "projectId": project_id, "modelSelection": {}, "runId": null, "status": "running" }),
                )
            }
            "orchestrator_capabilities" => Ok(json!({ "providers": [
                { "providerInstanceId": "claudeAgent", "displayName": "Claude", "constraints": [], "models": [
                    { "id": "claude-opus-5-5", "label": "Claude Opus 5.5", "options": [
                        { "id": "effort", "type": "select", "options": [
                            { "id": "medium", "label": "Medium", "isDefault": true }, { "id": "high", "label": "High" } ] } ] },
                    { "id": "claude-haiku-4-5", "label": "Claude Haiku 4.5", "options": [] } ] },
                { "providerInstanceId": "cursor", "displayName": "Cursor", "constraints": ["Cursor is disabled."], "models": [] },
            ]})),
            "t3_thread_configuration" => match self.threads.get(args["threadId"].as_str().unwrap_or_default()) {
                Some(t) => Ok(json!({ "threadId": t.thread_id, "modelSelection": t.model_selection })),
                None => failure("thread_not_found", "No such thread"),
            },
            "t3_thread_configure" => {
                self.clock += 1;
                let clock = self.clock;
                match self.threads.get_mut(args["threadId"].as_str().unwrap_or_default()) {
                    Some(t) => {
                        t.model_selection = args["modelSelection"].clone();
                        t.updated = clock;
                        Ok(json!({ "threadId": t.thread_id, "modelSelection": t.model_selection }))
                    }
                    None => failure("thread_not_found", "No such thread"),
                }
            }
            "t3_thread_interrupt" => {
                Ok(json!({ "threadId": args["threadId"], "runId": null, "status": "no_active_run" }))
            }
            _ => failure("invalid_request", &format!("Unknown tool {name}")),
        }
    }
}

pub struct FakeT3 {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeT3 {
    pub fn start() -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        state.lock().unwrap().add_thread(
            "t1",
            "p1",
            "Fix the login bug",
            &[
                ("user_message", "The login button does nothing"),
                ("reasoning", "thinking..."),
                ("assistant_message", "Found it: the handler was never bound."),
            ],
        );
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(50)) {
                        handle(request, &state);
                    }
                }
            })
        };
        Self { url: format!("http://127.0.0.1:{port}/mcp"), state, stop, handle: Some(handle) }
    }
}

impl Drop for FakeT3 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn handle(mut request: tiny_http::Request, state: &Mutex<State>) {
    if request.method().as_str() == "DELETE" {
        let _ = request.respond(Response::empty(200));
        return;
    }
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let Some(id) = message.get("id").cloned() else {
        let _ = request.respond(Response::empty(202));
        return;
    };
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    let mut stream = false;
    let reply = match method {
        "initialize" => json!({ "result": {
            "protocolVersion": params["protocolVersion"], "capabilities": { "tools": {} },
            "serverInfo": { "name": "fake-t3", "version": "0" },
        }}),
        "tools/list" => {
            let tools: Vec<Value> =
                TOOLS.iter().map(|name| json!({ "name": name, "inputSchema": { "type": "object" } })).collect();
            json!({ "result": { "tools": tools } })
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            // Exercise the event stream response path too.
            stream = name == "t3_thread_list";
            let mut state = state.lock().unwrap();
            state.calls.push((name.to_string(), args.clone()));
            match state.tool(name, &args) {
                Ok(result) => {
                    json!({ "result": { "content": [{ "type": "text", "text": result.to_string() }], "structuredContent": result } })
                }
                Err(Some(payload)) => json!({ "result": {
                    "isError": true, "content": [{ "type": "text", "text": payload.to_string() }], "structuredContent": payload,
                }}),
                Err(None) => {
                    json!({ "error": { "code": -32602, "message": "Expected a value greater than or equal to 0 at [\"afterPosition\"]" } })
                }
            }
        }
        _ => json!({ "error": { "code": -32601, "message": "Method not found" } }),
    };
    let mut reply = reply;
    reply["jsonrpc"] = json!("2.0");
    reply["id"] = id;
    let response = if stream {
        let text = format!(
            "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{{}}}}\n\nevent: message\ndata: {reply}\n\n"
        );
        Response::from_string(text).with_header(header("content-type", "text/event-stream"))
    } else {
        Response::from_string(reply.to_string()).with_header(header("content-type", "application/json"))
    };
    let _ = request.respond(response.with_header(header("mcp-session-id", "fake-session")));
}
