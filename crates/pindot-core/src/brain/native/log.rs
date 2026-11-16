//! The conversation file of Pindot's own loop: one JSON line per message,
//! in the subset of Claude Code's transcript format that
//! `chat::transcript::parse_line` reads, so Chat and History show these
//! conversations exactly as they show Claude Code's. Pictures are never
//! written.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use super::openai::ToolCall;

/// Tool names are written as Claude Code writes MCP tools.
pub const TOOL_PREFIX: &str = "mcp__desktop__";
pub const INTERRUPTED: &str = "[Request interrupted by user]";

pub struct ConversationLog {
    path: PathBuf,
    session: String,
}

impl ConversationLog {
    /// `dir` is `brain::transcript::conversations_dir(data dir)`.
    pub fn create(dir: &Path, session_id: &str) -> Result<Self> {
        crate::paths::ensure_private_dir(dir)?;
        let path = dir.join(format!("{session_id}.jsonl"));
        let mut o = OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&path)?;
        Ok(Self { path, session: session_id.to_string() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, kind: &str, content: Value) {
        let line = json!({
            "type": kind,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "sessionId": self.session,
            "message": {"role": kind, "content": content},
        });
        let ok = OpenOptions::new().append(true).open(&self.path).and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = ok {
            tracing::warn!("conversation file not written: {e}");
        }
    }

    pub fn user(&self, text: &str) {
        self.write("user", json!(text));
    }

    pub fn assistant(&self, thinking: &str, text: &str, calls: &[ToolCall]) {
        let mut blocks = vec![];
        if !thinking.trim().is_empty() {
            blocks.push(json!({"type": "thinking", "thinking": thinking}));
        }
        if !text.trim().is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        for c in calls {
            let input = serde_json::from_str::<Value>(&c.arguments).unwrap_or_else(|_| json!({"raw": c.arguments}));
            blocks.push(json!({"type": "tool_use", "id": c.id, "name": format!("{TOOL_PREFIX}{}", c.name), "input": input}));
        }
        if !blocks.is_empty() {
            self.write("assistant", Value::Array(blocks));
        }
    }

    pub fn tool_result(&self, call_id: &str, text: &str, is_error: bool) {
        self.write("user", json!([{"type": "tool_result", "tool_use_id": call_id, "content": text, "is_error": is_error}]));
    }

    pub fn interrupted(&self) {
        self.write("user", json!(INTERRUPTED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::transcript::{ChatItem, read_all};

    #[test]
    fn every_line_reads_back_as_chat_items() {
        let dir = std::env::temp_dir().join(format!("pindot-log-{}", uuid::Uuid::new_v4()));
        let log = ConversationLog::create(&dir, "s1").unwrap();
        log.user("Open TextEdit");
        log.assistant(
            "I'll check.",
            "Opening it.",
            &[ToolCall { id: "c1".into(), name: "open_app".into(), arguments: "{\"name\":\"TextEdit\"}".into() }],
        );
        log.tool_result("c1", "Opened TextEdit.", false);
        log.assistant("", "Done.", &[]);
        log.interrupted();
        let items = read_all(log.path());
        assert!(matches!(&items[0], ChatItem::User { text, .. } if text == "Open TextEdit"), "{items:?}");
        assert!(matches!(&items[1], ChatItem::Thinking { text, .. } if text == "I'll check."), "{items:?}");
        assert!(matches!(&items[2], ChatItem::Assistant { text, .. } if text == "Opening it."));
        assert!(
            matches!(&items[3], ChatItem::Step { name, result: Some(r), .. } if name == "mcp__desktop__open_app" && r == "Opened TextEdit."),
            "{items:?}"
        );
        assert!(matches!(&items[4], ChatItem::Assistant { text, .. } if text == "Done."));
        assert!(matches!(&items[5], ChatItem::Interrupted { .. }));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
