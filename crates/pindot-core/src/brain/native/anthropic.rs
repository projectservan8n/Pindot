//! Anthropic's Messages API, the loop's second request format (raw HTTP:
//! Rust has no official Anthropic SDK). The shapes follow the documented
//! API: `POST /v1/messages` with `x-api-key` and `anthropic-version`,
//! streamed server-sent events, tools with `input_schema`. Thinking blocks
//! come back signed and are replayed unchanged (`Msg::Assistant.raw`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{Value, json};

use super::openai::{CallError, Endpoint, Image, Msg, Reply, ToolCall, ToolDef, stream_post};

/// Room for a long answer; streaming keeps a large cap safe.
pub const MAX_TOKENS: u64 = 64_000;
/// Server-side fallbacks: a declined request is retried on another model in
/// the same call.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// What is said when the model (and its fallbacks) declined.
pub const REFUSED: &str = "I can't help with that one.";

/// The models that take `fallbacks: "default"`.
pub fn supports_fallbacks(model: &str) -> bool {
    ["claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5", "claude-fable-5-1", "claude-fable-5-2"].contains(&model)
}

fn image_block(i: &Image) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": i.mime, "data": i.base64}})
}

/// The assistant's blocks: as the API returned them when we have them,
/// else made from the text and the calls.
fn assistant_content(text: &str, calls: &[ToolCall], raw: &Option<Value>) -> Value {
    if let Some(Value::Array(blocks)) = raw
        && !blocks.is_empty()
    {
        return Value::Array(blocks.clone());
    }
    let mut blocks = vec![];
    if !text.is_empty() {
        blocks.push(json!({"type": "text", "text": text}));
    }
    for c in calls {
        let input = serde_json::from_str::<Value>(&c.arguments).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
        blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": input}));
    }
    if blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": "(no reply)"}));
    }
    Value::Array(blocks)
}

pub fn request_body(model: &str, messages: &[Msg], tools: &[ToolDef]) -> Value {
    let mut system = String::new();
    let mut out: Vec<Value> = vec![];
    // Tool results (then the pictures they returned) go in one user message.
    let mut pending: Vec<Value> = vec![];
    fn flush(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({"role": "user", "content": std::mem::take(pending)}));
        }
    }
    for m in messages {
        match m {
            Msg::System(t) => system = t.clone(),
            Msg::User { text, images } => {
                flush(&mut out, &mut pending);
                let content = if images.is_empty() {
                    json!(text)
                } else {
                    let mut parts: Vec<Value> = images.iter().map(image_block).collect();
                    parts.push(json!({"type": "text", "text": text}));
                    Value::Array(parts)
                };
                out.push(json!({"role": "user", "content": content}));
            }
            Msg::Assistant { text, calls, raw } => {
                flush(&mut out, &mut pending);
                out.push(json!({"role": "assistant", "content": assistant_content(text, calls, raw)}));
            }
            Msg::Tool { call_id, text } => pending.push(json!({"type": "tool_result", "tool_use_id": call_id, "content": text})),
            Msg::ToolImages(images) => {
                pending.push(json!({"type": "text", "text": "The picture the tool returned:"}));
                pending.extend(images.iter().map(image_block));
            }
        }
    }
    flush(&mut out, &mut pending);
    let mut b = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "messages": out,
        "cache_control": {"type": "ephemeral"},
    });
    if !system.is_empty() {
        b["system"] = json!(system);
    }
    if !tools.is_empty() {
        b["tools"] = tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters, "eager_input_streaming": true}))
            .collect();
    }
    if supports_fallbacks(model) {
        b["fallbacks"] = json!("default");
    }
    b
}

/// Builds a `Reply` from the Messages API's streamed events, keeping every
/// content block as the API sent it.
#[derive(Default)]
pub struct StreamParser {
    blocks: Vec<Value>,
    partial: Vec<String>,
    stop: Option<String>,
    done: bool,
    seen: bool,
}

