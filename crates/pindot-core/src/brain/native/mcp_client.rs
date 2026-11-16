//! The loop's MCP client of Pindot's own server, on 127.0.0.1 with the
//! bearer token and an `X-Pindot-Session` header of the loop's session id,
//! exactly as the `claude` Pindot starts reaches it: so approvals, the
//! input lock, Esc, the overlay, the activity log and the Vault rules all
//! apply unchanged.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::openai::{Image, ToolDef};
use crate::server::tools::SESSION_HEADER;

const PROTOCOL: &str = "2025-06-18";
/// A screenshot result is a few MB of base64; leave room.
const RESPONSE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<Image>,
    pub is_error: bool,
}

impl ToolOutput {
    fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), images: vec![], is_error: true }
    }
}

pub struct McpClient {
    url: String,
    token: String,
    session_key: String,
    mcp_session: Mutex<Option<String>>,
    agent: ureq::Agent,
    next_id: AtomicU64,
}

impl McpClient {
    pub fn new(url: &str, token: &str, session_key: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(10 * 60)))
            .build()
            .into();
        Self {
            url: url.into(),
            token: token.into(),
            session_key: session_key.into(),
            mcp_session: Mutex::new(None),
            agent,
            next_id: AtomicU64::new(1),
        }
    }

    /// One POST: (HTTP status, a new MCP session id if given, the JSON-RPC
    /// message with an `id`, or Null).
    fn post(&self, body: &Value) -> Result<(u16, Option<String>, Value), String> {
        let mut r = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header(SESSION_HEADER, &self.session_key);
        if let Some(s) = self.mcp_session.lock().unwrap().clone() {
            r = r.header("mcp-session-id", s).header("mcp-protocol-version", PROTOCOL);
        }
        let resp = r.send(&body.to_string()).map_err(|e| format!("Pindot's desktop tools did not answer: {e}"))?;
        let code = resp.status().as_u16();
        let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
        let text = resp.into_body().with_config().limit(RESPONSE_LIMIT).read_to_string().unwrap_or_default();
        Ok((code, sid, rpc_message(&text)))
    }

    fn initialize(&self) -> Result<(), String> {
        *self.mcp_session.lock().unwrap() = None;
        let (code, sid, v) = self.post(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL, "capabilities": {},
            "clientInfo": {"name": "pindot-loop", "version": env!("CARGO_PKG_VERSION")}}}))?;
        if code >= 400 || v.get("result").is_none() {
            return Err(format!("Pindot's desktop tools refused to start a session ({code})"));
        }
        *self.mcp_session.lock().unwrap() = sid;
        self.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(())
    }

    /// A JSON-RPC call; opens the MCP session first, and once more if the
    /// server forgot it (404).
    fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.mcp_session.lock().unwrap().is_none() {
            self.initialize()?;
        }
        for attempt in 0..2 {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let (code, _, v) = self.post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
            if code == 404 && attempt == 0 {
                self.initialize()?;
                continue;
            }
            if let Some(r) = v.get("result") {
                return Ok(r.clone());
            }
            let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no result");
            return Err(format!("{method}: {msg} ({code})"));
        }
        Err(format!("{method}: the session could not be opened"))
    }

    /// The tools, as the model sees them.
    pub fn list_tools(&self) -> Result<Vec<ToolDef>, String> {
        let mut out = vec![];
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.rpc("tools/list", params)?;
            for t in r.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let mut schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"}));
                if let Some(o) = schema.as_object_mut() {
                    o.remove("$schema");
                }
                out.push(ToolDef {
                    name: t.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                    parameters: schema,
                });
            }
            cursor = r.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                return Ok(out);
            }
        }
    }

    /// Calls a tool. Never fails: a problem becomes an error result the
    /// model reads.
    pub fn call(&self, name: &str, arguments: &str) -> ToolOutput {
        let raw = if arguments.trim().is_empty() { "{}" } else { arguments };
        let args: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(format!("The arguments were not valid JSON ({e}). Call {name} again with a JSON object.")),
        };
        let r = match self.rpc("tools/call", json!({"name": name, "arguments": args})) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(e),
        };
        let mut text = vec![];
        let mut images = vec![];
        for c in r.get("content").and_then(Value::as_array).into_iter().flatten() {
            match c.get("type").and_then(Value::as_str) {
                Some("text") => text.push(c.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
                Some("image") => images.push(Image {
                    mime: c.get("mimeType").and_then(Value::as_str).unwrap_or("image/png").to_string(),
                    base64: c.get("data").and_then(Value::as_str).unwrap_or_default().to_string(),
                }),
                _ => {}
            }
        }
        ToolOutput { text: text.join("\n"), images, is_error: r.get("isError").and_then(Value::as_bool).unwrap_or(false) }
    }

    /// Ends the MCP session (the caller also ends Pindot's session with
    /// `ServerState::end_session`).
    pub fn close(&self) {
        let Some(s) = self.mcp_session.lock().unwrap().take() else { return };
        let _ = self.agent.delete(&self.url).header("Authorization", format!("Bearer {}", self.token)).header("mcp-session-id", s).call();
    }
}

/// The JSON-RPC message with an `id` in a JSON or server-sent-event body.
fn rpc_message(body: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return v;
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v.get("id").is_some())
        .unwrap_or(Value::Null)
}
