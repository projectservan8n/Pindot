//! OpenAI Chat Completions, the format Ollama, LM Studio, llama.cpp,
//! OpenAI, OpenRouter, DeepSeek and Groq all accept: the messages, the
//! request body, the streamed reply, and one streamed HTTP call.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// One message of a conversation, as the loop keeps it.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    System(String),
    /// What the user said or typed (with pictures they attached).
    User { text: String, images: Vec<Image> },
    Assistant { text: String, calls: Vec<ToolCall> },
    Tool { call_id: String, text: String },
    /// Pictures returned by tools (`screenshot`, `zoom`), sent as a user
    /// message after the tool results: many servers take text only there.
    ToolImages(Vec<Image>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub mime: String,
    pub base64: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text, as the model wrote it.
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
}

fn image_part(i: &Image) -> Value {
    json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", i.mime, i.base64)}})
}

fn message(m: &Msg) -> Value {
    match m {
        Msg::System(t) => json!({"role": "system", "content": t}),
        Msg::User { text, images } if images.is_empty() => json!({"role": "user", "content": text}),
        Msg::User { text, images } => {
            let mut parts = vec![json!({"type": "text", "text": text})];
            parts.extend(images.iter().map(image_part));
            json!({"role": "user", "content": parts})
        }
        Msg::Assistant { text, calls } => {
            let content = if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) };
            let mut v = json!({"role": "assistant", "content": content});
            if !calls.is_empty() {
                v["tool_calls"] = calls
                    .iter()
                    .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}))
                    .collect();
            }
            v
        }
        Msg::Tool { call_id, text } => json!({"role": "tool", "tool_call_id": call_id, "content": text}),
        Msg::ToolImages(images) => {
            let mut parts = vec![json!({"type": "text", "text": "The picture the tool returned:"})];
            parts.extend(images.iter().map(image_part));
            json!({"role": "user", "content": parts})
        }
    }
}

pub fn request_body(model: &str, messages: &[Msg], tools: &[ToolDef]) -> Value {
    let mut b = json!({"model": model, "messages": messages.iter().map(message).collect::<Vec<_>>(), "stream": true});
    if !tools.is_empty() {
        b["tools"] = tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
    }
    b
}

/// A reply, put together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub thinking: String,
    pub calls: Vec<ToolCall>,
    pub finish: Option<String>,
}

#[derive(Default)]
struct PartialCall {
    index: u64,
    id: String,
    name: String,
    arguments: String,
}

/// Builds a `Reply` from the lines of a server-sent event stream.
#[derive(Default)]
pub struct StreamParser {
    text: String,
    thinking: String,
    calls: Vec<PartialCall>,
    finish: Option<String>,
    done: bool,
    seen: bool,
}

impl StreamParser {
    /// One line of the stream. An error object in it is returned as `Err`.
    pub fn line(&mut self, line: &str) -> Result<(), String> {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { return Ok(()) };
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(()) };
        if let Some(e) = v.get("error") {
            return Err(e.get("message").and_then(Value::as_str).unwrap_or("the server reported an error").to_string());
        }
        self.seen = true;
        let Some(choice) = v.pointer("/choices/0") else { return Ok(()) };
        if let Some(f) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(f.to_string());
        }
        let Some(d) = choice.get("delta") else { return Ok(()) };
        if let Some(t) = d.get("content").and_then(Value::as_str) {
            self.text.push_str(t);
        }
        for key in ["reasoning_content", "reasoning"] {
            if let Some(t) = d.get(key).and_then(Value::as_str) {
                self.thinking.push_str(t);
            }
        }
        for tc in d.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            let index = tc.get("index").and_then(Value::as_u64).unwrap_or(self.calls.len() as u64);
            let pos = match self.calls.iter().position(|c| c.index == index) {
                Some(p) => p,
                None => {
                    self.calls.push(PartialCall { index, ..Default::default() });
                    self.calls.len() - 1
                }
            };
            let c = &mut self.calls[pos];
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                c.id = id.to_string();
            }
            if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                c.name.push_str(n);
            }
            if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                c.arguments.push_str(a);
            }
        }
        Ok(())
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Whether any data line arrived.
    pub fn saw_data(&self) -> bool {
        self.seen
    }

    pub fn finish(mut self) -> Reply {
        self.calls.sort_by_key(|c| c.index);
        let (think_inline, text) = split_think(&self.text);
        let thinking =
            [self.thinking.trim(), think_inline.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
        Reply {
            text: text.trim().to_string(),
            thinking,
            calls: self
                .calls
                .into_iter()
                .map(|c| ToolCall {
                    id: if c.id.is_empty() { format!("call_{}", c.index) } else { c.id },
                    name: c.name,
                    arguments: if c.arguments.trim().is_empty() { "{}".into() } else { c.arguments },
                })
                .collect(),
            finish: self.finish,
        }
    }
}