impl StreamParser {
    /// One line of the stream. An error event is returned as `Err`.
    pub fn line(&mut self, line: &str) -> Result<(), String> {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { return Ok(()) };
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(()) };
        let index = v.get("index").and_then(Value::as_u64).map(|i| i as usize);
        match v.get("type").and_then(Value::as_str) {
            Some("message_start") => self.seen = true,
            Some("content_block_start") => {
                let i = index.unwrap_or(self.blocks.len());
                while self.blocks.len() <= i {
                    self.blocks.push(Value::Null);
                    self.partial.push(String::new());
                }
                self.blocks[i] = v.get("content_block").cloned().unwrap_or(Value::Null);
            }
            Some("content_block_delta") => {
                let (Some(i), Some(d)) = (index, v.get("delta")) else { return Ok(()) };
                let Some(block) = self.blocks.get_mut(i) else { return Ok(()) };
                let add = |block: &mut Value, field: &str, more: Option<&str>| {
                    if let Some(more) = more {
                        let now = block.get(field).and_then(Value::as_str).unwrap_or("").to_string();
                        block[field] = json!(now + more);
                    }
                };
                match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => add(block, "text", d.get("text").and_then(Value::as_str)),
                    Some("thinking_delta") => add(block, "thinking", d.get("thinking").and_then(Value::as_str)),
                    Some("signature_delta") => add(block, "signature", d.get("signature").and_then(Value::as_str)),
                    Some("input_json_delta") => {
                        if let Some(p) = d.get("partial_json").and_then(Value::as_str) {
                            self.partial[i].push_str(p);
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let Some(i) = index
                    && let Some(block) = self.blocks.get_mut(i)
                    && block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && !self.partial[i].trim().is_empty()
                {
                    // An input cut short stays as text for the call; the
                    // replayed block gets an empty object.
                    block["input"] = serde_json::from_str::<Value>(&self.partial[i]).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
                }
            }
            Some("message_delta") => {
                if let Some(s) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(s.to_string());
                }
            }
            Some("message_stop") => self.done = true,
            Some("error") => {
                return Err(v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the server reported an error").to_string());
            }
            _ => {}
        }
        Ok(())
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn saw_data(&self) -> bool {
        self.seen
    }

    pub fn finish(self) -> Reply {
        let StreamParser { blocks, partial, stop, .. } = self;
        let mut kept = vec![];
        let mut args = vec![];
        for (b, p) in blocks.into_iter().zip(partial) {
            if !b.is_null() {
                args.push(p);
                kept.push(b);
            }
        }
        reply_from(kept, args, stop)
    }
}

/// A `Reply` from content blocks (and, per block, the tool input text as
/// streamed, which may be empty).
fn reply_from(blocks: Vec<Value>, args: Vec<String>, stop: Option<String>) -> Reply {
    let mut text = vec![];
    let mut thinking = vec![];
    let mut calls = vec![];
    for (b, a) in blocks.iter().zip(args.iter().chain(std::iter::repeat(&String::new()))) {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => text.push(b.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
            Some("thinking") => thinking.push(b.get("thinking").and_then(Value::as_str).unwrap_or("").to_string()),
            Some("tool_use") => calls.push(ToolCall {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                arguments: if a.trim().is_empty() { b.get("input").cloned().unwrap_or_else(|| json!({})).to_string() } else { a.clone() },
            }),
            _ => {}
        }
    }
    let mut text = text.join("").trim().to_string();
    if stop.as_deref() == Some("refusal") && text.is_empty() {
        text = REFUSED.to_string();
    }
    Reply {
        text,
        thinking: thinking.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
        calls,
        finish: stop,
        raw: Some(Value::Array(blocks)),
    }
}

/// A reply sent whole (a server that ignored `stream`).
pub fn complete_reply(body: &str) -> Result<Reply, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the server's answer was not understood".to_string())?;
    if let Some(e) = v.get("error") {
        return Err(e.get("message").and_then(Value::as_str).unwrap_or("the server reported an error").to_string());
    }
    let blocks = v.get("content").and_then(Value::as_array).cloned().ok_or("the server's answer had no content")?;
    let n = blocks.len();
    Ok(reply_from(blocks, vec![String::new(); n], v.get("stop_reason").and_then(Value::as_str).map(str::to_string)))
}

/// One streamed Messages API request.
pub fn chat(ep: &Endpoint, body: &Value, cancel: &AtomicBool, idle: Duration) -> Result<Reply, CallError> {
    let agent = Endpoint::agent(Duration::from_secs(5));
    let mut req = ep.post(&agent, "messages")?;
    if body.get("fallbacks").is_some() {
        req = req.header("anthropic-beta", FALLBACK_BETA);
    }
    let mut parser = StreamParser::default();
    let plain = stream_post(req, body.to_string(), cancel, idle, &mut |l| {
        parser.line(l)?;
        Ok(parser.is_done())
    })?;
    if parser.saw_data() { Ok(parser.finish()) } else { complete_reply(&plain).map_err(CallError::Stream) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::native::openai::ToolCall;

    fn img() -> Image {
        Image { mime: "image/png".into(), base64: "AAAA".into() }
    }

    #[test]
    fn the_request_body_follows_the_messages_api() {
        let raw = json!([{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "tool_use", "id": "t1", "name": "click", "input": {"ref": "r1"}}]);
        let msgs = vec![
            Msg::System("rules".into()),
            Msg::User { text: "look".into(), images: vec![img()] },
            Msg::Assistant { text: String::new(), calls: vec![ToolCall { id: "t1".into(), name: "click".into(), arguments: "{\"ref\":\"r1\"}".into() }], raw: Some(raw.clone()) },
            Msg::Tool { call_id: "t1".into(), text: "done".into() },
            Msg::ToolImages(vec![img()]),
            Msg::Assistant { text: "Clicked.".into(), calls: vec![], raw: None },
        ];
        let tools = vec![ToolDef { name: "click".into(), description: "Click".into(), parameters: json!({"type": "object"}) }];
        let b = request_body("claude-opus-5-5", &msgs, &tools);
        assert_eq!(b["system"], "rules");
        assert_eq!(b["stream"], true);
        assert_eq!(b["max_tokens"], MAX_TOKENS);
        assert_eq!(b["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(b["fallbacks"], "default");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 4, "{m:?}");
        assert_eq!(m[0]["content"][0]["source"], json!({"type": "base64", "media_type": "image/png", "data": "AAAA"}));
        assert_eq!(m[0]["content"][1], json!({"type": "text", "text": "look"}));
        assert_eq!(m[1]["content"], raw, "replayed unchanged");
        assert_eq!(m[2]["content"][0], json!({"type": "tool_result", "tool_use_id": "t1", "content": "done"}));
        assert_eq!(m[2]["content"][2]["type"], "image", "the tool's picture follows the result in the same message");
        assert_eq!(m[3], json!({"role": "assistant", "content": [{"type": "text", "text": "Clicked."}]}));
        assert_eq!(b["tools"][0], json!({"name": "click", "description": "Click", "input_schema": {"type": "object"}, "eager_input_streaming": true}));
        assert!(request_body("claude-haiku-4-5", &msgs[..2], &[]).get("fallbacks").is_none());
        assert!(request_body("claude-haiku-4-5", &msgs[..2], &[]).get("tools").is_none());
    }

    #[test]
    fn a_stream_with_thinking_text_and_a_tool_call_is_put_together() {
        let mut p = StreamParser::default();
        for l in [
            r#"data: {"type":"message_start","message":{"id":"msg_1","content":[]}}"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Plan."}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG"}}"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Opening "}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"it."}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"open_app","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"name\":"}}"#,
            r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"TextEdit\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":2}"#,
            r#"data: {"type":"ping"}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
            r#"data: {"type":"message_stop"}"#,
        ] {
            p.line(l).unwrap();
        }
        assert!(p.is_done());
        let r = p.finish();
        assert_eq!((r.text.as_str(), r.thinking.as_str()), ("Opening it.", "Plan."));
        assert_eq!(r.calls, vec![ToolCall { id: "toolu_1".into(), name: "open_app".into(), arguments: "{\"name\":\"TextEdit\"}".into() }]);
        assert_eq!(r.finish.as_deref(), Some("tool_use"));
        let raw = r.raw.unwrap();
        assert_eq!(raw[0], json!({"type": "thinking", "thinking": "Plan.", "signature": "SIG"}));
        assert_eq!(raw[2]["input"], json!({"name": "TextEdit"}));
    }

    #[test]
    fn a_refusal_and_an_error_are_said_plainly() {
        let mut p = StreamParser::default();
        p.line(r#"data: {"type":"message_start","message":{}}"#).unwrap();
        p.line(r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal"}}"#).unwrap();
        p.line(r#"data: {"type":"message_stop"}"#).unwrap();
        assert_eq!(p.finish().text, REFUSED);
        let mut p = StreamParser::default();
        assert_eq!(p.line(r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#), Err("Overloaded".into()));
    }

    #[test]
    fn a_reply_that_was_not_streamed_is_read_too() {
        let r = complete_reply(r#"{"content":[{"type":"text","text":"Hi"},{"type":"tool_use","id":"t","name":"wait","input":{}}],"stop_reason":"tool_use"}"#).unwrap();
        assert_eq!((r.text.as_str(), r.calls.len()), ("Hi", 1));
        assert_eq!(r.calls[0].arguments, "{}");
    }
}
