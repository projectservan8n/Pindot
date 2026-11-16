//! Reading Claude Code session transcripts
//! (`~/.claude/projects/<project-slug>/<session-id>.jsonl`).
//!
//! Each line is one entry. Assistant messages may span several lines that
//! share `message.id` (one content block per line). User entries carry either
//! the typed prompt (string or text blocks) or `tool_result` blocks.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

/// `<data dir>/conversations`: where Pindot's own loop writes each
/// conversation, in the transcript line format Chat reads.
pub fn conversations_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("conversations")
}

/// Find a session's transcript by id when the hook didn't give us the path:
/// Pindot's own conversation file, else Claude Code's.
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    let data = crate::paths::data_dir().ok();
    let projects = dirs::home_dir().map(|h| h.join(".claude").join("projects"));
    find_transcript_in(session_id, data.as_deref(), projects.as_deref())
}

/// As `find_transcript`, with the folders given: `data_dir/conversations/`
/// first, then every project folder under `projects`.
pub fn find_transcript_in(session_id: &str, data_dir: Option<&Path>, projects: Option<&Path>) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    if let Some(d) = data_dir {
        let own = conversations_dir(d).join(&name);
        if own.is_file() {
            return Some(own);
        }
    }
    for entry in std::fs::read_dir(projects?).ok()?.flatten() {
        let p = entry.path().join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[derive(Debug, Default, Clone)]
pub struct TurnSummary {
    /// The typed prompt that started the last turn.
    pub prompt: Option<String>,
    /// Text of the last assistant message that had text.
    pub final_text: Option<String>,
    /// Tool names called during the last turn, in order.
    pub tool_uses: Vec<String>,
}

fn text_blocks(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let t: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if t.is_empty() { None } else { Some(t.join("\n")) }
        }
        _ => None,
    }
}

fn is_tool_result(content: &Value) -> bool {
    content
        .as_array()
        .map(|b| b.iter().any(|x| x.get("type").and_then(Value::as_str) == Some("tool_result")))
        .unwrap_or(false)
}

/// Summarise the last turn of a transcript.
pub fn last_turn(path: &Path) -> Result<TurnSummary> {
    let raw = std::fs::read_to_string(path)?;
    let entries: Vec<Value> = raw.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();

    // The last real typed prompt starts the turn.
    let start = entries
        .iter()
        .rposition(|e| {
            e.get("type").and_then(Value::as_str) == Some("user")
                && !e.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
                && e.pointer("/message/content").map(|c| !is_tool_result(c) && text_blocks(c).is_some()).unwrap_or(false)
        })
        .unwrap_or(0);

    let mut s = TurnSummary {
        prompt: entries.get(start).and_then(|e| e.pointer("/message/content")).and_then(text_blocks),
        ..Default::default()
    };
    let mut cur_id: Option<String> = None;
    let mut cur_text: Vec<String> = vec![];
    for e in &entries[start..] {
        if e.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let id = e.pointer("/message/id").and_then(Value::as_str).map(str::to_string);
        if id != cur_id {
            if !cur_text.is_empty() {
                s.final_text = Some(cur_text.join("\n"));
            }
            cur_text.clear();
            cur_id = id;
        }
        if let Some(blocks) = e.pointer("/message/content").and_then(Value::as_array) {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            cur_text.push(t.to_string());
                        }
                    }
                    Some("tool_use") => {
                        if let Some(n) = b.get("name").and_then(Value::as_str) {
                            s.tool_uses.push(n.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if !cur_text.is_empty() {
        s.final_text = Some(cur_text.join("\n"));
    }
    Ok(s)
}

/// The transcript is written asynchronously and can lag the Stop hook, so
/// poll briefly until it shows the final text (matching `expected` if given).
pub fn wait_for_last_turn(path: &Path, expected: Option<&str>, timeout: Duration) -> Result<(TurnSummary, bool)> {
    let start = Instant::now();
    loop {
        let s = last_turn(path).unwrap_or_default();
        let matches = match (&s.final_text, expected) {
            (Some(t), Some(exp)) => t.trim() == exp.trim(),
            (Some(_), None) => true,
            _ => false,
        };
        if matches || start.elapsed() >= timeout {
            return Ok((s, matches));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_last_turn_with_tools_and_split_message() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"old prompt"}}"#,
            r#"{"type":"assistant","message":{"id":"m0","role":"assistant","content":[{"type":"text","text":"old answer"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"take a screenshot"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Looking."}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","name":"mcp__desktop__screenshot","input":{}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"img"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Done: I see a button."}]}}"#,
        ];
        let dir = std::env::temp_dir().join(format!("pindot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        std::fs::write(&p, lines.join("\n")).unwrap();
        let s = last_turn(&p).unwrap();
        assert_eq!(s.prompt.as_deref(), Some("take a screenshot"));
        assert_eq!(s.final_text.as_deref(), Some("Done: I see a button."));
        assert_eq!(s.tool_uses, vec!["mcp__desktop__screenshot"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn own_conversations_are_found_before_claude_codes() {
        let base = std::env::temp_dir().join(format!("pindot-find-{}", uuid::Uuid::new_v4()));
        let data = base.join("data");
        let projects = base.join("projects");
        let project = projects.join("some-project");
        std::fs::create_dir_all(conversations_dir(&data)).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        let name = format!("{id}.jsonl");
        assert_eq!(find_transcript_in(id, Some(&data), Some(&projects)), None);
        std::fs::write(project.join(&name), "").unwrap();
        assert_eq!(find_transcript_in(id, Some(&data), Some(&projects)), Some(project.join(&name)));
        std::fs::write(conversations_dir(&data).join(&name), "").unwrap();
        assert_eq!(find_transcript_in(id, Some(&data), Some(&projects)), Some(conversations_dir(&data).join(&name)));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