/// Takes `<think>...</think>` (some models write their thinking inline)
/// out of `text`: (thinking, the rest). An unclosed `<think>` runs to the end.
pub fn split_think(text: &str) -> (String, String) {
    let mut thinking = String::new();
    let mut rest = String::new();
    let mut s = text;
    while let Some(start) = s.find("<think>") {
        rest.push_str(&s[..start]);
        let after = &s[start + "<think>".len()..];
        match after.find("</think>") {
            Some(end) => {
                thinking.push_str(after[..end].trim());
                s = &after[end + "</think>".len()..];
            }
            None => {
                thinking.push_str(after.trim());
                s = "";
            }
        }
    }
    rest.push_str(s);
    (thinking, rest)
}

/// A reply sent whole (a server that ignored `stream`).
pub fn complete_reply(body: &str) -> Result<Reply, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the server's answer was not understood".to_string())?;
    if let Some(e) = v.get("error") {
        return Err(e.get("message").and_then(Value::as_str).unwrap_or("the server reported an error").to_string());
    }
    let m = v.pointer("/choices/0/message").ok_or("the server's answer had no message")?;
    let (thinking, text) = split_think(m.get("content").and_then(Value::as_str).unwrap_or(""));
    let calls = m
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, c)| ToolCall {
            id: c.get("id").and_then(Value::as_str).map_or_else(|| format!("call_{i}"), str::to_string),
            name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string(),
            arguments: c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}").to_string(),
        })
        .collect();
    Ok(Reply {
        text: text.trim().to_string(),
        thinking,
        calls,
        finish: v.pointer("/choices/0/finish_reason").and_then(Value::as_str).map(str::to_string),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// Could not connect (the server is not running, or no network).
    Unreachable(String),
    Status { code: u16, body: String },
    /// The stream broke off or carried an error.
    Stream(String),
    Timeout,
    Cancelled,
}

/// Who is answering, for messages.
#[derive(Debug, Clone, PartialEq)]
pub struct Who {
    pub provider: String,
    pub model: String,
    pub local: bool,
}

pub fn is_context_overflow(e: &CallError) -> bool {
    let CallError::Status { code: 400 | 413, body } = e else { return false };
    let b = body.to_ascii_lowercase();
    b.contains("context") && ["length", "too long", "exceed", "maximum", "window"].iter().any(|w| b.contains(w))
}

/// A server error's own message, if its body has one.
fn server_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// What went wrong, in words for the user.
pub fn describe(e: &CallError, who: &Who) -> String {
    let (p, m) = (&who.provider, &who.model);
    match e {
        CallError::Unreachable(_) if who.local => format!("{p} isn't running. Start it, or pick another brain in Settings."),
        CallError::Unreachable(_) => format!("Can't reach {p}. Check the internet connection."),
        CallError::Status { code: 401 | 403, .. } => format!("{p} refused the API key. Check it in Settings > Brain."),
        CallError::Status { code: 402, .. } => format!("Your {p} account is out of credit."),
        CallError::Status { code: 429, .. } => format!("{p} is busy or rate-limiting. Try again in a moment."),
        CallError::Status { code: 404, .. } => format!("{m} isn't available on {p}. Pick another in Settings > Brain."),
        CallError::Status { code, body } => format!("{p} answered with an error ({code}): {}", server_message(body)),
        CallError::Stream(s) => format!("{p} stopped in the middle of a reply: {s}"),
        CallError::Timeout => format!("No answer from {m} in two minutes."),
        CallError::Cancelled => "Stopped.".into(),
    }
}

/// Where requests go: the base URL (ending in `/v1` or similar) and the key.
#[derive(Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub key: Option<String>,
}

