//! A minimal MCP client over Streamable HTTP: initialize, tools/list and
//! tools/call, which is all the bridge needs. Responses may come back as JSON
//! or as a server-sent event stream.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::BRIDGE_VERSION;
use crate::error::{Error, Result};

const PROTOCOL_VERSION: &str = "2025-06-18";
const INVALID_PARAMS: i64 = -32602;

pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(format!("t3wow/{BRIDGE_VERSION}"))
        .build()
        .into()
}

pub struct McpSession {
    agent: ureq::Agent,
    url: String,
    token: Option<String>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    next_id: u64,
}

/// Result of `tools/call`, before the bridge interprets it.
pub struct ToolResult {
    pub payload: Value,
    pub is_error: bool,
    pub text: String,
}

impl McpSession {
    pub fn connect(url: &str, token: Option<String>) -> Result<Self> {
        let mut session = Self {
            agent: http_agent(),
            url: url.to_string(),
            token,
            session_id: None,
            protocol_version: None,
            next_id: 1,
        };
        let init = session.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "t3wow-bridge", "version": BRIDGE_VERSION },
            }),
        )?;
        session.protocol_version = Some(init["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION).to_string());
        session.notify("notifications/initialized")?;
        Ok(session)
    }

    pub fn list_tools(&mut self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.request("tools/list", params)?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                if let Some(name) = tool["name"].as_str() {
                    names.push(name.to_string());
                }
            }
            cursor = page["nextCursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
            if cursor.is_none() {
                return Ok(names);
            }
        }
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<ToolResult> {
        let result =
            self.request("tools/call", json!({ "name": name, "arguments": arguments })).map_err(
                |error| match error {
                    // The server rejected the arguments; retrying the same call cannot succeed.
                    Error::Tool { code, message, .. } if code == "invalid_params" => {
                        Error::Tool { tool: name.to_string(), code: "invalid_request".into(), message }
                    }
                    other => other,
                },
            )?;
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let payload = match result.get("structuredContent") {
            Some(v) if !v.is_null() => v.clone(),
            _ => serde_json::from_str(&text)
                .unwrap_or_else(|_| if text.is_empty() { Value::Null } else { json!({ "message": text }) }),
        };
        Ok(ToolResult { payload, is_error: result["isError"] == true, text })
    }

    /// End the session on the server. Best effort.
    pub fn close(&mut self) {
        if let Some(id) = self.session_id.take() {
            let mut request = self.agent.delete(&self.url).header("mcp-session-id", &id);
            if let Some(token) = &self.token {
                request = request.header("authorization", &format!("Bearer {token}"));
            }
            let _ = request.call();
        }
    }

    fn post(&mut self, body: &Value) -> Result<ureq::http::Response<ureq::Body>> {
        let mut request = self
            .agent
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(token) = &self.token {
            request = request.header("authorization", &format!("Bearer {token}"));
        }
        if let Some(id) = &self.session_id {
            request = request.header("mcp-session-id", id);
        }
        if let Some(version) = &self.protocol_version {
            request = request.header("mcp-protocol-version", version);
        }
        let response = request.send(body.to_string())?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            let mut response = response;
            let detail = response.body_mut().read_to_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&detail)
                .ok()
                .and_then(|v| v["message"].as_str().map(String::from))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(Error::AuthRequired(message));
        }
        if let Some(id) = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session_id = Some(id.to_string());
        }
        Ok(response)
    }

    fn notify(&mut self, method: &str) -> Result<()> {
        let response = self.post(&json!({ "jsonrpc": "2.0", "method": method }))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::other(format!("{method}: HTTP {status}")));
        }
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut response = self.post(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let status = response.status().as_u16();
        if status == 404 && self.session_id.is_some() {
            return Err(Error::other("MCP session expired"));
        }
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let message = if is_stream {
            read_event_stream(response.body_mut().as_reader(), id)?
        } else {
            let text = response.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string()?;
            if !(200..300).contains(&status) {
                return Err(Error::other(format!("{method}: HTTP {status} {}", text.trim())));
            }
            serde_json::from_str(&text)?
        };
        if let Some(error) = message.get("error") {
            let code = error["code"].as_i64().unwrap_or(0);
            let text = error["message"].as_str().unwrap_or("MCP error").to_string();
            if code == INVALID_PARAMS {
                return Err(Error::Tool { tool: method.into(), code: "invalid_params".into(), message: text });
            }
            return Err(Error::other(format!("{method}: {text}")));
        }
        Ok(message.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// Read server-sent events until the response to request `id` arrives.
fn read_event_stream(reader: impl std::io::Read, id: u64) -> Result<Value> {
    let mut data = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line?;
        if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            data.push('\n');
        } else if line.is_empty() && !data.is_empty() {
            if let Ok(message) = serde_json::from_str::<Value>(&data)
                && message["id"] == id
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                return Ok(message);
            }
            data.clear();
        }
    }
    Err(Error::other("event stream ended without a response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_matching_event() {
        let stream = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                      data: {\"jsonrpc\":\"2.0\",\"id\":3,\n\
                      data: \"result\":{\"ok\":true}}\n\n";
        let message = read_event_stream(stream.as_bytes(), 3).unwrap();
        assert_eq!(message["result"]["ok"], true);
        assert!(read_event_stream("data: {}\n\n".as_bytes(), 3).is_err());
    }
}
