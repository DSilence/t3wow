//! OAuth sign-in for the environment's /mcp endpoint: discovery, dynamic client
//! registration and the authorization code flow with PKCE, with a loopback
//! redirect. T3 issues no refresh tokens; a sign-in lasts 30 days and then
//! `t3wow login` has to be run again.
//!
//! `~/.t3wow/auth.json` maps each MCP URL to its stored sign-in (client
//! registration, tokens, discovery results).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::{BRIDGE_VERSION, home_dir, read_json_file, write_json_file};
use crate::error::{Error, Result};
use crate::mcp::http_agent;
use crate::url;

fn auth_path() -> PathBuf {
    home_dir().join("auth.json")
}

fn read_store() -> Result<Map<String, Value>> {
    Ok(read_json_file(&auth_path())?.unwrap_or_default())
}

fn update_store(server_url: &str, update: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    let mut store = read_store()?;
    let entry = store.entry(server_url).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    update(entry.as_object_mut().unwrap());
    write_json_file(&auth_path(), &store, true)?;
    Ok(())
}

pub fn stored_auth(server_url: &str) -> Result<Option<Value>> {
    Ok(read_store()?.remove(server_url))
}

pub fn forget_auth(server_url: &str) -> Result<()> {
    let mut store = read_store()?;
    store.remove(server_url);
    write_json_file(&auth_path(), &store, true)?;
    Ok(())
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The stored access token, unless it has expired.
pub fn access_token(server_url: &str) -> Result<Option<String>> {
    let Some(entry) = stored_auth(server_url)? else { return Ok(None) };
    if entry["tokensExpireAt"].as_i64().is_some_and(|at| at <= now_ms()) {
        return Ok(None);
    }
    Ok(entry["tokens"]["access_token"].as_str().map(String::from))
}

/// T3 Code before 0.0.46 has no OAuth sign-in for outside agents: its /mcp only
/// takes tokens T3 hands its own agents, and the discovery URLs fall through to
/// the web app's HTML.
pub fn check_sign_in_supported(server_url: &str) -> Result<()> {
    let metadata_url = protected_resource_url(server_url)?;
    let origin = url::origin(server_url).unwrap_or(server_url);
    let mut response = http_agent()
        .get(&metadata_url)
        .header("accept", "application/json")
        .call()
        .map_err(|e| Error::other(format!("Cannot reach {origin}: {e}")))?;
    let is_json =
        response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("json"));
    let _ = response.body_mut().read_to_string();
    let status = response.status().as_u16();
    if (200..300).contains(&status) && is_json {
        return Ok(());
    }
    if !is_json {
        return Err(Error::other(format!(
            "{} does not offer sign-in for outside agents. This needs T3 Code 0.0.46 or newer on that machine \
             (nightly for now); older versions only accept T3's own agents on /mcp.",
            url::host(server_url)
        )));
    }
    Err(Error::other(format!("{metadata_url} answered HTTP {status}. Is this the environment's MCP URL?")))
}

/// RFC 9728: `/.well-known/oauth-protected-resource` + the resource's path.
fn protected_resource_url(server_url: &str) -> Result<String> {
    let origin = url::origin(server_url).ok_or_else(|| Error::other(format!("Not a URL: {server_url}")))?;
    let path = url::path(server_url).trim_end_matches('/');
    Ok(format!("{origin}/.well-known/oauth-protected-resource{path}"))
}

fn get_json(agent: &ureq::Agent, target: &str) -> Result<Option<Value>> {
    let mut response = agent.get(target).header("accept", "application/json").call()?;
    if !(200..300).contains(&response.status().as_u16()) {
        return Ok(None);
    }
    Ok(serde_json::from_str(&response.body_mut().read_to_string()?).ok())
}

struct Discovery {
    resource: String,
    scopes: Vec<String>,
    metadata: Value,
    state: Value,
}

