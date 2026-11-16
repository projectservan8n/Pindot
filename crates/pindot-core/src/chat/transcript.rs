//! The session transcript (`~/.claude/projects/<slug>/<id>.jsonl`, read
//! only) turned into what the chat shows: the user's messages, the
//! assistant's replies, and one compact "step" per tool call.
//!
//! Verified against `claude` 2.1.325: one content block per line; assistant
//! lines are `text`, `tool_use` (id, name, input) or `thinking`; user lines
//! carry the prompt (string or text blocks) or `tool_result` blocks that
//! point back at a `tool_use` id. In that version the `thinking` block has an
//! empty `text` (only a signature), so thinking is shown only when a block
//! really has text, which today means never.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::attachments::split_prompt;

const SUMMARY_MAX: usize = 140;
const RESULT_MAX: usize = 800;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatItem {
    User { id: String, text: String, files: Vec<String> },
    Assistant { id: String, text: String },
    Thinking { id: String, text: String },
    Step { id: String, name: String, summary: String, result: Option<String>, is_error: bool },
    /// The user pressed Stop.
    Interrupted { id: String },
}

impl ChatItem {
    pub fn id(&self) -> &str {
        match self {
            ChatItem::User { id, .. }
            | ChatItem::Assistant { id, .. }
            | ChatItem::Thinking { id, .. }
            | ChatItem::Step { id, .. }
            | ChatItem::Interrupted { id } => id,
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max { one } else { format!("{}...", one.chars().take(max).collect::<String>()) }
}

fn clip_keep_lines(s: &str, max: usize) -> String {
    let s = s.trim_end();
    if s.chars().count() <= max { s.to_string() } else { format!("{}...", s.chars().take(max).collect::<String>()) }
}

/// The one input that says what a tool call is about.
pub fn summarize_input(name: &str, input: &Value) -> String {
    let get = |k: &str| input.get(k).and_then(Value::as_str);
    let key = match name {
        "Bash" => get("command"),
        "Read" | "Write" | "Edit" => get("file_path"),
        "Grep" | "Glob" => get("pattern"),
        "WebFetch" => get("url"),
        "WebSearch" => get("query"),
        _ => None,
    };
    let key = key
        .or_else(|| get("description"))
        .or_else(|| get("command"))
        .or_else(|| get("file_path"))
        .or_else(|| get("path"))
        .or_else(|| get("query"))
        .or_else(|| get("text"));
    match key {
        Some(k) => clip(k, SUMMARY_MAX),
        None => clip(&input.to_string(), SUMMARY_MAX),
    }
}

fn text_of(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(b) => {
            let t: Vec<&str> = b
                .iter()
                .filter(|x| x.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|x| x.get("text").and_then(Value::as_str))
                .collect();
            (!t.is_empty()).then(|| t.join("\n"))
        }
        _ => None,
    }
}

/// Turn one transcript line into items, in order. `results` are tool results
/// to attach to earlier steps: `(tool_use id, text, is_error)`.
pub fn parse_line(line: &str, n: usize) -> (Vec<ChatItem>, Vec<(String, String, bool)>) {
    let mut items = vec![];
    let mut results = vec![];
    // Most lines are API blobs and bookkeeping; skip them cheaply.
    if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) {
        return (items, results);
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return (items, results) };
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) || v.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return (items, results);
    }
    let base = v.get("uuid").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("line-{n}"));
    let Some(content) = v.pointer("/message/content") else { return (items, results) };
    match v.get("type").and_then(Value::as_str) {
        Some("user") => {
            if let Some(blocks) = content.as_array() {
                for b in blocks {
                    if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                        let text = b.get("content").and_then(text_of).unwrap_or_default();
                        let err = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        results.push((id, clip_keep_lines(&text, RESULT_MAX), err));
                    }
                }
            }
            if results.is_empty() {
                if let Some(t) = text_of(content) {
                    let t = t.trim();
                    if t.starts_with("[Request interrupted by user") {
                        items.push(ChatItem::Interrupted { id: base });
                    } else if !t.is_empty() && !t.starts_with('<') {
                        let t = t.strip_prefix(super::TYPED_NOTE).unwrap_or(t);
                        let (text, paths) = split_prompt(t);
                        let files = paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
                        items.push(ChatItem::User { id: base, text, files });
                    }
                }
            }
        }
        Some("assistant") => {
            if let Some(blocks) = content.as_array() {
                for (i, b) in blocks.iter().enumerate() {
                    let id = if i == 0 { base.clone() } else { format!("{base}-{i}") };
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if !t.is_empty() {
                                items.push(ChatItem::Assistant { id, text: t.to_string() });
                            }
                        }
                        Some("thinking") => {
                            let t = b.get("thinking").and_then(Value::as_str).unwrap_or("").trim();
                            if !t.is_empty() {
                                items.push(ChatItem::Thinking { id, text: t.to_string() });
                            }
                        }
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                            let tid = b.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or(id);
                            let summary = summarize_input(&name, b.get("input").unwrap_or(&Value::Null));
                            items.push(ChatItem::Step { id: tid, name, summary, result: None, is_error: false });
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    (items, results)
}

/// Reads a transcript as it grows. Each `poll` reads only the new bytes
/// (a half-written last line waits for the next poll).
pub struct Reader {
    path: PathBuf,
    offset: u64,
    lines: usize,
    items: Vec<ChatItem>,
    steps: HashMap<String, usize>,
}

impl Reader {
    pub fn new(path: PathBuf) -> Self {
        Reader { path, offset: 0, lines: 0, items: vec![], steps: HashMap::new() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn items(&self) -> &[ChatItem] {
        &self.items
    }

    /// Read what was appended. True if the items changed.
    pub fn poll(&mut self) -> bool {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return false };
        let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
        if len < self.offset {
            // Replaced or truncated: start over.
            *self = Reader::new(self.path.clone());
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut buf).is_err() {
            return false;
        }
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return false };
        self.offset += end as u64 + 1;
        let mut changed = false;
        for line in String::from_utf8_lossy(&buf[..end]).lines() {
            self.lines += 1;
            let (items, results) = parse_line(line, self.lines);
            for it in items {
                if let ChatItem::Step { id, .. } = &it {
                    self.steps.insert(id.clone(), self.items.len());
                }
                self.items.push(it);
                changed = true;
            }
            for (id, text, err) in results {
                if let Some(&i) = self.steps.get(&id) {
                    if let ChatItem::Step { result, is_error, .. } = &mut self.items[i] {
                        *result = Some(text);
                        *is_error = err;
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// Everything in a finished transcript.
pub fn read_all(path: &Path) -> Vec<ChatItem> {
    let mut r = Reader::new(path.to_path_buf());
    r.poll();
    r.items
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        v.to_string()
    }
    fn user(id: &str, content: Value) -> String {
        line(json!({"type":"user","uuid":id,"message":{"role":"user","content":content}}))
    }
    fn asst(id: &str, block: Value) -> String {
        line(json!({"type":"assistant","uuid":id,"message":{"role":"assistant","content":[block]}}))
    }

    #[test]
    fn a_conversation_becomes_items_with_results_on_their_steps() {
        let lines = [
            r#"{"type":"queue-operation","operation":"enqueue"}"#.to_string(),
            user("u1", json!("What is in notes.txt?")),
            asst("a1", json!({"type":"thinking","thinking":"","signature":"abc"})),
            asst("a2", json!({"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"/d/notes.txt"}})),
            user("u2", json!([{"type":"tool_result","tool_use_id":"tu1","content":"the lighthouse is red"}])),
            asst("a3", json!({"type":"text","text":"It says **red**."})),
        ];
        let mut items = vec![];
        let mut steps: Vec<(String, String, bool)> = vec![];
        for (i, l) in lines.iter().enumerate() {
            let (it, r) = parse_line(l, i);
            items.extend(it);
            steps.extend(r);
        }
        assert_eq!(items.len(), 3, "empty thinking and bookkeeping lines are dropped: {items:?}");
        assert_eq!(items[0], ChatItem::User { id: "u1".into(), text: "What is in notes.txt?".into(), files: vec![] });
        assert!(matches!(&items[1], ChatItem::Step { id, name, summary, .. } if id == "tu1" && name == "Read" && summary == "/d/notes.txt"));
        assert_eq!(steps, [("tu1".to_string(), "the lighthouse is red".to_string(), false)]);
        assert_eq!(items[2], ChatItem::Assistant { id: "a3".into(), text: "It says **red**.".into() });
    }

    #[test]
    fn thinking_shows_only_when_it_has_text() {
        let (it, _) = parse_line(&asst("t", json!({"type":"thinking","thinking":"Let me check.","signature":"s"})), 0);
        assert_eq!(it, [ChatItem::Thinking { id: "t".into(), text: "Let me check.".into() }]);
    }

    #[test]
    fn the_typed_note_and_attachment_paths_are_hidden_from_the_user_bubble() {
        let p = format!("{}Look [Attached files, read each with the Read tool: \"/d/attachments/s/a b.txt\"]", super::super::TYPED_NOTE);
        let (it, _) = parse_line(&user("u", json!(p)), 0);
        assert_eq!(it, [ChatItem::User { id: "u".into(), text: "Look".into(), files: vec!["a b.txt".into()] }]);
    }

    #[test]
    fn interrupts_meta_sidechain_and_system_lines() {
        let (it, _) = parse_line(&user("i", json!([{"type":"text","text":"[Request interrupted by user]"}])), 0);
        assert_eq!(it, [ChatItem::Interrupted { id: "i".into() }]);
        let meta = line(json!({"type":"user","isMeta":true,"uuid":"m","message":{"content":"x"}}));
        assert!(parse_line(&meta, 0).0.is_empty());
        let side = line(json!({"type":"assistant","isSidechain":true,"uuid":"s","message":{"content":[{"type":"text","text":"x"}]}}));
        assert!(parse_line(&side, 0).0.is_empty());
        assert!(parse_line(&user("c", json!("<command-name>/clear</command-name>")), 0).0.is_empty());
        assert!(parse_line("not json {\"type\":\"user\"", 0).0.is_empty());
    }

    #[test]
    fn step_summaries_pick_the_useful_input_and_stay_short() {
        assert_eq!(summarize_input("Bash", &json!({"command":"ls  -la\n/tmp"})), "ls -la /tmp");
        assert_eq!(summarize_input("Grep", &json!({"pattern":"fn main","path":"/x"})), "fn main");
        assert_eq!(summarize_input("mcp__desktop__click", &json!({"x":1,"y":2})), r#"{"x":1,"y":2}"#);
        assert!(summarize_input("Bash", &json!({"command":"x".repeat(500)})).chars().count() <= SUMMARY_MAX + 3);
    }

    #[test]
    fn results_are_clipped_and_errors_flagged() {
        let big = "z".repeat(5000);
        let (_, r) = parse_line(&user("u", json!([{"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":big}],"is_error":true}])), 0);
        assert_eq!(r.len(), 1);
        assert!(r[0].1.chars().count() <= RESULT_MAX + 3 && r[0].2);
    }

    #[test]
    fn the_reader_follows_a_growing_file_and_waits_for_whole_lines() {
        let p = std::env::temp_dir().join(format!("pindot-tr-{}.jsonl", uuid::Uuid::new_v4()));
        let l1 = user("u1", json!("hi"));
        let l2 = asst("a1", json!({"type":"tool_use","id":"t1","name":"Bash","input":{"command":"date"}}));
        let l3 = user("u2", json!([{"type":"tool_result","tool_use_id":"t1","content":"Mon"}]));
        std::fs::write(&p, format!("{l1}\n{}", &l2[..20])).unwrap();
        let mut r = Reader::new(p.clone());
        assert!(r.poll());
        assert_eq!(r.items().len(), 1, "the half line waits");
        assert!(!r.poll(), "nothing new");
        std::fs::write(&p, format!("{l1}\n{l2}\n{l3}\n")).unwrap();
        assert!(r.poll());
        assert_eq!(r.items().len(), 2);
        assert!(matches!(&r.items()[1], ChatItem::Step { result: Some(t), .. } if t == "Mon"));
        // A replaced, shorter file starts over.
        std::fs::write(&p, format!("{l1}\n")).unwrap();
        assert!(r.poll());
        assert_eq!(r.items().len(), 1);
        assert_eq!(read_all(&p).len(), 1);
        assert!(read_all(Path::new("/nonexistent/x.jsonl")).is_empty());
    }
}
