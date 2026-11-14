//! Follow a session transcript (`~/.claude/projects/<slug>/<id>.jsonl`) as
//! Claude Code appends to it, and report what matters to the voice loop:
//! typed prompts, assistant text messages, tool calls and their results.
//!
//! Claude Code writes one line per content block, so an assistant message
//! with text and a tool call arrives as two lines; each is reported once.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::transcript::find_transcript;

pub const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEvent {
    /// A prompt typed into the session (not a tool result, not a meta line).
    Prompt(String),
    /// The text of one assistant text block.
    Text(String),
    ToolUse(String),
    ToolResult,
    /// Claude Code recorded "[Request interrupted by user...]".
    Interrupted,
}

/// Turn one transcript line into events.
pub fn parse_line(line: &str) -> Vec<TranscriptEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
    let mut out = vec![];
    let content = v.pointer("/message/content");
    match v.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            for b in content.and_then(Value::as_array).into_iter().flatten() {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                            out.push(TranscriptEvent::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        out.push(TranscriptEvent::ToolUse(
                            b.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        Some("user") => {
            if v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
                return out;
            }
            let text = match content {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(blocks)) => {
                    let mut text = vec![];
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("tool_result") => out.push(TranscriptEvent::ToolResult),
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(Value::as_str) {
                                    text.push(t.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                    if text.is_empty() { None } else { Some(text.join("\n")) }
                }
                _ => None,
            };
            if let Some(t) = text {
                let t = t.trim();
                if t.starts_with("[Request interrupted by user") {
                    out.push(TranscriptEvent::Interrupted);
                } else if !t.is_empty() && !t.starts_with("<command-") && !t.starts_with("<local-command") {
                    out.push(TranscriptEvent::Prompt(t.to_string()));
                }
            }
        }
        _ => {}
    }
    out
}

struct Follow {
    session: Option<String>,
    path: Option<PathBuf>,
    offset: u64,
    partial: String,
    last_lookup: Option<Instant>,
}

pub struct TranscriptTailer {
    follow: Arc<Mutex<Follow>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TranscriptTailer {
    /// Start polling. `on_event(session_id, event)` runs on the tailer thread.
    pub fn start(on_event: impl Fn(&str, TranscriptEvent) + Send + 'static) -> std::io::Result<Self> {
        let follow = Arc::new(Mutex::new(Follow {
            session: None,
            path: None,
            offset: 0,
            partial: String::new(),
            last_lookup: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (follow.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("deskhand-transcript".into()).spawn(move || {
            while !s.load(Ordering::SeqCst) {
                let batch = {
                    let mut f = f.lock().unwrap();
                    poll_once(&mut f)
                };
                if let Some((session, events)) = batch {
                    for e in events {
                        on_event(&session, e);
                    }
                }
                std::thread::sleep(POLL);
            }
        })?;
        Ok(Self { follow, stop, thread: Some(thread) })
    }

    /// Follow `session_id` from the end of what it has now (a resumed session
    /// keeps its history, which must not be replayed). `None` pauses.
    pub fn follow(&self, session_id: Option<&str>) {
        let mut f = self.follow.lock().unwrap();
        if f.session.as_deref() == session_id {
            return;
        }
        f.session = session_id.map(str::to_string);
        f.path = session_id.and_then(find_transcript);
        f.offset = f.path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
        f.partial.clear();
        f.last_lookup = Some(Instant::now());
    }

    /// The transcript file, once Claude Code has created it.
    pub fn path(&self) -> Option<PathBuf> {
        self.follow.lock().unwrap().path.clone()
    }
}

impl Drop for TranscriptTailer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn poll_once(f: &mut Follow) -> Option<(String, Vec<TranscriptEvent>)> {
    let session = f.session.clone()?;
    if f.path.is_none() {
        // The file appears with the first prompt; look it up a few times a second.
        if f.last_lookup.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
            return None;
        }
        f.last_lookup = Some(Instant::now());
        f.path = find_transcript(&session);
        f.offset = 0;
    }
    let path = f.path.clone()?;
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < f.offset {
        // Rewritten or truncated: start over.
        f.offset = 0;
        f.partial.clear();
    }
    if len == f.offset {
        return None;
    }
    file.seek(SeekFrom::Start(f.offset)).ok()?;
    let mut buf = Vec::with_capacity((len - f.offset) as usize);
    file.take(len - f.offset).read_to_end(&mut buf).ok()?;
    f.offset += buf.len() as u64;
    f.partial.push_str(&String::from_utf8_lossy(&buf));
    let mut events = vec![];
    while let Some(nl) = f.partial.find('\n') {
        let line: String = f.partial.drain(..=nl).collect();
        events.extend(parse_line(line.trim_end()));
    }
    Some((session, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompts_text_tools_and_interrupts() {
        let p = parse_line(r#"{"type":"user","message":{"role":"user","content":"open text edit"}}"#);
        assert_eq!(p, vec![TranscriptEvent::Prompt("open text edit".into())]);
        let a = parse_line(
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Opening it."},{"type":"tool_use","name":"mcp__desktop__open_app","input":{}}]}}"#,
        );
        assert_eq!(a, vec![TranscriptEvent::Text("Opening it.".into()), TranscriptEvent::ToolUse("mcp__desktop__open_app".into())]);
        let r = parse_line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#);
        assert_eq!(r, vec![TranscriptEvent::ToolResult]);
        let i = parse_line(r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#);
        assert_eq!(i, vec![TranscriptEvent::Interrupted]);
        assert!(parse_line(r#"{"type":"user","isMeta":true,"message":{"content":"x"}}"#).is_empty());
        assert!(parse_line(r#"{"type":"user","message":{"content":"<command-name>/exit</command-name>"}}"#).is_empty());
        assert!(parse_line("not json").is_empty());
    }

    #[test]
    fn follows_appends_and_split_lines() {
        let dir = std::env::temp_dir().join(format!("dh-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"old\"}}\n").unwrap();
        let mut f = Follow {
            session: Some("s".into()),
            path: Some(path.clone()),
            offset: std::fs::metadata(&path).unwrap().len(),
            partial: String::new(),
            last_lookup: None,
        };
        assert!(poll_once(&mut f).is_none(), "history is not replayed");
        use std::io::Write;
        let mut w = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        w.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"new\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"te").unwrap();
        let (_, ev) = poll_once(&mut f).unwrap();
        assert_eq!(ev, vec![TranscriptEvent::Prompt("new".into())]);
        w.write_all(b"xt\",\"text\":\"Hi.\"}]}}\n").unwrap();
        let (_, ev) = poll_once(&mut f).unwrap();
        assert_eq!(ev, vec![TranscriptEvent::Text("Hi.".into())]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