fn discover(agent: &ureq::Agent, server_url: &str) -> Result<Discovery> {
    let origin = url::origin(server_url).unwrap_or(server_url).to_string();
    let mut resource_metadata_url = protected_resource_url(server_url)?;
    let mut resource_metadata = get_json(agent, &resource_metadata_url)?;
    if resource_metadata.is_none() {
        resource_metadata_url = format!("{origin}/.well-known/oauth-protected-resource");
        resource_metadata = get_json(agent, &resource_metadata_url)?;
    }
    let resource_metadata = resource_metadata.unwrap_or(Value::Null);
    let authorization_server_url = resource_metadata["authorization_servers"][0]
        .as_str()
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or(origin);

    // RFC 8414 with the issuer's path inserted, then the OpenID Connect variants.
    let as_origin = url::origin(&authorization_server_url).unwrap_or(&authorization_server_url).to_string();
    let as_path = url::path(&authorization_server_url).trim_end_matches('/').to_string();
    let candidates = if as_path.is_empty() {
        vec![
            format!("{as_origin}/.well-known/oauth-authorization-server"),
            format!("{as_origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{as_origin}/.well-known/oauth-authorization-server{as_path}"),
            format!("{as_origin}/.well-known/openid-configuration{as_path}"),
            format!("{as_origin}{as_path}/.well-known/openid-configuration"),
        ]
    };
    let mut metadata = None;
    for candidate in candidates {
        if let Some(found) = get_json(agent, &candidate)? {
            metadata = Some(found);
            break;
        }
    }
    let metadata = metadata.ok_or_else(|| {
        Error::other(format!("Could not find the authorization server metadata for {authorization_server_url}"))
    })?;
    for field in ["authorization_endpoint", "token_endpoint"] {
        if metadata[field].as_str().is_none() {
            return Err(Error::other(format!("The authorization server metadata has no {field}")));
        }
    }

    let resource = resource_metadata["resource"].as_str().unwrap_or(server_url).to_string();
    let scopes = resource_metadata["scopes_supported"]
        .as_array()
        .or(metadata["scopes_supported"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    let state = json!({
        "authorizationServerUrl": authorization_server_url,
        "resourceMetadataUrl": resource_metadata_url,
        "resourceMetadata": resource_metadata,
        "authorizationServerMetadata": metadata,
    });
    Ok(Discovery { resource, scopes, metadata, state })
}

pub fn redirect_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

fn client_metadata(port: u16) -> Value {
    json!({
        "client_name": "T3 WoW bridge",
        "client_uri": "https://github.com/pingdotgg/t3code",
        "software_id": "t3wow",
        "software_version": BRIDGE_VERSION,
        "redirect_uris": [redirect_url(port)],
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}

/// Reuse the stored client registration if it was made for this redirect, else register.
fn client_id(agent: &ureq::Agent, server_url: &str, discovery: &Discovery, port: u16) -> Result<String> {
    let stored = stored_auth(server_url)?.unwrap_or(Value::Null);
    let info = &stored["clientInformation"];
    let redirect = redirect_url(port);
    let same_redirect = info["redirect_uris"].as_array().is_some_and(|uris| uris.iter().any(|u| *u == redirect));
    if let (Some(id), true) = (info["client_id"].as_str(), same_redirect) {
        return Ok(id.to_string());
    }
    let endpoint = discovery.metadata["registration_endpoint"].as_str().ok_or_else(|| {
        Error::other("The environment does not allow registering the bridge (no registration_endpoint)")
    })?;
    let mut response = agent
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .send(client_metadata(port).to_string())?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    if !(200..300).contains(&status) {
        return Err(Error::other(format!("Registering the bridge failed: HTTP {status} {}", text.trim())));
    }
    let info: Value = serde_json::from_str(&text)?;
    let id = info["client_id"].as_str().ok_or_else(|| Error::other("Registration returned no client_id"))?.to_string();
    update_store(server_url, |entry| {
        entry.insert("clientInformation".into(), info);
    })?;
    Ok(id)
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| Error::other(format!("no randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// A sign-in waiting for the browser to come back with a code.
pub struct PendingLogin {
    server_url: String,
    port: u16,
    listener: TcpListener,
    discovery: Discovery,
    client_id: String,
    verifier: String,
    state: String,
    pub authorization_url: String,
}

/// Prepare the sign-in: discover the authorization server, register the bridge
/// and start listening on the loopback redirect.
pub fn begin_login(server_url: &str, port: u16) -> Result<PendingLogin> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Error::other(format!("Cannot listen on 127.0.0.1:{port} for the sign-in redirect: {e}")))?;
    let agent = http_agent();
    let discovery = discover(&agent, server_url)?;
    let client_id = client_id(&agent, server_url, &discovery, port)?;
    let verifier = random_token(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(16)?;
    let redirect = redirect_url(port);
    let scope = discovery.scopes.join(" ");
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("redirect_uri", redirect.as_str()),
        ("state", state.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if !scope.is_empty() {
        params.push(("scope", scope.as_str()));
    }
    let endpoint = discovery.metadata["authorization_endpoint"].as_str().unwrap_or_default();
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let authorization_url = format!("{endpoint}{separator}{}", url::query_string(&params));
    Ok(PendingLogin {
        server_url: server_url.to_string(),
        port,
        listener,
        discovery,
        client_id,
        verifier,
        state,
        authorization_url,
    })
}

impl PendingLogin {
    /// Wait for the redirect, then trade the code for a token and store it.
    pub fn finish(self, timeout: Duration) -> Result<()> {
        let code = self.wait_for_code(timeout)?;
        let token_endpoint = self.discovery.metadata["token_endpoint"].as_str().unwrap_or_default();
        let redirect = redirect_url(self.port);
        let mut response = http_agent().post(token_endpoint).header("accept", "application/json").send_form([
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", self.verifier.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", self.client_id.as_str()),
            ("resource", self.discovery.resource.as_str()),
        ])?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string()?;
        let tokens: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !(200..300).contains(&status) || tokens["access_token"].as_str().is_none() {
            let reason = tokens["error_description"].as_str().or(tokens["error"].as_str()).unwrap_or(text.trim());
            if tokens["error"] == "invalid_client" {
                // The environment forgot the registration; register again next time.
                update_store(&self.server_url, |entry| {
                    entry.remove("clientInformation");
                })?;
            }
            return Err(Error::other(format!("The environment did not issue a token: {reason}")));
        }
        let expires_at = tokens["expires_in"].as_i64().map(|s| now_ms() + s * 1000);
        update_store(&self.server_url, |entry| {
            entry.insert("tokens".into(), tokens);
            match expires_at {
                Some(at) => entry.insert("tokensExpireAt".into(), json!(at)),
                None => entry.remove("tokensExpireAt"),
            };
            entry.insert("discoveryState".into(), self.discovery.state.clone());
            entry.remove("codeVerifier");
        })
    }

    fn wait_for_code(&self, timeout: Duration) -> Result<String> {
        self.listener.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = self.handle_callback(stream) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(Error::other("Timed out waiting for sign-in."));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Answers one request; returns a result once the real redirect arrived.
    fn handle_callback(&self, mut stream: TcpStream) -> Option<Result<String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != "/callback" {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            return None;
        }
        let params = url::parse_query(query);
        let param = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        let issuer = self.discovery.metadata["issuer"].as_str();
        let (status, title, body, result) = if let Some(error) = param("error") {
            let description = param("error_description").unwrap_or(error).to_string();
            let page = escape_html(&description);
            (
                "400 Bad Request",
                "Sign-in failed",
                page,
                Some(Err(Error::other(format!("Authorization failed: {description}")))),
            )
        } else if param("state") != Some(self.state.as_str())
            || param("code").is_none()
            || param("iss").is_some_and(|iss| Some(iss) != issuer)
        {
            ("400 Bad Request", "Sign-in failed", "The response did not match this sign-in.".into(), None)
        } else {
            let code = param("code").unwrap_or_default().to_string();
            (
                "200 OK",
                "T3 WoW bridge is signed in",
                "You can close this tab and return to the game.".into(),
                Some(Ok(code)),
            )
        };
        let page = format!(
            "<!doctype html>\n<meta charset=\"utf-8\"><title>{title}</title>\n\
             <body style=\"font:16px system-ui;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0\">\n\
             <div style=\"text-align:center\"><h1>{title}</h1><p>{body}</p></div>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
            page.len()
        );
        result
    }
}

fn escape_html(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' | '<' | '>' | '"' | '\'' => format!("&#{};", c as u32),
            c => c.to_string(),
        })
        .collect()
}