impl Endpoint {
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    pub fn agent(connect: Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(connect))
            .timeout_global(Some(Duration::from_secs(20 * 60)))
            .build()
            .into()
    }

    /// A POST with the key, refused if the key would cross the network in
    /// the clear.
    pub fn post(&self, agent: &ureq::Agent, path: &str) -> Result<ureq::RequestBuilder<ureq::typestate::WithBody>, CallError> {
        let mut r = agent.post(self.url(path));
        if let Some(k) = &self.key {
            if super::providers::plain_http_remote(&self.base_url) {
                return Err(CallError::Unreachable("a key is never sent over plain http to another computer".into()));
            }
            r = r.header("Authorization", format!("Bearer {k}"));
        }
        Ok(r)
    }
}

enum Line {
    Line(String),
    Failed(CallError),
    End,
}

/// One streamed chat request. Runs on a helper thread so `cancel` (Esc)
/// takes effect at once; a server that sends nothing for `idle` (before or
/// between chunks) gives `Timeout`.
pub fn chat(ep: &Endpoint, body: &Value, cancel: &AtomicBool, idle: Duration) -> Result<Reply, CallError> {
    let agent = Endpoint::agent(Duration::from_secs(5));
    let req = ep.post(&agent, "chat/completions")?.header("Accept", "text/event-stream").header("Content-Type", "application/json");
    let body = body.to_string();
    let (tx, rx) = mpsc::channel::<Line>();
    std::thread::Builder::new()
        .name("pindot-loop-http".into())
        .spawn(move || {
            let resp = match req.send(&body) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Line::Failed(CallError::Unreachable(e.to_string())));
                    return;
                }
            };
            let code = resp.status().as_u16();
            if code >= 400 {
                let body = resp.into_body().read_to_string().unwrap_or_default();
                let _ = tx.send(Line::Failed(CallError::Status { code, body: body.chars().take(4000).collect() }));
                return;
            }
            let reader = BufReader::new(resp.into_body().into_reader());
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Line::Line(l)).is_err() {
                            return; // the caller gave up (Esc or a timeout)
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Line::Failed(CallError::Stream(e.to_string())));
                        return;
                    }
                }
            }
            let _ = tx.send(Line::End);
        })
        .map_err(|e| CallError::Stream(e.to_string()))?;

    let mut parser = StreamParser::default();
    let mut plain = String::new();
    let mut last = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CallError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Line::Line(l)) => {
                last = Instant::now();
                if l.starts_with("data:") {
                    parser.line(&l).map_err(CallError::Stream)?;
                    if parser.is_done() {
                        return Ok(parser.finish());
                    }
                } else if !l.starts_with(':') {
                    plain.push_str(&l);
                    plain.push('\n');
                }
            }
            Ok(Line::Failed(e)) => return Err(e),
            Ok(Line::End) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if parser.saw_data() {
                    return Ok(parser.finish());
                }
                return complete_reply(&plain).map_err(CallError::Stream);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last.elapsed() > idle {
                    return Err(CallError::Timeout);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_body_has_every_kind_of_message() {
        let msgs = vec![
            Msg::System("rules".into()),
            Msg::User { text: "hi".into(), images: vec![] },
            Msg::Assistant {
                text: String::new(),
                calls: vec![ToolCall { id: "c1".into(), name: "click".into(), arguments: "{\"ref\":\"r1\"}".into() }],
            },
            Msg::Tool { call_id: "c1".into(), text: "done".into() },
            Msg::ToolImages(vec![Image { mime: "image/png".into(), base64: "AAAA".into() }]),
        ];
        let tools = vec![ToolDef { name: "click".into(), description: "Click".into(), parameters: json!({"type":"object"}) }];
        let b = request_body("m", &msgs, &tools);
        assert_eq!(b["model"], "m");
        assert_eq!(b["stream"], true);
        assert_eq!(b["messages"][0], json!({"role":"system","content":"rules"}));
        assert_eq!(b["messages"][1], json!({"role":"user","content":"hi"}));
        assert_eq!(b["messages"][2]["content"], Value::Null);
        assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["name"], "click");
        assert_eq!(b["messages"][3], json!({"role":"tool","tool_call_id":"c1","content":"done"}));
        assert_eq!(b["messages"][4]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        assert_eq!(b["tools"][0]["function"]["parameters"], json!({"type":"object"}));
        assert!(request_body("m", &msgs[..2], &[]).get("tools").is_none());
    }

    #[test]
    fn a_stream_with_text_and_split_tool_calls_is_put_together() {
        let mut p = StreamParser::default();
        for l in [
            r#"data: {"choices":[{"delta":{"role":"assistant","content":"Open"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"ing it."}}]}"#,
            "",
            ": keep-alive",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"open_app","arguments":"{\"na"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"me\":\"TextEdit\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"name":"wait","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
        ] {
            p.line(l).unwrap();
        }
        assert!(p.is_done());
        let r = p.finish();
        assert_eq!(r.text, "Opening it.");
        assert_eq!(r.calls.len(), 2);
        assert_eq!(r.calls[0], ToolCall { id: "call_a".into(), name: "open_app".into(), arguments: "{\"name\":\"TextEdit\"}".into() });
        assert_eq!(r.calls[1].id, "call_1", "a call without an id gets one");
        assert_eq!(r.finish.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn thinking_is_kept_apart_from_the_reply() {
        let mut p = StreamParser::default();
        p.line(r#"data: {"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#).unwrap();
        p.line(r#"data: {"choices":[{"delta":{"content":"<think>more</think>Done."}}]}"#).unwrap();
        let r = p.finish();
        assert_eq!((r.thinking.as_str(), r.text.as_str()), ("hmm\nmore", "Done."));
        assert_eq!(split_think("<think>only thinking"), ("only thinking".into(), String::new()));
    }

    #[test]
    fn an_error_in_the_stream_is_reported() {
        let mut p = StreamParser::default();
        assert_eq!(p.line(r#"data: {"error":{"message":"model crashed"}}"#), Err("model crashed".into()));
    }

    #[test]
    fn a_reply_that_was_not_streamed_is_read_too() {
        let r = complete_reply(
            r#"{"choices":[{"message":{"content":"Hi","tool_calls":[{"id":"x","type":"function","function":{"name":"wait","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap();
        assert_eq!((r.text.as_str(), r.calls.len()), ("Hi", 1));
    }

    #[test]
    fn errors_are_said_plainly() {
        let local = Who { provider: "Ollama".into(), model: "qwen3:8b".into(), local: true };
        let remote = Who { provider: "OpenAI".into(), model: "gpt-x".into(), local: false };
        let status = |code| CallError::Status { code, body: String::new() };
        assert_eq!(
            describe(&CallError::Unreachable("refused".into()), &local),
            "Ollama isn't running. Start it, or pick another brain in Settings."
        );
        assert_eq!(describe(&status(401), &remote), "OpenAI refused the API key. Check it in Settings > Brain.");
        assert_eq!(describe(&status(402), &remote), "Your OpenAI account is out of credit.");
        assert_eq!(describe(&status(404), &remote), "gpt-x isn't available on OpenAI. Pick another in Settings > Brain.");
        assert_eq!(describe(&CallError::Timeout, &local), "No answer from qwen3:8b in two minutes.");
        assert_eq!(
            describe(&CallError::Status { code: 500, body: r#"{"error":{"message":"out of memory"}}"#.into() }, &local),
            "Ollama answered with an error (500): out of memory"
        );
        assert!(is_context_overflow(&CallError::Status { code: 400, body: "This model's maximum context length is 8192 tokens".into() }));
        assert!(!is_context_overflow(&CallError::Status { code: 400, body: "bad tool".into() }));
    }

    #[test]
    fn a_key_never_goes_to_another_computer_over_plain_http() {
        let ep = Endpoint { base_url: "http://192.168.1.9:8080/v1".into(), key: Some("sk-secret-123456".into()) };
        assert!(matches!(ep.post(&Endpoint::agent(Duration::from_secs(1)), "chat/completions"), Err(CallError::Unreachable(_))));
        assert_eq!(Endpoint { base_url: "http://x/v1/".into(), key: None }.url("/models"), "http://x/v1/models");
    }
}
