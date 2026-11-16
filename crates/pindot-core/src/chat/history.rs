//! Past conversations, for the history list. A conversation is one Claude
//! Code session Pindot started; Pindot keeps a small index of them in the
//! data dir (`chats.json`) so it never has to scan `~/.claude`, which it
//! only reads, one transcript at a time, when a conversation is opened.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

const FILE: &str = "chats.json";
const KEEP: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub session_id: String,
    /// First thing the user said (typed or spoken), short.
    pub title: String,
    /// Unix seconds when the session started / last got a message.
    pub started: u64,
    pub updated: u64,
    /// The transcript, once known.
    pub transcript: Option<PathBuf>,
    /// The folder the session ran in (needed to resume it).
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    pub fn load(data_dir: &Path) -> Index {
        std::fs::read_to_string(data_dir.join(FILE)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        crate::paths::write_private(&data_dir.join(FILE), serde_json::to_string_pretty(self)?.as_bytes())
    }

    /// Note a session (new, or seen again) and, when it has no title yet,
    /// give it one from `first_words`.
    pub fn touch(&mut self, session_id: &str, now: u64, first_words: Option<&str>, transcript: Option<&Path>, cwd: Option<&Path>) {
        let i = match self.entries.iter().position(|e| e.session_id == session_id) {
            Some(i) => i,
            None => {
                self.entries.push(Entry { session_id: session_id.into(), title: String::new(), started: now, updated: now, transcript: None, cwd: None });
                self.entries.len() - 1
            }
        };
        let e = &mut self.entries[i];
        e.updated = now;
        if e.title.is_empty() {
            if let Some(w) = first_words {
                e.title = title_from(w);
            }
        }
        if let Some(t) = transcript {
            e.transcript = Some(t.to_path_buf());
        }
        if let Some(c) = cwd {
            e.cwd = Some(c.to_path_buf());
        }
        self.entries.sort_by(|a, b| b.updated.cmp(&a.updated));
        self.entries.truncate(KEEP);
    }

    /// Newest first, leaving out sessions that never got a message.
    pub fn list(&self) -> Vec<Entry> {
        self.entries.iter().filter(|e| !e.title.is_empty()).cloned().collect()
    }

    pub fn find(&self, session_id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.session_id == session_id)
    }
}

/// A short title from the first words: one line, no attachment note.
pub fn title_from(words: &str) -> String {
    let (text, _) = super::attachments::split_prompt(words);
    let text = text.strip_prefix(super::TYPED_NOTE).unwrap_or(&text);
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= 60 { one } else { format!("{}...", one.chars().take(60).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_titles_once_sorts_newest_first_and_hides_untitled() {
        let mut ix = Index::default();
        ix.touch("a", 10, None, None, None);
        assert!(ix.list().is_empty(), "no message yet");
        ix.touch("a", 11, Some("What's the weather like today"), Some(Path::new("/t/a.jsonl")), Some(Path::new("/c")));
        ix.touch("b", 20, Some("Second chat"), None, None);
        ix.touch("a", 30, Some("ignored later words"), None, None);
        let l = ix.list();
        assert_eq!(l.iter().map(|e| e.session_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(l[0].title, "What's the weather like today");
        assert_eq!(l[0].started, 10);
        assert_eq!(l[0].transcript.as_deref(), Some(Path::new("/t/a.jsonl")));
        assert!(ix.find("zzz").is_none());
    }

    #[test]
    fn titles_drop_the_note_and_the_file_paths_and_stay_short() {
        let w = format!("{}Look at this [Attached files, read each with the Read tool: \"/x/y.txt\"]", super::super::TYPED_NOTE);
        assert_eq!(title_from(&w), "Look at this");
        assert!(title_from(&"word ".repeat(40)).chars().count() <= 63);
    }

    #[test]
    fn the_index_round_trips_and_a_bad_file_is_an_empty_index() {
        let d = std::env::temp_dir().join(format!("pindot-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(Index::load(&d).entries.is_empty());
        let mut ix = Index::default();
        ix.touch("s", 5, Some("hello"), None, None);
        ix.save(&d).unwrap();
        assert_eq!(Index::load(&d).list().len(), 1);
        std::fs::write(d.join(FILE), "{broken").unwrap();
        assert!(Index::load(&d).entries.is_empty());
    }

    #[test]
    fn the_index_is_capped() {
        let mut ix = Index::default();
        for i in 0..KEEP + 20 {
            ix.touch(&format!("s{i}"), i as u64, Some("x"), None, None);
        }
        assert_eq!(ix.entries.len(), KEEP);
        assert_eq!(ix.entries[0].session_id, format!("s{}", KEEP + 19));
    }
}
