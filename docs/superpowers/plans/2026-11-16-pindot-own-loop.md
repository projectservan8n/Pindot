# Pindot's own loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Pindot use a local model (Ollama, LM Studio, llama.cpp) or an OpenAI-compatible API (OpenAI, OpenRouter, DeepSeek, Groq, any other) as its brain, through a loop Pindot runs itself, with Claude Code unchanged as the default.

**Architecture:** A `Brain` trait (send, interrupt, new session) sits between the voice loop/Chat and the brain. `PersistentBrain` (Claude Code) implements it unchanged; the new `LoopBrain` (`crates/pindot-core/src/brain/native/`) streams OpenAI Chat Completions with `ureq`, calls the desktop tools as an MCP client of Pindot's own server (so approvals, Esc, the overlay and the activity log apply unchanged), and writes each conversation in the transcript line format Chat already reads.

**Tech Stack:** Rust 2024 (`ureq` 3.5, `serde_json`, `keyring-core`, `url`, `base64`, `axum` for test servers), Tauri 2 commands, Svelte 5 + TypeScript.

**Spec:** `docs/superpowers/specs/2026-11-16-pindot-own-loop-design.md`.

**Conventions for every task:**
- `export PATH="$HOME/.cargo/bin:$PATH"` before cargo. Send long output to a log and read the tail.
- Before using a crate API not shown here, read its source under `~/.cargo/registry/src` (CLAUDE.md rule). `ureq` 3.5.2: `Agent::config_builder()...build().into()`, `agent.post(url).header(k, v).send_json(v)`, `resp.status().as_u16()`, `resp.into_body().into_reader()`, `body.read_to_string()`.
- Commits: `git -c user.name="Tony Herrera" -c user.email=tony@opusautomations.com commit`, message ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- TDD: write the test, run it and see it fail (compile errors for missing items count), then implement, then run it green.

---

## File map

Create (all under `crates/pindot-core/src/brain/native/`):

| File | Responsibility |
|---|---|
| `mod.rs` | `LoopBrain`: worker thread, sessions, availability retries, `Brain` impl. |
| `providers.rs` | Provider presets; key-prefix warnings; plain-http check. |
| `settings.rs` | `BrainSettings` (the `brain` block of settings.json). |
| `secrets.rs` | API keys in the OS credential store (`SecretStore`). |
| `openai.rs` | Message types, request body, SSE parser, one streamed HTTP call, error wording. |
| `mcp_client.rs` | MCP client of Pindot's own server: list tools, call a tool. |
| `context.rs` | Token estimate and trimming to fit the context. |
| `log.rs` | Conversation file in Claude Code's transcript line format. |
| `run.rs` | One turn: model, tool calls, results, repeat. |
| `probe.rs` | Find local servers, list models, read capabilities. |
| `test_model.rs` | The two-turn model test. |

Create elsewhere: `crates/pindot-core/tests/native_loop.rs` (end-to-end with a fake model server and Pindot's real server), `app/src-tauri/src/brain.rs` (Tauri commands), `app/src/lib/BrainPicker.svelte`.

Modify: `brain/mod.rs` (trait), `brain/persistent.rs` (impl), `brain/transcript.rs` (find own conversations), `voice/runner.rs` (`Box<dyn Brain>`), `voice/plan.rs` (loop facts), `voice/machine.rs` (neutral wording), `settings.rs` (`brain` field), `app/src-tauri/src/{voice.rs,ui.rs,lib.rs}`, `app/src/lib/api.ts`, `app/src/views/{Setup,Settings}.svelte`, `app/src/App.svelte`, `CLAUDE.md`, `CONTRIBUTING.md`, `README.md`, `docs/development.md`.

---

### Task 1: The `Brain` trait

**Files:** Modify `crates/pindot-core/src/brain/mod.rs`, `crates/pindot-core/src/brain/persistent.rs`, `crates/pindot-core/src/voice/runner.rs:18,316`, `app/src-tauri/src/voice.rs:433`.

- [ ] **Step 1: Write the failing test** in the `tests` module of `brain/mod.rs`:

```rust
    #[test]
    fn a_brain_is_driven_through_the_trait() {
        use std::sync::Mutex;
        #[derive(Default)]
        struct Rec(Mutex<Vec<String>>);
        impl super::Brain for Rec {
            fn send(&self, text: &str) {
                self.0.lock().unwrap().push(format!("send {text}"));
            }
            fn interrupt(&self) {
                self.0.lock().unwrap().push("interrupt".into());
            }
            fn new_session(&self) {
                self.0.lock().unwrap().push("new".into());
            }
        }
        let rec = std::sync::Arc::new(Rec::default());
        let b: Box<dyn super::Brain> = Box::new(ArcBrain(rec.clone()));
        b.send("hi");
        b.interrupt();
        b.new_session();
        assert_eq!(*rec.0.lock().unwrap(), vec!["send hi", "interrupt", "new"]);

        struct ArcBrain(std::sync::Arc<Rec>);
        impl super::Brain for ArcBrain {
            fn send(&self, t: &str) {
                self.0.send(t)
            }
            fn interrupt(&self) {
                self.0.interrupt()
            }
            fn new_session(&self) {
                self.0.new_session()
            }
        }
    }
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib a_brain_is_driven` → FAIL (`Brain` not found).

- [ ] **Step 3: Implement.** In `brain/mod.rs`, after the `pub mod` lines:

```rust
/// What the voice loop and Chat need from a brain: the user's Claude Code
/// (`persistent::PersistentBrain`) or Pindot's own loop
/// (`native::LoopBrain`). Both report back through `persistent::InputSink`
/// with `voice::machine::Input`s.
pub trait Brain: Send {
    fn send(&self, text: &str);
    fn interrupt(&self);
    fn new_session(&self);
}
```

At the end of `persistent.rs` (before its tests, if any):

```rust
impl super::Brain for PersistentBrain {
    fn send(&self, text: &str) {
        PersistentBrain::send(self, text)
    }
    fn interrupt(&self) {
        PersistentBrain::interrupt(self)
    }
    fn new_session(&self) {
        PersistentBrain::new_session(self)
    }
}
```

In `voice/runner.rs`: replace `use crate::brain::persistent::PersistentBrain;` with `use crate::brain::Brain;` and the field with `pub brain: Box<dyn Brain>,`. In `app/src-tauri/src/voice.rs` line 433: `brain: Box::new(brain),`.

- [ ] **Step 4: Run** `cargo test -p pindot-core --lib a_brain_is_driven` → PASS; `cargo build -p pindot` → no errors or warnings.

- [ ] **Step 5: Commit** "A Brain trait between the voice loop and Claude Code".

### Task 2: Chat finds Pindot's own conversation files

**Files:** Modify `crates/pindot-core/src/brain/transcript.rs:15-25`.

- [ ] **Step 1: Failing test** in `brain/transcript.rs` tests (create `#[cfg(test)] mod tests` if missing, with `use super::*;`):

```rust
    #[test]
    fn own_conversations_are_found_before_claude_codes() {
        let base = std::env::temp_dir().join(format!("pindot-find-{}", uuid::Uuid::new_v4()));
        let data = base.join("data");
        let projects = base.join("projects").join("some-project");
        std::fs::create_dir_all(conversations_dir(&data)).unwrap();
        std::fs::create_dir_all(&projects).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        assert_eq!(find_transcript_in(id, Some(&data), Some(&base.join("projects"))), None);
        std::fs::write(projects.join(format!("{id}.jsonl")), "").unwrap();
        assert_eq!(find_transcript_in(id, Some(&data), Some(&base.join("projects"))), Some(projects.join(format!("{id}.jsonl"))));
        std::fs::write(conversations_dir(&data).join(format!("{id}.jsonl")), "").unwrap();
        assert_eq!(
            find_transcript_in(id, Some(&data), Some(&base.join("projects"))),
            Some(conversations_dir(&data).join(format!("{id}.jsonl")))
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib own_conversations_are_found` → FAIL.

- [ ] **Step 3: Implement** — replace `find_transcript` with:

```rust
/// `<data dir>/conversations`: where Pindot's own loop writes each
/// conversation, in the transcript line format Chat reads.
pub fn conversations_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("conversations")
}

/// A session's transcript: Pindot's own conversation file, else Claude
/// Code's under `~/.claude/projects/<any project>/`.
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    let data = crate::paths::data_dir().ok();
    let projects = dirs::home_dir().map(|h| h.join(".claude").join("projects"));
    find_transcript_in(session_id, data.as_deref(), projects.as_deref())
}

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
```

Add `use std::path::Path;` if missing.

- [ ] **Step 4: Run** the test → PASS. **Step 5: Commit** "Chat finds Pindot's own conversation files".

### Task 3: Provider presets

**Files:** Create `crates/pindot-core/src/brain/native/mod.rs` (for now only `pub mod providers;`), `.../native/providers.rs`; modify `brain/mod.rs` (`pub mod native;`).

- [ ] **Step 1: Write `providers.rs` with its tests** (tests first in the file; the functions are written in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_presets_are_loopback_http_and_need_no_key() {
        for p in PROVIDERS.iter().filter(|p| p.local) {
            assert!(p.base_url.starts_with("http://127.0.0.1:"), "{}", p.id);
            assert!(!p.needs_key && p.key_prefix.is_none(), "{}", p.id);
        }
        assert_eq!(find("ollama").unwrap().base_url, "http://127.0.0.1:11434/v1");
        assert!(find("nope").is_none());
    }

    #[test]
    fn a_key_for_another_provider_is_caught() {
        let openai = find("openai").unwrap();
        let openrouter = find("openrouter").unwrap();
        assert_eq!(key_warning(openai, "sk-proj-abc"), None);
        assert_eq!(key_warning(openai, "sk-or-v1-abc"), Some("This looks like an OpenRouter key, not an OpenAI one.".into()));
        assert_eq!(key_warning(openrouter, "sk-proj-abc"), Some("OpenRouter keys usually start with \"sk-or-\".".into()));
        assert_eq!(key_warning(find("groq").unwrap(), "gsk_x"), None);
        assert_eq!(key_warning(find("custom").unwrap(), "anything"), None);
    }

    #[test]
    fn plain_http_to_another_computer_is_spotted() {
        assert!(!plain_http_remote("http://127.0.0.1:11434/v1"));
        assert!(!plain_http_remote("http://localhost:1234/v1"));
        assert!(!plain_http_remote("http://[::1]:8080/v1"));
        assert!(!plain_http_remote("https://api.openai.com/v1"));
        assert!(plain_http_remote("http://192.168.1.20:11434/v1"));
    }
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::providers` → FAIL.

- [ ] **Step 3: Implement** above the tests:

```rust
//! The providers Pindot's own loop knows: local servers and API services
//! that speak OpenAI's Chat Completions. Check a URL against the
//! provider's current documentation before changing it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Empty for Custom.
    pub base_url: &'static str,
    /// Runs on this computer: looked for on loopback, no key.
    pub local: bool,
    pub needs_key: bool,
    /// How its keys usually start, to catch a key pasted for another one.
    pub key_prefix: Option<&'static str>,
}

const fn p(id: &'static str, name: &'static str, base_url: &'static str, local: bool, key_prefix: Option<&'static str>) -> Provider {
    Provider { id, name, base_url, local, needs_key: key_prefix.is_some(), key_prefix }
}

pub const PROVIDERS: &[Provider] = &[
    p("ollama", "Ollama", "http://127.0.0.1:11434/v1", true, None),
    p("lmstudio", "LM Studio", "http://127.0.0.1:1234/v1", true, None),
    p("llamacpp", "llama.cpp", "http://127.0.0.1:8080/v1", true, None),
    p("openai", "OpenAI", "https://api.openai.com/v1", false, Some("sk-")),
    p("openrouter", "OpenRouter", "https://openrouter.ai/api/v1", false, Some("sk-or-")),
    p("deepseek", "DeepSeek", "https://api.deepseek.com/v1", false, Some("sk-")),
    p("groq", "Groq", "https://api.groq.com/openai/v1", false, Some("gsk_")),
    p("custom", "Other (OpenAI-compatible)", "", false, None),
];

pub fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn article(name: &str) -> &'static str {
    if name.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" }
}

/// A warning when `key` looks like it belongs to another provider, or not
/// like this provider's keys at all.
pub fn key_warning(provider: &Provider, key: &str) -> Option<String> {
    let key = key.trim();
    let mine = provider.key_prefix.filter(|k| key.starts_with(k)).map_or(0, str::len);
    let matches: Vec<(usize, &Provider)> =
        PROVIDERS.iter().filter_map(|p| p.key_prefix.filter(|k| key.starts_with(k)).map(|k| (k.len(), p))).collect();
    let best = matches.iter().map(|m| m.0).max().unwrap_or(0);
    let owners: Vec<&Provider> = matches.iter().filter(|m| m.0 == best).map(|m| m.1).collect();
    if best > mine && owners.len() == 1 && owners[0].id != provider.id {
        let o = owners[0];
        return Some(format!(
            "This looks like {} {} key, not {} {} one.",
            article(o.name),
            o.name,
            article(provider.name),
            provider.name
        ));
    }
    match provider.key_prefix {
        Some(k) if mine == 0 => Some(format!("{} keys usually start with \"{k}\".", provider.name)),
        _ => None,
    }
}

/// An `http://` address of another computer: a key must never go there in
/// the clear.
pub fn plain_http_remote(base_url: &str) -> bool {
    let Ok(u) = url::Url::parse(base_url.trim()) else { return false };
    if u.scheme() != "http" {
        return false;
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) => !ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => !ip.is_loopback(),
        Some(url::Host::Domain(d)) => d != "localhost",
        None => true,
    }
}
```

- [ ] **Step 4: Run** → PASS. **Step 5: Commit** "Provider presets for Pindot's own loop".

### Task 4: `BrainSettings` in settings.json

**Files:** Create `native/settings.rs`; modify `native/mod.rs` (`pub mod settings;`), `crates/pindot-core/src/settings.rs` (field, default, validate, tests).

- [ ] **Step 1: Failing tests.** In `native/settings.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn loop_brain() -> BrainSettings {
        BrainSettings { kind: BrainKind::Loop, model: "qwen3:8b".into(), ..BrainSettings::default() }
    }

    #[test]
    fn claude_code_needs_no_loop_fields() {
        assert!(BrainSettings::default().problems().is_empty());
        assert!(!BrainSettings::default().is_loop());
    }

    #[test]
    fn a_loop_brain_is_checked() {
        assert!(loop_brain().problems().is_empty(), "{:?}", loop_brain().problems());
        let b = BrainSettings { model: " ".into(), base_url: "ftp://x".into(), provider: "nope".into(), context_tokens: Some(10), ..loop_brain() };
        assert_eq!(b.problems().len(), 4, "{:?}", b.problems());
    }

    #[test]
    fn test_results_are_kept_per_model_newest_first() {
        let mut b = loop_brain();
        for i in 0..25 {
            b.record_test(TestResult { base_url: "u".into(), model: format!("m{i}"), ok: true, note: String::new(), at: i });
        }
        assert_eq!(b.tested.len(), TESTS_KEPT);
        assert_eq!(b.tested[0].model, "m24");
        b.record_test(TestResult { base_url: "u".into(), model: "m24".into(), ok: false, note: "no tools".into(), at: 99 });
        assert_eq!(b.tested.len(), TESTS_KEPT);
        assert!(!b.test_for("u", "m24").unwrap().ok);
    }

    #[test]
    fn only_brain_fields_restart_the_loop() {
        let a = loop_brain();
        let mut b = a.clone();
        b.record_test(TestResult { base_url: "u".into(), model: "m".into(), ok: true, note: String::new(), at: 1 });
        assert!(a.same_brain(&b));
        b.model = "other".into();
        assert!(!a.same_brain(&b));
    }
}
```

In `settings.rs` tests:

```rust
    #[test]
    fn a_file_without_a_brain_block_means_claude_code() {
        let dir = temp_dir("nobrain");
        fs::write(Settings::path_in(&dir), r#"{"version":1,"port":47823}"#).unwrap();
        let (s, _) = Settings::load_in(&dir).unwrap();
        assert!(!s.brain.is_loop());
        let mut s = s;
        s.brain.kind = crate::brain::native::settings::BrainKind::Loop;
        s.brain.model = "qwen3:8b".into();
        s.save_in(&dir).unwrap();
        assert_eq!(Settings::load_in(&dir).unwrap().0.brain, s.brain);
        s.brain.model.clear();
        assert!(s.validate().unwrap_err().iter().any(|p| p.contains("choose a model")));
        fs::remove_dir_all(&dir).unwrap();
    }
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib settings` → FAIL.

- [ ] **Step 3: Implement** `native/settings.rs` above its tests:

```rust
//! The `brain` block of settings.json: which brain Pindot uses. A file
//! without the block means Claude Code, so no migration is needed.

use serde::{Deserialize, Serialize};

use super::providers;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrainKind {
    /// The user's Claude Code: interactive `claude` in a PTY.
    #[default]
    ClaudeCode,
    /// Pindot's own loop against an OpenAI-compatible endpoint.
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Vision {
    /// Ask the server; unknown means no.
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub base_url: String,
    pub model: String,
    pub ok: bool,
    /// One sentence on what failed; empty when it passed.
    pub note: String,
    /// Seconds since 1970.
    pub at: u64,
}

pub const TESTS_KEPT: usize = 20;
pub const CONTEXT_MIN: u32 = 2048;
pub const CONTEXT_MAX: u32 = 2_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrainSettings {
    pub kind: BrainKind,
    /// A `providers::PROVIDERS` id.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub vision: Vision,
    /// `None`: from the server, else a default.
    pub context_tokens: Option<u32>,
    /// One result per base URL and model, newest first.
    pub tested: Vec<TestResult>,
}

impl Default for BrainSettings {
    fn default() -> Self {
        Self {
            kind: BrainKind::ClaudeCode,
            provider: "ollama".into(),
            base_url: providers::find("ollama").map(|p| p.base_url.to_string()).unwrap_or_default(),
            model: String::new(),
            vision: Vision::Auto,
            context_tokens: None,
            tested: vec![],
        }
    }
}

impl BrainSettings {
    pub fn is_loop(&self) -> bool {
        self.kind == BrainKind::Loop
    }

    /// The same brain (test results aside): anything else restarts the
    /// voice loop with a fresh conversation.
    pub fn same_brain(&self, o: &BrainSettings) -> bool {
        (self.kind, &self.provider, &self.base_url, &self.model, self.vision, self.context_tokens)
            == (o.kind, &o.provider, &o.base_url, &o.model, o.vision, o.context_tokens)
    }

    /// What is wrong with a loop brain, in words for the user. Nothing for
    /// Claude Code.
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if !self.is_loop() {
            return out;
        }
        let provider = providers::find(&self.provider);
        if provider.is_none() {
            out.push(format!("Brain: unknown provider \"{}\"", self.provider));
        }
        match url::Url::parse(self.base_url.trim()) {
            Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {}
            _ => out.push("Brain: the address must start with http:// or https://".into()),
        }
        if provider.is_some_and(|p| p.needs_key) && providers::plain_http_remote(&self.base_url) {
            out.push("Brain: use an https:// address for a provider that needs a key".into());
        }
        if self.model.trim().is_empty() {
            out.push("Brain: choose a model".into());
        }
        if let Some(c) = self.context_tokens
            && !(CONTEXT_MIN..=CONTEXT_MAX).contains(&c)
        {
            out.push(format!("Brain: the context size must be from {CONTEXT_MIN} to {CONTEXT_MAX} tokens"));
        }
        out
    }

    pub fn test_for(&self, base_url: &str, model: &str) -> Option<&TestResult> {
        self.tested.iter().find(|t| t.base_url == base_url && t.model == model)
    }

    pub fn record_test(&mut self, r: TestResult) {
        self.tested.retain(|t| !(t.base_url == r.base_url && t.model == r.model));
        self.tested.insert(0, r);
        self.tested.truncate(TESTS_KEPT);
    }
}
```

In `settings.rs`: add the field after `voice` —

```rust
    /// Which brain answers: Claude Code (the default) or Pindot's own loop.
    pub brain: crate::brain::native::settings::BrainSettings,
```

— `brain: Default::default(),` in `Default`, and in `validate()` before the final `if problems.is_empty()`: `problems.extend(self.brain.problems());`.

- [ ] **Step 4: Run** `cargo test -p pindot-core --lib settings` → PASS (all settings tests, old ones included).

- [ ] **Step 5: Commit** "The brain block in settings.json".

### Task 5: API keys in the OS credential store

**Files:** Create `native/secrets.rs`; `native/mod.rs` (`pub mod secrets;`).

- [ ] **Step 1: Failing tests** in `secrets.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_secrets_round_trip() {
        let s = MemorySecrets::default();
        assert_eq!(s.get("openai").unwrap(), None);
        s.set("openai", "  sk-test  ").unwrap();
        assert_eq!(s.get("openai").unwrap().as_deref(), Some("sk-test"));
        assert!(s.set("openai", "   ").is_err());
        s.delete("openai").unwrap();
        s.delete("openai").unwrap();
        assert_eq!(s.get("openai").unwrap(), None);
    }

    #[test]
    fn a_key_is_scrubbed_from_text() {
        assert_eq!(redact("bad key sk-abc123 rejected", Some("sk-abc123")), "bad key [key] rejected");
        assert_eq!(redact("nothing", None), "nothing");
        assert_eq!(redact("short ab", Some("ab")), "short ab", "keys under 8 characters are not real keys");
    }

    /// Writes to the real Keychain / Credential Manager under a test name.
    ///   cargo test -p pindot-core --lib real_secret_store -- --ignored
    #[test]
    #[ignore]
    fn real_secret_store() {
        let s = SystemSecrets::named("Pindot AI keys TEST (safe to delete)");
        s.set("probe", "sk-real-store-test").unwrap();
        assert_eq!(s.get("probe").unwrap().as_deref(), Some("sk-real-store-test"));
        s.delete("probe").unwrap();
        assert_eq!(s.get("probe").unwrap(), None);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::secrets` → FAIL.

- [ ] **Step 3: Implement** above the tests (same store setup as `vault/keys.rs`):

```rust
//! API keys for the loop's providers, in the OS credential store (the
//! macOS Keychain, Windows Credential Manager). Never in settings.json, a
//! log, an error message or a conversation file.

use std::collections::HashMap;
use std::sync::Mutex;

pub const SERVICE: &str = "Pindot AI keys";

pub trait SecretStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
    fn set(&self, provider: &str, key: &str) -> Result<(), String>;
    fn delete(&self, provider: &str) -> Result<(), String>;
}

/// Tests only.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(provider).cloned())
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let key = checked(key)?;
        self.0.lock().unwrap().insert(provider.into(), key);
        Ok(())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(provider);
        Ok(())
    }
}

fn checked(key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("The key is empty.".into());
    }
    if k.chars().any(char::is_whitespace) || k.len() > 512 {
        return Err("That doesn't look like an API key.".into());
    }
    Ok(k.to_string())
}

pub struct SystemSecrets {
    service: String,
}

impl Default for SystemSecrets {
    fn default() -> Self {
        Self::named(SERVICE)
    }
}

impl SystemSecrets {
    pub fn named(service: &str) -> Self {
        Self { service: service.into() }
    }

    fn entry(&self, provider: &str) -> Result<keyring_core::Entry, String> {
        use keyring_core::api::CredentialStoreApi;
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(not(any(target_os = "macos", windows)))]
        return Err("this operating system has no supported credential store".into());
        #[cfg(any(target_os = "macos", windows))]
        {
            let store = store.map_err(|e| e.to_string())?;
            store.build(&self.service, provider, None).map_err(|e| e.to_string())
        }
    }
}

impl SecretStore for SystemSecrets {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        match self.entry(provider)?.get_secret() {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| "the saved key is damaged".into()),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let key = checked(key)?;
        self.entry(provider)?.set_secret(key.as_bytes()).map_err(|e| e.to_string())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        match self.entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// `text` with every copy of `key` replaced, for anything that might be
/// shown or logged.
pub fn redact(text: &str, key: Option<&str>) -> String {
    match key.map(str::trim).filter(|k| k.len() >= 8) {
        Some(k) => text.replace(k, "[key]"),
        None => text.to_string(),
    }
}
```

- [ ] **Step 4: Run** → PASS (the ignored test is skipped). **Step 5: Commit** "API keys in the OS credential store".

### Task 6: The Chat Completions wire format

**Files:** Create `native/openai.rs`; `native/mod.rs` (`pub mod openai;`).

- [ ] **Step 1: Failing tests** in `openai.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_body_has_every_kind_of_message() {
        let msgs = vec![
            Msg::System("rules".into()),
            Msg::User { text: "hi".into(), images: vec![] },
            Msg::Assistant { text: String::new(), calls: vec![ToolCall { id: "c1".into(), name: "click".into(), arguments: "{\"ref\":\"r1\"}".into() }] },
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
    fn errors_are_said_plainly_and_never_carry_the_key() {
        let local = Who { provider: "Ollama".into(), model: "qwen3:8b".into(), local: true };
        let remote = Who { provider: "OpenAI".into(), model: "gpt-x".into(), local: false };
        assert_eq!(describe(&CallError::Unreachable("refused".into()), &local), "Ollama isn't running. Start it, or pick another brain in Settings.");
        assert_eq!(describe(&CallError::Status { code: 401, body: String::new() }, &remote), "OpenAI refused the API key. Check it in Settings > Brain.");
        assert_eq!(describe(&CallError::Status { code: 402, body: String::new() }, &remote), "Your OpenAI account is out of credit.");
        assert_eq!(describe(&CallError::Status { code: 404, body: String::new() }, &remote), "gpt-x isn't available on OpenAI. Pick another in Settings > Brain.");
        assert_eq!(describe(&CallError::Timeout, &local), "No answer from qwen3:8b in two minutes.");
        assert!(is_context_overflow(&CallError::Status { code: 400, body: "This model's maximum context length is 8192 tokens".into() }));
        assert!(!is_context_overflow(&CallError::Status { code: 400, body: "bad tool".into() }));
    }
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::openai` → FAIL.

- [ ] **Step 3: Implement** above the tests:

```rust
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
            let mut v = json!({"role": "assistant", "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) }});
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
        let thinking = [self.thinking.trim(), think_inline.trim()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n");
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

    /// A request with the key, refused if the key would cross the network
    /// in the clear.
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
    let req = ep.post(&agent, "chat/completions")?.header("Accept", "text/event-stream");
    let body = body.clone();
    let (tx, rx) = mpsc::channel::<Line>();
    std::thread::Builder::new()
        .name("pindot-loop-http".into())
        .spawn(move || {
            let resp = match req.send_json(&body) {
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
                            return; // the caller gave up (Esc or timeout)
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
```

Check: `ureq::RequestBuilder<ureq::typestate::WithBody>` is the type `agent.post()` returns in 3.5.2 (read `src/request.rs`); adjust the return type if the name differs.

- [ ] **Step 4: Run** → PASS. **Step 5: Commit** "Chat Completions for Pindot's own loop: messages, the stream, errors in plain words".

### Task 7: Context fitting

**Files:** Create `native/context.rs`; `native/mod.rs` (`pub mod context;`).

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::native::openai::{Image, ToolCall};

    fn img() -> Image {
        Image { mime: "image/png".into(), base64: "A".repeat(100) }
    }
    fn turn(n: usize) -> Vec<Msg> {
        vec![
            Msg::User { text: format!("request {n}"), images: vec![] },
            Msg::Assistant { text: String::new(), calls: vec![ToolCall { id: format!("c{n}"), name: "screenshot".into(), arguments: "{}".into() }] },
            Msg::Tool { call_id: format!("c{n}"), text: "x".repeat(2000) },
            Msg::ToolImages(vec![img()]),
            Msg::Assistant { text: format!("done {n}"), calls: vec![] },
        ]
    }

    #[test]
    fn nothing_changes_when_it_fits() {
        let mut m = vec![Msg::System("s".into())];
        m.extend(turn(1));
        let before = m.clone();
        assert!(!fit(&mut m, 100_000));
        assert_eq!(m, before);
    }

    #[test]
    fn old_pictures_go_first_then_long_results_then_old_turns() {
        let mut m = vec![Msg::System("s".into())];
        for n in 0..4 {
            m.extend(turn(n));
        }
        let pics = |m: &[Msg]| m.iter().filter(|x| matches!(x, Msg::ToolImages(_))).count();
        assert_eq!(pics(&m), 4);
        // Just over: only the old pictures go.
        let budget = estimate(&m) - 2 * IMAGE_TOKENS;
        assert!(fit(&mut m, budget));
        assert_eq!(pics(&m), 1, "the latest picture stays");
        assert_eq!(m.iter().filter(|x| matches!(x, Msg::User { .. })).count(), 4, "no turn dropped yet");
        // Much tighter: old turns go, the system prompt and the last turn stay.
        assert!(fit(&mut m, 900));
        assert!(matches!(m[0], Msg::System(_)));
        assert!(matches!(&m[1], Msg::User { text, .. } if text == "request 3"), "{m:?}");
    }
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::context` → FAIL.

- [ ] **Step 3: Implement:**

```rust
//! Keeping a conversation inside the model's context: an estimate of its
//! size, and trimming in a fixed order (old pictures, then long old tool
//! results, then the oldest whole turns).

use super::openai::Msg;

/// About what one screenshot costs a model that can see.
pub const IMAGE_TOKENS: usize = 1100;
/// Old tool results are cut to this many characters when trimming.
pub const SHORT_RESULT: usize = 300;

/// A rough token count: four characters a token, plus pictures.
pub fn estimate(messages: &[Msg]) -> usize {
    messages
        .iter()
        .map(|m| {
            4 + match m {
                Msg::System(t) | Msg::Tool { text: t, .. } => t.len() / 4,
                Msg::User { text, images } => text.len() / 4 + images.len() * IMAGE_TOKENS,
                Msg::Assistant { text, calls } => (text.len() + calls.iter().map(|c| c.name.len() + c.arguments.len()).sum::<usize>()) / 4,
                Msg::ToolImages(i) => i.len() * IMAGE_TOKENS,
            }
        })
        .sum()
}

/// Trims `messages` (index 0, the system prompt, always stays) until it
/// fits in `budget` tokens. Returns whether anything was dropped.
pub fn fit(messages: &mut Vec<Msg>, budget: usize) -> bool {
    if estimate(messages) <= budget {
        return false;
    }
    // 1. Every tool picture but the latest.
    let last_pic = messages.iter().rposition(|m| matches!(m, Msg::ToolImages(_)));
    let mut i = 0;
    messages.retain(|m| {
        let keep = !matches!(m, Msg::ToolImages(_)) || Some(i) == last_pic;
        i += 1;
        keep
    });
    if estimate(messages) <= budget {
        return true;
    }
    // 2. Long tool results, all but the last two.
    let tools: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Msg::Tool { .. })).map(|(i, _)| i).collect();
    for &i in tools.iter().rev().skip(2) {
        if let Msg::Tool { text, .. } = &mut messages[i]
            && text.len() > SHORT_RESULT
        {
            let cut: String = text.chars().take(SHORT_RESULT).collect();
            *text = format!("{cut}... (shortened)");
        }
    }
    // 3. The oldest whole turns (a turn starts at a user message), never the last one.
    while estimate(messages) > budget {
        let users: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Msg::User { .. })).map(|(i, _)| i).collect();
        if users.len() < 2 {
            break;
        }
        messages.drain(users[0]..users[1]);
    }
    true
}
```

- [ ] **Step 4: Run** → PASS. **Step 5: Commit** "Trimming a loop conversation to fit the model's context".

### Task 8: The conversation file

**Files:** Create `native/log.rs`; `native/mod.rs` (`pub mod log;`).

- [ ] **Step 1: Failing test:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::transcript::{ChatItem, read_all};

    #[test]
    fn every_line_reads_back_as_chat_items() {
        let dir = std::env::temp_dir().join(format!("pindot-log-{}", uuid::Uuid::new_v4()));
        let log = ConversationLog::create(&dir, "s1").unwrap();
        log.user("Open TextEdit");
        log.assistant("I'll check.", "Opening it.", &[ToolCall { id: "c1".into(), name: "open_app".into(), arguments: "{\"name\":\"TextEdit\"}".into() }]);
        log.tool_result("c1", "Opened TextEdit.", false);
        log.assistant("", "Done.", &[]);
        log.interrupted();
        let items = read_all(log.path());
        assert!(matches!(&items[0], ChatItem::User { text, .. } if text == "Open TextEdit"), "{items:?}");
        assert!(matches!(&items[1], ChatItem::Thinking { text, .. } if text == "I'll check."));
        assert!(matches!(&items[2], ChatItem::Assistant { text, .. } if text == "Opening it."));
        assert!(matches!(&items[3], ChatItem::Step { name, result: Some(r), .. } if name == "mcp__desktop__open_app" && r == "Opened TextEdit."));
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
```

(If `read_all` attaches results differently — check `chat/transcript.rs` `Reader::poll` — adjust only the expected `result` match, not the format.)

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::log` → FAIL.

- [ ] **Step 3: Implement:**

```rust
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
```

- [ ] **Step 4: Run** → PASS. **Step 5: Commit** "The loop's conversation file, in the format Chat reads".

### Task 9: MCP client of Pindot's own server

**Files:** Create `native/mcp_client.rs`; `native/mod.rs` (`pub mod mcp_client;`); create `crates/pindot-core/tests/native_loop.rs`.

- [ ] **Step 1: Failing test.** Create `crates/pindot-core/tests/native_loop.rs` with a harness (a trimmed copy of `tests/server.rs`'s) and the first test:

```rust
//! Pindot's own loop against Pindot's real MCP server (mock platform) and
//! a fake Chat Completions server: tools listed and called with the
//! session header, the approval gate, a whole turn.

use std::sync::Arc;
use std::time::Duration;

use pindot_core::brain::native::mcp_client::McpClient;
use pindot_core::platform::mock::{MockAccessibility, MockCapture, MockInput, MockNode, MockOverlay, MockStopKey};
use pindot_core::platform::{AppInfo, Rect, WindowInfo};
use pindot_core::server::activity::ActivityLog;
use pindot_core::server::approval::{ApprovalRequest, Approver};
use pindot_core::server::{RunningServer, ServerDeps, ServerState, start};
use tokio::sync::oneshot;

const TOKEN: &str = "test-token-0123456789abcdef";

struct Deny;
impl Approver for Deny {
    fn ask(&self, _req: ApprovalRequest, reply: oneshot::Sender<bool>) {
        let _ = reply.send(false);
    }
    fn cancel(&self, _id: &str) {}
}

pub struct Pindot {
    pub rt: tokio::runtime::Runtime,
    pub server: RunningServer,
    pub state: Arc<ServerState>,
    pub input: Arc<MockInput>,
}

/// TextEdit (pid 10) with one window and a "New Document" button; only
/// TextEdit is auto-approved.
pub fn pindot() -> Pindot {
    let ax = MockAccessibility::default();
    *ax.apps.lock().unwrap() = vec![AppInfo { pid: 10, name: "TextEdit".into(), bundle_id: Some("com.apple.TextEdit".into()) }];
    *ax.windows.lock().unwrap() = vec![WindowInfo {
        id: 100,
        pid: 10,
        app: "TextEdit".into(),
        title: "TextEdit window".into(),
        bounds: Rect::new(100.0, 100.0, 800.0, 600.0),
        layer: 0,
        alpha: 1.0,
    }];
    *ax.frontmost.lock().unwrap() = Some(10);
    ax.trees.lock().unwrap().insert(
        100,
        vec![
            MockNode::new(1, 0, "AXWindow", "Untitled", Some(Rect::new(100.0, 100.0, 800.0, 600.0))),
            MockNode::new(2, 1, "AXButton", "New Document", Some(Rect::new(200.0, 200.0, 90.0, 20.0))),
        ],
    );
    let input = Arc::new(MockInput::default());
    let vault_dir = std::env::temp_dir().join(format!("pindot-loop-test-{}", uuid::Uuid::new_v4()));
    let state = ServerState::new(ServerDeps {
        capture: Arc::new(MockCapture::default()),
        input: input.clone(),
        accessibility: Arc::new(ax),
        stop_key: Arc::new(MockStopKey::default()),
        overlay: Arc::new(MockOverlay::default()),
        approver: Arc::new(Deny),
        activity: Arc::new(ActivityLog::memory()),
        vault: Arc::new(pindot_core::vault::Vault::new(vault_dir, Arc::new(pindot_core::vault::keys::MemoryKeys::default()))),
        test_auto_approve: vec!["TextEdit".into()],
        own_pid: 99,
        idle: Duration::from_millis(50),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(start(state.clone(), TOKEN, 0)).unwrap();
    Pindot { rt, server, state, input }
}

#[test]
fn tools_are_listed_and_called_in_the_loops_own_session() {
    let p = pindot();
    let mcp = McpClient::new(&p.server.mcp_url(), TOKEN, "loop-session-1");
    let tools = mcp.list_tools().unwrap();
    assert!(tools.iter().any(|t| t.name == "desktop_context"));
    assert!(tools.iter().all(|t| t.parameters.get("$schema").is_none()));
    let out = mcp.call("request_access", r#"{"apps":["TextEdit"],"reason":"test"}"#);
    assert!(!out.is_error, "{}", out.text);
    let ctx = mcp.call("desktop_context", "{}");
    assert!(ctx.text.contains("TextEdit"), "{}", ctx.text);
    // Approvals belong to this session only.
    let other = McpClient::new(&p.server.mcp_url(), TOKEN, "loop-session-2");
    assert!(!other.call("desktop_context", "{}").text.contains("Approved for this session: TextEdit"));
    let bad = mcp.call("desktop_context", "not json");
    assert!(bad.is_error && bad.text.contains("not valid JSON"));
    mcp.close();
    p.state.end_session("loop-session-1");
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --test native_loop` → FAIL (no `mcp_client`).

- [ ] **Step 3: Implement** `native/mcp_client.rs`:

```rust
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
        Self { url: url.into(), token: token.into(), session_key: session_key.into(), mcp_session: Mutex::new(None), agent, next_id: AtomicU64::new(1) }
    }

    /// One POST; returns (HTTP status, the JSON-RPC message with `id`, or Null).
    fn post(&self, body: &Value) -> Result<(u16, Option<String>, Value), String> {
        let mut r = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/json, text/event-stream")
            .header(SESSION_HEADER, &self.session_key);
        if let Some(s) = self.mcp_session.lock().unwrap().clone() {
            r = r.header("mcp-session-id", s).header("mcp-protocol-version", PROTOCOL);
        }
        let resp = r.send_json(body).map_err(|e| format!("Pindot's desktop tools did not answer: {e}"))?;
        let code = resp.status().as_u16();
        let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
        let text = resp.into_body().with_config().limit(64 * 1024 * 1024).read_to_string().unwrap_or_default();
        Ok((code, sid, rpc_message(&text)))
    }

    fn initialize(&self) -> Result<(), String> {
        *self.mcp_session.lock().unwrap() = None;
        let (code, sid, v) = self.post(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "pindot-loop", "version": env!("CARGO_PKG_VERSION")}}}))?;
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
        let args: Value = match serde_json::from_str(if arguments.trim().is_empty() { "{}" } else { arguments }) {
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
        let _ = self
            .agent
            .delete(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("mcp-session-id", s)
            .call();
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
```

Make `SESSION_HEADER` reachable: it is `pub const` in `server/tools.rs`; if `tools` is not `pub mod` in `server/mod.rs`, add `pub use tools::SESSION_HEADER;` there and import `crate::server::SESSION_HEADER` instead. If `mock::MockNode::new`, `Rect::new` or `ActivityLog::memory` differ, copy their use from `tests/server.rs`.

- [ ] **Step 4: Run** `cargo test -p pindot-core --test native_loop` → PASS.

- [ ] **Step 5: Commit** "The loop's MCP client of Pindot's own server".

### Task 10: One turn

**Files:** Create `native/run.rs`; `native/mod.rs` (`pub mod run;`).

- [ ] **Step 1: Failing tests** (a scripted model and scripted tools, no network):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Script(VecDeque<Result<Reply, CallError>>, Vec<usize>);
    impl Model for Script {
        fn reply(&mut self, messages: &[Msg], _t: &[ToolDef], _c: &AtomicBool) -> Result<Reply, CallError> {
            self.1.push(messages.len());
            self.0.pop_front().unwrap_or_else(|| Ok(Reply { text: "end".into(), ..Default::default() }))
        }
    }
    #[derive(Default)]
    struct Fake {
        called: Vec<String>,
        cancel_on: Option<(String, std::sync::Arc<AtomicBool>)>,
    }
    impl Tools for Fake {
        fn call(&mut self, name: &str, _a: &str) -> ToolOutput {
            self.called.push(name.to_string());
            if let Some((n, c)) = &self.cancel_on
                && n == name
            {
                c.store(true, Ordering::SeqCst);
            }
            let images = if name == "screenshot" { vec![Image { mime: "image/png".into(), base64: "AA".into() }] } else { vec![] };
            ToolOutput { text: format!("{name} ok"), images, is_error: false }
        }
    }
    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), arguments: "{}".into() }
    }
    fn who() -> Who {
        Who { provider: "Ollama".into(), model: "m".into(), local: true }
    }
    fn conv() -> Conversation {
        Conversation { messages: vec![Msg::System("s".into()), Msg::User { text: "go".into(), images: vec![] }], context_tokens: 32_000, vision: true }
    }
    const LIMITS: TurnLimits = TurnLimits { max_tool_calls: 25, deadline: Duration::from_secs(60) };

    #[test]
    fn tools_are_called_until_a_reply_without_calls() {
        let mut model = Script(
            VecDeque::from([
                Ok(Reply { text: "Looking.".into(), calls: vec![call("a", "screenshot"), call("b", "find")], ..Default::default() }),
                Ok(Reply { text: "Found it.".into(), ..Default::default() }),
            ]),
            vec![],
        );
        let mut tools = Fake::default();
        let mut c = conv();
        let mut events = vec![];
        let end = run_turn(&mut c, &mut model, &[], &mut tools, None, &AtomicBool::new(false), &LIMITS, &who(), &mut |e| events.push(e));
        assert_eq!(end, TurnEnd::Done { last: Some("Found it.".into()) });
        assert_eq!(tools.called, vec!["screenshot", "find"]);
        assert_eq!(
            events,
            vec![
                TurnEvent::Text("Looking.".into()),
                TurnEvent::ToolUse("screenshot".into()),
                TurnEvent::ToolResult,
                TurnEvent::ToolUse("find".into()),
                TurnEvent::ToolResult,
                TurnEvent::Text("Found it.".into()),
            ]
        );
        // system, user, assistant, tool, tool, picture, assistant
        assert!(matches!(c.messages[5], Msg::ToolImages(_)));
        assert_eq!(c.messages.len(), 7);
    }

    #[test]
    fn no_pictures_go_to_a_model_that_cannot_see() {
        let mut model = Script(VecDeque::from([Ok(Reply { calls: vec![call("a", "screenshot")], ..Default::default() })]), vec![]);
        let mut c = Conversation { vision: false, ..conv() };
        run_turn(&mut c, &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &LIMITS, &who(), &mut |_| {});
        assert!(!c.messages.iter().any(|m| matches!(m, Msg::ToolImages(_))));
    }

    #[test]
    fn esc_between_calls_skips_the_rest_and_answers_every_call() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut model = Script(VecDeque::from([Ok(Reply { calls: vec![call("a", "click"), call("b", "type_text")], ..Default::default() })]), vec![]);
        let mut tools = Fake { cancel_on: Some(("click".into(), cancel.clone())), ..Default::default() };
        let mut c = conv();
        let end = run_turn(&mut c, &mut model, &[], &mut tools, None, &cancel, &LIMITS, &who(), &mut |_| {});
        assert_eq!(end, TurnEnd::Interrupted);
        assert_eq!(tools.called, vec!["click"]);
        assert!(matches!(&c.messages[4], Msg::Tool { call_id, text } if call_id == "b" && text.contains("stopped")));
    }

    #[test]
    fn the_step_limit_ends_the_turn() {
        let replies = (0..10).map(|i| Ok(Reply { calls: vec![call(&format!("c{i}"), "wait")], ..Default::default() })).collect();
        let mut model = Script(replies, vec![]);
        let limits = TurnLimits { max_tool_calls: 3, ..LIMITS };
        let end = run_turn(&mut conv(), &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &limits, &who(), &mut |_| {});
        assert_eq!(end, TurnEnd::StepLimit);
    }

    #[test]
    fn a_context_overflow_is_trimmed_and_tried_again_once() {
        let overflow = || Err(CallError::Status { code: 400, body: "maximum context length exceeded".into() });
        let mut model = Script(VecDeque::from([overflow(), Ok(Reply { text: "ok".into(), ..Default::default() })]), vec![]);
        let end = run_turn(&mut conv(), &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &LIMITS, &who(), &mut |_| {});
        assert_eq!(end, TurnEnd::Done { last: Some("ok".into()) });
        let mut model = Script(VecDeque::from([overflow(), overflow()]), vec![]);
        let end = run_turn(&mut conv(), &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &LIMITS, &who(), &mut |_| {});
        assert!(matches!(end, TurnEnd::TooLong(_)));
    }

    #[test]
    fn an_error_is_said_plainly() {
        let mut model = Script(VecDeque::from([Err(CallError::Unreachable("refused".into()))]), vec![]);
        let end = run_turn(&mut conv(), &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &LIMITS, &who(), &mut |_| {});
        assert_eq!(end, TurnEnd::Failed("Ollama isn't running. Start it, or pick another brain in Settings.".into()));
    }
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib native::run` → FAIL.

- [ ] **Step 3: Implement:**

```rust
//! One turn of Pindot's own loop: ask the model, run the tool calls it
//! makes, give it the results, and repeat until it answers without tools.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::context;
use super::log::ConversationLog;
use super::mcp_client::{McpClient, ToolOutput};
use super::openai::{CallError, Image, Msg, Reply, ToolCall, ToolDef, Who, describe, is_context_overflow};

/// What answers a request: the HTTP model in use, or a script in tests.
pub trait Model: Send {
    fn reply(&mut self, messages: &[Msg], tools: &[ToolDef], cancel: &AtomicBool) -> Result<Reply, CallError>;
}

/// What runs a tool call: the MCP client, or a fake in tests.
pub trait Tools: Send {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput;
}

impl Tools for McpClient {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput {
        McpClient::call(self, name, arguments)
    }
}

pub struct Conversation {
    /// Starts with the system prompt.
    pub messages: Vec<Msg>,
    pub context_tokens: usize,
    /// The model can see pictures.
    pub vision: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// A complete assistant message with text.
    Text(String),
    /// A tool call is starting (the tool's own name).
    ToolUse(String),
    ToolResult,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    Done { last: Option<String> },
    /// `max_tool_calls` reached.
    StepLimit,
    Interrupted,
    Failed(String),
    /// Too long even after trimming: start a fresh conversation.
    TooLong(String),
}

#[derive(Debug, Clone, Copy)]
pub struct TurnLimits {
    pub max_tool_calls: usize,
    pub deadline: Duration,
}

pub const LIMITS: TurnLimits = TurnLimits { max_tool_calls: 25, deadline: crate::brain::persistent::TURN_TIMEOUT };

/// The share of the context the conversation may fill (the rest is for
/// the reply).
const FILL: f64 = 0.85;

#[allow(clippy::too_many_arguments)]
pub fn run_turn(
    conv: &mut Conversation,
    model: &mut dyn Model,
    tools: &[ToolDef],
    runner: &mut dyn Tools,
    log: Option<&ConversationLog>,
    cancel: &AtomicBool,
    limits: &TurnLimits,
    who: &Who,
    emit: &mut dyn FnMut(TurnEvent),
) -> TurnEnd {
    let started = Instant::now();
    let mut calls_made = 0;
    let mut last_text: Option<String> = None;
    let budget = (conv.context_tokens as f64 * FILL) as usize;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return interrupted(log);
        }
        if started.elapsed() > limits.deadline {
            return TurnEnd::Failed("This took longer than 15 minutes, so I stopped.".into());
        }
        context::fit(&mut conv.messages, budget);
        let reply = match model.reply(&conv.messages, tools, cancel) {
            Ok(r) => r,
            Err(CallError::Cancelled) => return interrupted(log),
            Err(e) if is_context_overflow(&e) => {
                context::fit(&mut conv.messages, budget / 2);
                match model.reply(&conv.messages, tools, cancel) {
                    Ok(r) => r,
                    Err(CallError::Cancelled) => return interrupted(log),
                    Err(e) if is_context_overflow(&e) => {
                        return TurnEnd::TooLong(format!("This conversation got too long for {}. I'll start a fresh one.", who.model));
                    }
                    Err(e) => return TurnEnd::Failed(describe(&e, who)),
                }
            }
            Err(e) => return TurnEnd::Failed(describe(&e, who)),
        };
        if let Some(l) = log {
            l.assistant(&reply.thinking, &reply.text, &reply.calls);
        }
        conv.messages.push(Msg::Assistant { text: reply.text.clone(), calls: reply.calls.clone() });
        if !reply.text.is_empty() {
            last_text = Some(reply.text.clone());
            emit(TurnEvent::Text(reply.text.clone()));
        }
        if reply.calls.is_empty() {
            return TurnEnd::Done { last: last_text };
        }
        let mut pictures: Vec<Image> = vec![];
        let mut stopped = false;
        for c in &reply.calls {
            if stopped || cancel.load(Ordering::SeqCst) {
                stopped = true;
                answer(conv, log, c, "Not run: the user stopped the turn.", true);
                continue;
            }
            emit(TurnEvent::ToolUse(c.name.clone()));
            let out = runner.call(&c.name, &c.arguments);
            answer(conv, log, c, &out.text, out.is_error);
            emit(TurnEvent::ToolResult);
            if conv.vision {
                pictures.extend(out.images);
            }
            calls_made += 1;
        }
        if !pictures.is_empty() {
            conv.messages.push(Msg::ToolImages(pictures));
        }
        if stopped || cancel.load(Ordering::SeqCst) {
            return interrupted(log);
        }
        if calls_made >= limits.max_tool_calls {
            return TurnEnd::StepLimit;
        }
    }
}

fn answer(conv: &mut Conversation, log: Option<&ConversationLog>, c: &ToolCall, text: &str, is_error: bool) {
    let text = if text.is_empty() { "(no output)" } else { text };
    if let Some(l) = log {
        l.tool_result(&c.id, text, is_error);
    }
    conv.messages.push(Msg::Tool { call_id: c.id.clone(), text: text.to_string() });
}

fn interrupted(log: Option<&ConversationLog>) -> TurnEnd {
    if let Some(l) = log {
        l.interrupted();
    }
    TurnEnd::Interrupted
}
```

The "stopped" message in the test (`text.contains("stopped")`) matches "the user stopped the turn".

- [ ] **Step 4: Run** → PASS. **Step 5: Commit** "One turn of Pindot's own loop".

### Task 11: Finding servers, models and what a model can do

**Files:** Create `native/probe.rs`; `native/mod.rs` (`pub mod probe;`); add tests to `tests/native_loop.rs`.

- [ ] **Step 1: Failing tests.** Append to `tests/native_loop.rs` a fake model server (used again in Tasks 12-13):

```rust
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::Mutex;

/// A fake OpenAI-compatible server: `/v1/models`, Ollama's `/api/show`, and
/// `/v1/chat/completions` playing scripted SSE bodies in order (the last
/// one repeats). Records every chat request body.
pub struct FakeModel {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
}

pub fn fake_model(rt: &tokio::runtime::Runtime, replies: Vec<String>) -> FakeModel {
    #[derive(Clone)]
    struct S(Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<Value>>>);
    let requests = Arc::new(Mutex::new(vec![]));
    let s = S(Arc::new(Mutex::new(replies)), requests.clone());
    let app = Router::new()
        .route("/v1/models", get(|| async { Json(json!({"data": [{"id": "tiny"}, {"id": "nomic-embed"}]})) }))
        .route(
            "/api/show",
            post(|Json(b): Json<Value>| async move {
                let caps = if b["model"] == "nomic-embed" { json!(["embedding"]) } else { json!(["completion", "tools", "vision"]) };
                Json(json!({"capabilities": caps, "model_info": {"qwen3.context_length": 40960}}))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|State(s): State<S>, Json(b): Json<Value>| async move {
                s.1.lock().unwrap().push(b);
                let mut q = s.0.lock().unwrap();
                let body = if q.len() > 1 { q.remove(0) } else { q[0].clone() };
                ([("content-type", "text/event-stream")], body)
            }),
        )
        .with_state(s);
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    FakeModel { base_url: format!("http://{addr}/v1"), requests }
}

/// An SSE body: text, then optional tool calls, then [DONE].
pub fn sse(text: &str, calls: &[(&str, &str, &str)]) -> String {
    let mut out = String::new();
    if !text.is_empty() {
        out += &format!("data: {}\n\n", json!({"choices": [{"delta": {"content": text}}]}));
    }
    for (i, (id, name, args)) in calls.iter().enumerate() {
        out += &format!("data: {}\n\n", json!({"choices": [{"delta": {"tool_calls": [{"index": i, "id": id, "function": {"name": name, "arguments": args}}]}}]}));
    }
    out + "data: [DONE]\n\n"
}

#[test]
fn models_and_what_they_can_do_are_read_from_ollama() {
    use pindot_core::brain::native::probe;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fake = fake_model(&rt, vec![sse("hi", &[])]);
    let models = probe::list_models("ollama", &fake.base_url, None).unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["tiny"], "embedding models are left out");
    assert_eq!(models[0].vision, Some(true));
    assert_eq!(models[0].tools, Some(true));
    assert_eq!(models[0].context, Some(40960));
    let unknown = probe::list_models("custom", &fake.base_url, None).unwrap();
    assert_eq!(unknown.len(), 2);
    assert_eq!(unknown[0].vision, None);
    assert!(probe::list_models("ollama", "http://127.0.0.1:9/v1", None).is_err());
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --test native_loop models_and` → FAIL.

- [ ] **Step 3: Implement** `native/probe.rs`:

```rust
//! What the loop can learn from a server: which local servers are running,
//! which models a server has, and what a model can do (see pictures, use
//! tools, how much context). Each server says it its own way; anything
//! unknown is `None`. Read each API's current documentation before
//! changing a field name.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::openai::{CallError, Endpoint};
use super::providers::{self, PROVIDERS};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub vision: Option<bool>,
    pub tools: Option<bool>,
    pub context: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FoundServer {
    pub provider: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<ModelInfo>,
}

/// The server's root (the base URL without its trailing `/v1`).
fn root_of(base_url: &str) -> String {
    let b = base_url.trim_end_matches('/');
    b.strip_suffix("/v1").unwrap_or(b).to_string()
}

fn get_json(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Result<Value, CallError> {
    let mut r = agent.get(url);
    if let Some(k) = key {
        r = r.header("Authorization", format!("Bearer {k}"));
    }
    let resp = r.call().map_err(|e| CallError::Unreachable(e.to_string()))?;
    let code = resp.status().as_u16();
    let body = resp.into_body().read_to_string().unwrap_or_default();
    if code >= 400 {
        return Err(CallError::Status { code, body: body.chars().take(2000).collect() });
    }
    serde_json::from_str(&body).map_err(|e| CallError::Stream(format!("the model list was not understood: {e}")))
}

fn post_json(agent: &ureq::Agent, url: &str, body: Value) -> Option<Value> {
    let resp = agent.post(url).send_json(body).ok()?;
    if resp.status().as_u16() >= 400 {
        return None;
    }
    serde_json::from_str(&resp.into_body().read_to_string().ok()?).ok()
}

/// The models `provider` at `base_url` offers, with what is known about
/// each. Embedding-only models are left out.
pub fn list_models(provider: &str, base_url: &str, key: Option<&str>) -> Result<Vec<ModelInfo>, CallError> {
    if key.is_some() && providers::plain_http_remote(base_url) {
        return Err(CallError::Unreachable("a key is never sent over plain http to another computer".into()));
    }
    let agent = Endpoint::agent(Duration::from_secs(3));
    let ep = Endpoint { base_url: base_url.into(), key: key.map(str::to_string) };
    let list = get_json(&agent, &ep.url("models"), key)?;
    let mut out: Vec<ModelInfo> = vec![];
    for m in list.get("data").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = m.get("id").and_then(Value::as_str) else { continue };
        let mut info = ModelInfo { id: id.to_string(), vision: None, tools: None, context: None };
        // OpenRouter puts the details in the list itself.
        if let Some(inputs) = m.pointer("/architecture/input_modalities").and_then(Value::as_array) {
            info.vision = Some(inputs.iter().any(|v| v == "image"));
        }
        if let Some(params) = m.get("supported_parameters").and_then(Value::as_array) {
            info.tools = Some(params.iter().any(|v| v == "tools"));
        }
        info.context = m.get("context_length").and_then(Value::as_u64).map(|c| c.min(u32::MAX as u64) as u32);
        out.push(info);
    }
    let root = root_of(base_url);
    match provider {
        "ollama" => {
            out.retain_mut(|info| {
                let Some(show) = post_json(&agent, &format!("{root}/api/show"), json!({"model": info.id})) else { return true };
                let caps: Vec<&str> = show.get("capabilities").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                if !caps.is_empty() && !caps.contains(&"completion") {
                    return false; // an embedding model
                }
                info.vision = Some(caps.contains(&"vision"));
                info.tools = Some(caps.contains(&"tools"));
                info.context = show
                    .get("model_info")
                    .and_then(Value::as_object)
                    .and_then(|o| o.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| v.as_u64()))
                    .map(|c| c.min(u32::MAX as u64) as u32);
                true
            });
        }
        "lmstudio" => {
            if let Ok(v) = get_json(&agent, &format!("{root}/api/v0/models"), None) {
                for m in v.get("data").and_then(Value::as_array).into_iter().flatten() {
                    let id = m.get("id").and_then(Value::as_str).unwrap_or_default();
                    let kind = m.get("type").and_then(Value::as_str).unwrap_or_default();
                    if kind == "embeddings" {
                        out.retain(|i| i.id != id);
                        continue;
                    }
                    if let Some(info) = out.iter_mut().find(|i| i.id == id) {
                        info.vision = Some(kind == "vlm");
                        info.context = m
                            .get("loaded_context_length")
                            .or_else(|| m.get("max_context_length"))
                            .and_then(Value::as_u64)
                            .map(|c| c.min(u32::MAX as u64) as u32);
                    }
                }
            }
        }
        "llamacpp" => {
            if let Ok(v) = get_json(&agent, &format!("{root}/props"), None) {
                let ctx = v.pointer("/default_generation_settings/n_ctx").and_then(Value::as_u64).map(|c| c as u32);
                let vision = v.pointer("/modalities/vision").and_then(Value::as_bool);
                for info in &mut out {
                    info.context = ctx.or(info.context);
                    info.vision = vision.or(info.vision);
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

/// The local servers running now, with their models. Loopback only, a
/// short timeout each.
pub fn find_local() -> Vec<FoundServer> {
    PROVIDERS
        .iter()
        .filter(|p| p.local)
        .filter_map(|p| {
            let models = list_models(p.id, p.base_url, None).ok()?;
            Some(FoundServer { provider: p.id.into(), name: p.name.into(), base_url: p.base_url.into(), models })
        })
        .collect()
}

/// Ollama runs a model with a short context unless told otherwise
/// (`OLLAMA_CONTEXT_LENGTH`); a context Pindot cannot know is taken as this.
pub const OLLAMA_DEFAULT_CONTEXT: u32 = 8192;
/// Everything else, when the server does not say.
pub const DEFAULT_CONTEXT: u32 = 32_768;

/// The context the loop plans for: the setting, else what the server says
/// (for Ollama, never more than its default run length), else a default.
pub fn context_for(provider: &str, setting: Option<u32>, info: Option<&ModelInfo>) -> u32 {
    if let Some(c) = setting {
        return c;
    }
    let told = info.and_then(|i| i.context);
    match provider {
        "ollama" => told.map_or(OLLAMA_DEFAULT_CONTEXT, |c| c.min(OLLAMA_DEFAULT_CONTEXT)),
        _ => told.unwrap_or(DEFAULT_CONTEXT),
    }
}
```

Add to the unit tests of `probe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_comes_from_the_setting_then_the_server() {
        let i = ModelInfo { id: "m".into(), vision: None, tools: None, context: Some(40960) };
        assert_eq!(context_for("lmstudio", Some(16000), Some(&i)), 16000);
        assert_eq!(context_for("lmstudio", None, Some(&i)), 40960);
        assert_eq!(context_for("ollama", None, Some(&i)), OLLAMA_DEFAULT_CONTEXT);
        assert_eq!(context_for("openai", None, None), DEFAULT_CONTEXT);
        assert_eq!(root_of("http://127.0.0.1:11434/v1/"), "http://127.0.0.1:11434");
    }
}
```

- [ ] **Step 4: Run** `cargo test -p pindot-core --test native_loop models_and` and `cargo test -p pindot-core --lib native::probe` → PASS.

- [ ] **Step 5: Commit** "Finding local model servers, their models and what each can do".

### Task 12: `LoopBrain`

**Files:** Modify `native/mod.rs` (the brain itself); add an end-to-end test to `tests/native_loop.rs`.

- [ ] **Step 1: Failing test** in `tests/native_loop.rs`:

```rust
#[test]
fn a_spoken_request_runs_through_the_loop_and_pindots_tools() {
    use pindot_core::brain::Brain;
    use pindot_core::brain::native::settings::{BrainKind, BrainSettings, Vision};
    use pindot_core::brain::native::{LoopBrain, LoopConfig};
    use pindot_core::voice::machine::Input;

    let p = pindot();
    let fake = fake_model(
        &p.rt,
        vec![
            sse("Opening TextEdit.", &[("c1", "request_access", r#"{"apps":["TextEdit"],"reason":"asked"}"#)]),
            sse("", &[("c2", "desktop_context", "{}")]),
            sse("TextEdit is ready.", &[]),
        ],
    );
    let data = std::env::temp_dir().join(format!("pindot-loopbrain-{}", uuid::Uuid::new_v4()));
    let (tx, rx) = std::sync::mpsc::channel::<Input>();
    let tx = Mutex::new(tx);
    let brain = LoopBrain::start(
        LoopConfig {
            brain: BrainSettings { kind: BrainKind::Loop, provider: "custom".into(), base_url: fake.base_url.clone(), model: "tiny".into(), vision: Vision::Off, ..BrainSettings::default() },
            key: None,
            mcp_url: p.server.mcp_url(),
            token: TOKEN.into(),
            system_prompt: "Be brief.".into(),
            data_dir: data.clone(),
        },
        p.state.clone(),
        Arc::new(move |i| {
            let _ = tx.lock().unwrap().send(i);
        }),
    )
    .unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(10)).expect("an input from the brain");
    let Input::SessionReady { session_id } = next() else { panic!("not ready") };
    brain.send("open text edit");
    let mut got = vec![];
    loop {
        let i = next();
        let stop = matches!(i, Input::Stop { .. } | Input::TurnFailed { .. });
        got.push(i);
        if stop {
            break;
        }
    }
    assert!(matches!(&got[0], Input::TranscriptPrompt { session_id: s } if *s == session_id), "{got:?}");
    assert!(got.contains(&Input::TranscriptText { session_id: session_id.clone(), text: "Opening TextEdit.".into() }));
    assert!(got.contains(&Input::TranscriptToolUse { session_id: session_id.clone(), name: "mcp__desktop__request_access".into() }));
    assert_eq!(got.last(), Some(&Input::Stop { session_id: session_id.clone(), last_message: Some("TextEdit is ready.".into()) }));
    // The tools went through Pindot's server in the loop's session.
    let reqs = fake.requests.lock().unwrap();
    let tool_names: Vec<&str> = reqs[0]["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(tool_names.contains(&"click") && !tool_names.contains(&"screenshot"), "no screenshot without vision: {tool_names:?}");
    assert!(reqs[2]["messages"].to_string().contains("Approved for this session: TextEdit"), "the context result reached the model");
    // Chat can read the conversation.
    let file = pindot_core::brain::transcript::conversations_dir(&data).join(format!("{session_id}.jsonl"));
    let items = pindot_core::chat::transcript::read_all(&file);
    assert!(items.len() >= 4, "{items:?}");
    drop(brain);
    std::fs::remove_dir_all(&data).ok();
}
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --test native_loop a_spoken` → FAIL.

- [ ] **Step 3: Implement** `native/mod.rs` (keep the `pub mod` lines at the top):

```rust
//! Pindot's own loop: a brain for local models and OpenAI-compatible APIs.
//! A worker thread owns one conversation at a time and reports to the
//! voice machine with the same `Input`s as the Claude Code brain.

pub mod context;
pub mod log;
pub mod mcp_client;
pub mod openai;
pub mod probe;
pub mod providers;
pub mod run;
pub mod secrets;
pub mod settings;
pub mod test_model;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;

use self::log::ConversationLog;
use self::mcp_client::McpClient;
use self::openai::{CallError, Endpoint, Image, Msg, Reply, ToolDef, Who, chat, describe, request_body};
use self::run::{Conversation, LIMITS, Model, TurnEnd, TurnEvent, run_turn};
use self::settings::{BrainSettings, Vision};
use super::Brain;
use super::persistent::InputSink;
use crate::server::ServerState;
use crate::voice::machine::Input;

const RETRY_FIRST: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(300);
/// No first token (a local model loading counts) or no next chunk for this long.
pub const IDLE: Duration = Duration::from_secs(120);
/// Attached text files go in whole up to this size.
const TEXT_FILE_MAX: u64 = 32 * 1024;
const NO_PICTURES: &[&str] = &["screenshot", "zoom"];

pub struct LoopConfig {
    pub brain: BrainSettings,
    pub key: Option<String>,
    pub mcp_url: String,
    pub token: String,
    pub system_prompt: String,
    /// Conversations are written under it (`conversations/`).
    pub data_dir: PathBuf,
}

/// The HTTP model in use.
pub struct HttpModel {
    pub endpoint: Endpoint,
    pub model: String,
}

impl Model for HttpModel {
    fn reply(&mut self, messages: &[Msg], tools: &[ToolDef], cancel: &AtomicBool) -> Result<Reply, CallError> {
        chat(&self.endpoint, &request_body(&self.model, messages, tools), cancel, IDLE)
    }
}

/// The system prompt with the line about the tools this model has.
pub fn system_prompt(base: &str, vision: bool) -> String {
    let tools = if vision {
        "Use the desktop tools by name with JSON arguments. Prefer read_window and find; take a screenshot only when text is not enough."
    } else {
        "Use the desktop tools by name with JSON arguments. You cannot see pictures, so you have no screenshot tool: use read_window and find."
    };
    format!("{base} {tools}")
}

/// The tools a model gets: no picture tools for a model that cannot see.
pub fn tools_for(all: Vec<ToolDef>, vision: bool) -> Vec<ToolDef> {
    all.into_iter().filter(|t| vision || !NO_PICTURES.contains(&t.name.as_str())).collect()
}

/// The user's message: attached pictures as images (or a note), small text
/// files inline, anything else a note.
pub fn user_message(prompt: &str, vision: bool) -> Msg {
    let (text, paths) = crate::chat::attachments::split_prompt(prompt);
    let mut text = text;
    let mut images = vec![];
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match crate::chat::attachments::kind_of(&name) {
            crate::chat::attachments::Kind::Image if vision => match std::fs::read(&p) {
                Ok(bytes) => {
                    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                    let mime = if ext == "jpg" { "image/jpeg".to_string() } else { format!("image/{ext}") };
                    images.push(Image { mime, base64: base64::engine::general_purpose::STANDARD.encode(bytes) });
                }
                Err(_) => text.push_str(&format!("\n\n(The attached picture {name} could not be read.)")),
            },
            crate::chat::attachments::Kind::File if std::fs::metadata(&p).is_ok_and(|m| m.len() <= TEXT_FILE_MAX) => {
                match std::fs::read_to_string(&p) {
                    Ok(body) => text.push_str(&format!("\n\nAttached file {name}:\n```\n{body}\n```")),
                    Err(_) => text.push_str(&format!("\n\n(I can't open the attached file {name} with this brain.)")),
                }
            }
            _ => text.push_str(&format!("\n\n(I can't open the attached file {name} with this brain.)")),
        }
    }
    Msg::User { text, images }
}

enum Command {
    Send(String),
    NewSession,
    Shutdown,
}

/// Handle to the loop's worker. Dropping it ends the conversation.
pub struct LoopBrain {
    tx: mpsc::Sender<Command>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopBrain {
    pub fn start(config: LoopConfig, state: Arc<ServerState>, emit: InputSink) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let thread = std::thread::Builder::new().name("pindot-loop-brain".into()).spawn(move || Worker::new(config, state, emit, c).run(rx))?;
        Ok(Self { tx, cancel, thread: Some(thread) })
    }
}

impl Brain for LoopBrain {
    fn send(&self, text: &str) {
        let _ = self.tx.send(Command::Send(text.to_string()));
    }
    fn interrupt(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    fn new_session(&self) {
        let _ = self.tx.send(Command::NewSession);
    }
}

impl Drop for LoopBrain {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Session {
    id: String,
    mcp: McpClient,
    tools: Vec<ToolDef>,
    conv: Conversation,
    log: Option<ConversationLog>,
}

struct Worker {
    config: LoopConfig,
    state: Arc<ServerState>,
    emit: InputSink,
    cancel: Arc<AtomicBool>,
    model: HttpModel,
    who: Who,
    session: Option<Session>,
    problem: Option<String>,
}

impl Worker {
    fn new(config: LoopConfig, state: Arc<ServerState>, emit: InputSink, cancel: Arc<AtomicBool>) -> Self {
        let provider = providers::find(&config.brain.provider);
        let who = Who {
            provider: provider.map_or_else(|| config.brain.provider.clone(), |p| p.name.to_string()),
            model: config.brain.model.clone(),
            local: provider.is_some_and(|p| p.local),
        };
        let model = HttpModel { endpoint: Endpoint { base_url: config.brain.base_url.clone(), key: config.key.clone() }, model: config.brain.model.clone() };
        Self { config, state, emit, cancel, model, who, session: None, problem: None }
    }

    fn run(mut self, rx: mpsc::Receiver<Command>) {
        let mut retry = RETRY_FIRST;
        let mut next_try = Instant::now();
        loop {
            if self.session.is_none() && Instant::now() >= next_try {
                match self.open_session() {
                    Ok(()) => retry = RETRY_FIRST,
                    Err(msg) => {
                        if self.problem.as_deref() != Some(msg.as_str()) {
                            (self.emit)(Input::BrainUnavailable { message: msg.clone() });
                        }
                        self.problem = Some(msg);
                        next_try = Instant::now() + retry;
                        retry = (retry * 2).min(RETRY_MAX);
                    }
                }
            }
            let wait = if self.session.is_none() { next_try.saturating_duration_since(Instant::now()).max(Duration::from_millis(100)) } else { Duration::from_secs(3600) };
            match rx.recv_timeout(wait) {
                Ok(Command::Send(text)) => self.turn(&text),
                Ok(Command::NewSession) => {
                    self.close_session();
                    next_try = Instant::now();
                    retry = RETRY_FIRST;
                }
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.close_session();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn open_session(&mut self) -> Result<(), String> {
        let b = &self.config.brain;
        let models = probe::list_models(&b.provider, &b.base_url, self.config.key.as_deref()).map_err(|e| describe(&e, &self.who))?;
        let info = models.iter().find(|m| m.id == b.model);
        if info.is_none() && !models.is_empty() {
            return Err(describe(&CallError::Status { code: 404, body: String::new() }, &self.who));
        }
        let vision = match b.vision {
            Vision::On => true,
            Vision::Off => false,
            Vision::Auto => info.and_then(|i| i.vision).unwrap_or(false),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mcp = McpClient::new(&self.config.mcp_url, &self.config.token, &id);
        let tools = tools_for(mcp.list_tools()?, vision);
        let context = probe::context_for(&b.provider, b.context_tokens, info) as usize;
        let log = ConversationLog::create(&crate::brain::transcript::conversations_dir(&self.config.data_dir), &id)
            .map_err(|e| tracing::warn!("conversation file not created: {e:#}"))
            .ok();
        let conv = Conversation { messages: vec![Msg::System(system_prompt(&self.config.system_prompt, vision))], context_tokens: context, vision };
        tracing::info!(session = %id, model = %b.model, vision, context, tools = tools.len(), "loop brain session ready");
        self.session = Some(Session { id: id.clone(), mcp, tools, conv, log });
        self.problem = None;
        (self.emit)(Input::SessionReady { session_id: id });
        Ok(())
    }

    fn close_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.mcp.close();
            self.state.end_session(&s.id);
        }
    }

    fn turn(&mut self, text: &str) {
        let Some(s) = self.session.as_mut() else {
            let message = self.problem.clone().unwrap_or_else(|| "The assistant is still starting. Try again in a moment.".into());
            (self.emit)(Input::TurnFailed { message });
            return;
        };
        self.cancel.store(false, Ordering::SeqCst);
        let sid = s.id.clone();
        s.conv.messages.push(user_message(text, s.conv.vision));
        if let Some(l) = &s.log {
            l.user(text);
        }
        (self.emit)(Input::TranscriptPrompt { session_id: sid.clone() });
        let emit = self.emit.clone();
        let end = run_turn(&mut s.conv, &mut self.model, &s.tools, &mut s.mcp, s.log.as_ref(), &self.cancel, &LIMITS, &self.who, &mut |e| {
            let input = match e {
                TurnEvent::Text(text) => Input::TranscriptText { session_id: sid.clone(), text },
                TurnEvent::ToolUse(name) => Input::TranscriptToolUse { session_id: sid.clone(), name: format!("{}{name}", log::TOOL_PREFIX) },
                TurnEvent::ToolResult => Input::TranscriptToolResult { session_id: sid.clone() },
            };
            emit(input);
        });
        match end {
            TurnEnd::Done { last } => (self.emit)(Input::Stop { session_id: sid, last_message: last }),
            TurnEnd::StepLimit => (self.emit)(Input::Stop {
                session_id: sid,
                last_message: Some(format!("I've stopped after {} steps.", LIMITS.max_tool_calls)),
            }),
            TurnEnd::Interrupted => {}
            TurnEnd::Failed(message) => (self.emit)(Input::TurnFailed { message }),
            TurnEnd::TooLong(message) => {
                (self.emit)(Input::TurnFailed { message });
                self.close_session();
            }
        }
    }
}
```

`test_model` is declared here but written in Task 13; until then create `native/test_model.rs` containing only `//! The model test (Task 13).` so it compiles.

- [ ] **Step 4: Run** `cargo test -p pindot-core --test native_loop` → PASS (all three tests); `cargo test -p pindot-core --lib native` → PASS.

- [ ] **Step 5: Commit** "LoopBrain: Pindot's own loop as a brain".

### Task 13: The model test

**Files:** Write `native/test_model.rs`; add a test to `tests/native_loop.rs`.

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn the_model_test_passes_a_model_that_uses_tools_and_explains_one_that_does_not() {
    use pindot_core::brain::native::settings::{BrainKind, BrainSettings};
    use pindot_core::brain::native::test_model::test_model;
    let p = pindot();
    let brain = |url: &str| BrainSettings { kind: BrainKind::Loop, provider: "custom".into(), base_url: url.into(), model: "tiny".into(), ..BrainSettings::default() };
    let good = fake_model(&p.rt, vec![sse("ready", &[]), sse("", &[("c1", "desktop_context", "{}")]), sse("There is 1 display.", &[])]);
    let r = test_model(&brain(&good.base_url), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(r.ok, "{}", r.note);
    let no_tools = fake_model(&p.rt, vec![sse("Ready.", &[]), sse("I can't do that.", &[])]);
    let r = test_model(&brain(&no_tools.base_url), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(!r.ok);
    assert_eq!(r.note, "It answered but didn't call the tool: this model may not support tools.");
    let down = test_model(&brain("http://127.0.0.1:9/v1"), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(!down.ok && down.note.contains("Can't reach"), "{}", down.note);
}
```

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement** `native/test_model.rs`:

```rust
//! The model test: two short turns when a model is chosen. 1: a plain
//! reply ("ready"); 2: one call to `desktop_context` (read-only, no
//! approval) and an answer. It runs the loop directly, in a session of its
//! own that is ended afterwards, and writes no conversation file.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::mcp_client::McpClient;
use super::openai::{Endpoint, Msg, Who};
use super::run::{Conversation, TurnEnd, TurnEvent, TurnLimits, run_turn};
use super::settings::BrainSettings;
use super::{HttpModel, providers, system_prompt};
use crate::server::ServerState;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModelTest {
    pub ok: bool,
    /// One sentence on what failed; empty when it passed.
    pub note: String,
}

const LIMITS: TurnLimits = TurnLimits { max_tool_calls: 3, deadline: Duration::from_secs(180) };

pub fn test_model(brain: &BrainSettings, key: Option<String>, mcp_url: &str, token: &str, state: &ServerState) -> ModelTest {
    let provider = providers::find(&brain.provider);
    let who = Who {
        provider: provider.map_or_else(|| brain.provider.clone(), |p| p.name.to_string()),
        model: brain.model.clone(),
        local: provider.is_some_and(|p| p.local),
    };
    let mut model = HttpModel { endpoint: Endpoint { base_url: brain.base_url.clone(), key }, model: brain.model.clone() };
    let session = format!("model-test-{}", uuid::Uuid::new_v4());
    let mcp = McpClient::new(mcp_url, token, &session);
    let result = (|| {
        let tools = mcp.list_tools().map_err(|e| format!("Pindot's desktop tools did not answer: {e}"))?;
        let system = system_prompt("You are being tested. Do exactly what the user asks, briefly.", false);
        let cancel = AtomicBool::new(false);
        let conv = |text: &str| Conversation {
            messages: vec![Msg::System(system.clone()), Msg::User { text: text.into(), images: vec![] }],
            context_tokens: 32_768,
            vision: false,
        };
        let mut mcp_ref = &mcp;
        let mut c1 = conv("Reply with the single word: ready.");
        match run_turn(&mut c1, &mut model, &[], &mut mcp_ref, None, &cancel, &LIMITS, &who, &mut |_| {}) {
            TurnEnd::Done { last: Some(t) } if t.to_ascii_lowercase().contains("ready") => {}
            TurnEnd::Done { last } => return Err(format!("It didn't answer as asked (it said: {}).", last.unwrap_or_default().chars().take(80).collect::<String>())),
            TurnEnd::Failed(m) | TurnEnd::TooLong(m) => return Err(m),
            _ => return Err("It didn't answer.".into()),
        }
        let mut c2 = conv("Call the desktop_context tool, then say how many displays there are.");
        let mut used = false;
        let end = run_turn(&mut c2, &mut model, &tools, &mut mcp_ref, None, &cancel, &LIMITS, &who, &mut |e| {
            if e == TurnEvent::ToolUse("desktop_context".into()) {
                used = true;
            }
        });
        match end {
            TurnEnd::Failed(m) | TurnEnd::TooLong(m) => Err(m),
            _ if !used => Err("It answered but didn't call the tool: this model may not support tools.".into()),
            TurnEnd::Done { .. } => Ok(()),
            _ => Err("It called the tool but didn't finish its answer.".into()),
        }
    })();
    mcp.close();
    state.end_session(&session);
    match result {
        Ok(()) => ModelTest { ok: true, note: String::new() },
        Err(note) => ModelTest { ok: false, note },
    }
}
```

`run_turn` takes `&mut dyn Tools`; add to `run.rs` so a shared client works:

```rust
impl Tools for &McpClient {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput {
        McpClient::call(self, name, arguments)
    }
}
```

`McpClient` must be `Send` for `&McpClient: Send` (it is: `Mutex`, `AtomicU64`, `ureq::Agent`).

- [ ] **Step 4: Run** `cargo test -p pindot-core --test native_loop` → PASS. **Step 5: Commit** "The model test".

### Task 14: Start-up checks and wording that no longer assume Claude Code

**Files:** Modify `crates/pindot-core/src/voice/plan.rs`, `crates/pindot-core/src/voice/machine.rs`.

- [ ] **Step 1: Failing test** in `plan.rs` tests:

```rust
    #[test]
    fn a_loop_brain_needs_no_claude_code_but_needs_its_settings() {
        let f = Facts { loop_brain: true, claude_installed: false, ..ready_for_chat() };
        assert!(Plan::of(&f).chat_can_run());
        let f = Facts { loop_brain: true, claude_installed: false, brain_problems: vec!["Brain: choose a model".into()], ..ready_for_chat() };
        assert_eq!(Plan::of(&f).chat, vec!["Brain: choose a model".to_string()]);
    }
```

- [ ] **Step 2: Run** `cargo test -p pindot-core --lib voice::plan` → FAIL.

- [ ] **Step 3: Implement.** In `Facts` add:

```rust
    /// Pindot's own loop is the brain: Claude Code is not needed.
    pub loop_brain: bool,
    /// What is wrong with the loop brain's settings, in words.
    pub brain_problems: Vec<String>,
```

In `Plan::of`, wrap the Claude checks: `if f.loop_brain { chat.extend(f.brain_problems.iter().cloned()); } else if !f.claude_installed { ... } else if f.claude_signed_out { ... }`. Update the module comment ("Chat needs only the local server, a finished Setup and a brain: Claude Code, or a loop brain whose settings are complete.").

In `machine.rs`, replace both `"Claude Code is still starting. Try again in a moment."` with `"The assistant is still starting. Try again in a moment."`; update any test asserting the old text (`grep -n "still starting" crates/pindot-core/src/voice/machine.rs`).

- [ ] **Step 4: Run** `cargo test -p pindot-core --lib voice` → PASS. **Step 5: Commit** "Start-up checks for a loop brain; neutral wording while the brain starts".

### Task 15: The app starts the chosen brain

**Files:** Modify `app/src-tauri/src/voice.rs` (setup), `app/src-tauri/src/ui.rs` (`start_facts`, `settings_save`).

- [ ] **Step 1:** In `voice.rs` `setup()`, after `let config = BrainConfig {...};` build the sink once and choose:

```rust
    let (h, p, chat_app) = (handle.clone(), probe.clone(), app.clone());
    let sink: pindot_core::brain::persistent::InputSink = Arc::new(move |i: Input| {
        if let Some(p) = &p {
            p.input(&i);
        }
        if let Input::SessionReady { session_id } = &i
            && let Some(hub) = chat_app.try_state::<Arc<crate::chat::ChatHub>>()
        {
            hub.session_ready(session_id);
        }
        h.input(i)
    });
    let chosen = app.state::<crate::ui::Ui>().settings().brain;
    let brain: Box<dyn pindot_core::brain::Brain> = if chosen.is_loop() {
        use pindot_core::brain::native::secrets::{SecretStore, SystemSecrets};
        let key = SystemSecrets::default().get(&chosen.provider).unwrap_or_else(|e| {
            println!("voice: the {} key could not be read: {e}", chosen.provider);
            None
        });
        println!("voice: brain is Pindot's own loop ({} at {}, model {})", chosen.provider, chosen.base_url, chosen.model);
        Box::new(pindot_core::brain::native::LoopBrain::start(
            pindot_core::brain::native::LoopConfig {
                brain: chosen,
                key,
                mcp_url: config.mcp_url.clone(),
                token: config.token.clone(),
                system_prompt: config.system_prompt.clone(),
                data_dir: pindot_core::paths::data_dir()?,
            },
            state.clone(),
            sink,
        )?)
    } else {
        Box::new(PersistentBrain::start(config, state.clone(), sink)?)
    };
```

Remove the old `let brain = PersistentBrain::start(...)` block; `LiveIo { ..., brain, ... }` (already a `Box` after Task 1, so drop the `Box::new` added there).

- [ ] **Step 2:** In `ui.rs` `start_facts`, add `loop_brain: s.brain.is_loop(), brain_problems: s.brain.problems(),` and, to avoid running `claude auth status` for nothing, compute `let claude = if s.brain.is_loop() { ClaudeInfo::default() } else { claude_info() };` (if `ClaudeInfo` has no `Default`, derive it).

- [ ] **Step 3:** In `settings_save`, extend the restart condition with `|| !new.brain.same_brain(&old.brain)` and add, inside it:

```rust
        if !new.brain.same_brain(&old.brain) {
            notes.push("The assistant restarts with the new brain; the next request starts a fresh conversation.".into());
        }
```

- [ ] **Step 4: Build and test:** `cd app && npm run build && cd .. && cargo build -p pindot && cargo test -p pindot` → no errors, no warnings, tests pass.

- [ ] **Step 5: Commit** "The app starts the brain chosen in Settings".

### Task 16: Commands for choosing a brain

**Files:** Create `app/src-tauri/src/brain.rs`; modify `app/src-tauri/src/lib.rs` (`mod brain;` and handlers), `app/src/lib/api.ts`.

- [ ] **Step 1: Write `app/src-tauri/src/brain.rs`:**

```rust
//! Choosing a brain: the provider list, local servers, a provider's
//! models, its API key (stored, replaced or deleted, never shown) and the
//! model test.

use std::sync::Arc;

use pindot_core::brain::native::probe::{self, FoundServer, ModelInfo};
use pindot_core::brain::native::providers::{self, PROVIDERS};
use pindot_core::brain::native::secrets::{SecretStore, SystemSecrets, redact};
use pindot_core::brain::native::settings::{BrainSettings, TestResult};
use pindot_core::brain::native::test_model::test_model;
use pindot_core::server::{RunningServer, ServerState};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::ui::Ui;

#[derive(Serialize)]
pub struct ProviderView {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    local: bool,
    needs_key: bool,
    has_key: bool,
}

#[tauri::command]
pub async fn brain_providers() -> Vec<ProviderView> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = SystemSecrets::default();
        PROVIDERS
            .iter()
            .map(|p| ProviderView {
                id: p.id,
                name: p.name,
                base_url: p.base_url,
                local: p.local,
                needs_key: p.needs_key,
                has_key: !p.local && store.get(p.id).ok().flatten().is_some(),
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub async fn brain_find_local() -> Vec<FoundServer> {
    tauri::async_runtime::spawn_blocking(probe::find_local).await.unwrap_or_default()
}

#[tauri::command]
pub async fn brain_models(provider: String, base_url: String) -> Result<Vec<ModelInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = SystemSecrets::default().get(&provider).ok().flatten();
        let p = providers::find(&provider).ok_or("Unknown provider.")?;
        let who = pindot_core::brain::native::openai::Who { provider: p.name.into(), model: String::new(), local: p.local };
        probe::list_models(&provider, &base_url, key.as_deref())
            .map_err(|e| redact(&pindot_core::brain::native::openai::describe(&e, &who), key.as_deref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves the key; returns a warning when it looks like another provider's.
#[tauri::command]
pub async fn brain_key_set(provider: String, key: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = providers::find(&provider).ok_or("Unknown provider.")?;
        SystemSecrets::default().set(&provider, &key)?;
        Ok(providers::key_warning(p, &key))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn brain_key_delete(provider: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || SystemSecrets::default().delete(&provider)).await.map_err(|e| e.to_string())?
}

/// Runs the model test and records the result in settings.json.
#[tauri::command]
pub async fn brain_test(app: AppHandle, brain: BrainSettings) -> Result<TestResult, String> {
    let (Some(server), Some(state)) = (app.try_state::<RunningServer>(), app.try_state::<Arc<ServerState>>()) else {
        return Err("The MCP server is not running, so the desktop tools can't be tested.".into());
    };
    let (mcp_url, token, state) = (server.mcp_url(), server.token.get(), state.inner().clone());
    let b = brain.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let key = SystemSecrets::default().get(&b.provider).ok().flatten();
        let r = test_model(&b, key.clone(), &mcp_url, &token, &state);
        TestResult { base_url: b.base_url, model: b.model, ok: r.ok, note: redact(&r.note, key.as_deref()), at: crate::ui::now_secs() }
    })
    .await
    .map_err(|e| e.to_string())?;
    let ui = app.state::<Ui>();
    let mut s = ui.settings();
    s.brain.record_test(result.clone());
    s.save().map_err(|e| format!("Saving: {e:#}"))?;
    *ui.settings.lock().unwrap() = s;
    Ok(result)
}

/// Setup: use this brain now (saves only the brain block, restarts the assistant).
#[tauri::command]
pub fn brain_choose(app: AppHandle, brain: BrainSettings) -> Result<(), Vec<String>> {
    let problems = brain.problems();
    if !problems.is_empty() {
        return Err(problems);
    }
    let ui = app.state::<Ui>();
    let mut s = ui.settings();
    let restart = !s.brain.same_brain(&brain);
    let tested = std::mem::take(&mut s.brain.tested);
    s.brain = BrainSettings { tested, ..brain };
    s.save().map_err(|e| vec![format!("Saving: {e:#}")])?;
    *ui.settings.lock().unwrap() = s;
    if restart {
        crate::ui::restart_voice(&app);
    }
    crate::ui::changed(&app);
    Ok(())
}
```

Check against `ui.rs`: `Ui::settings()` returns a clone and `ui.settings` is `pub(crate)` (make it so if it isn't); `changed(&app)` and a `now_secs()` helper exist (`chat.rs` has one: move it to `ui.rs` as `pub fn now_secs()` if `ui.rs` lacks it); `SharedToken::get()` is the method name in `server/mod.rs` (read it).

- [ ] **Step 2:** In `lib.rs`: `mod brain;` and in `generate_handler!`: `brain::brain_providers, brain::brain_find_local, brain::brain_models, brain::brain_key_set, brain::brain_key_delete, brain::brain_test, brain::brain_choose,`.

- [ ] **Step 3:** In `app/src/lib/api.ts`, add the types and calls:

```ts
export type BrainKind = 'claude_code' | 'loop';
export type Vision = 'auto' | 'on' | 'off';

export interface TestResult {
  base_url: string;
  model: string;
  ok: boolean;
  note: string;
  at: number;
}

export interface BrainSettings {
  kind: BrainKind;
  provider: string;
  base_url: string;
  model: string;
  vision: Vision;
  context_tokens: number | null;
  tested: TestResult[];
}

export interface ProviderView {
  id: string;
  name: string;
  base_url: string;
  local: boolean;
  needs_key: boolean;
  has_key: boolean;
}

export interface ModelInfo {
  id: string;
  vision: boolean | null;
  tools: boolean | null;
  context: number | null;
}

export interface FoundServer {
  provider: string;
  name: string;
  base_url: string;
  models: ModelInfo[];
}
```

Add `brain: BrainSettings;` to the `Settings` interface, and to `api`:

```ts
  brainProviders: () => invoke<ProviderView[]>('brain_providers'),
  brainFindLocal: () => invoke<FoundServer[]>('brain_find_local'),
  brainModels: (provider: string, baseUrl: string) => invoke<ModelInfo[]>('brain_models', { provider, baseUrl }),
  brainKeySet: (provider: string, key: string) => invoke<string | null>('brain_key_set', { provider, key }),
  brainKeyDelete: (provider: string) => invoke<void>('brain_key_delete', { provider }),
  brainTest: (brain: BrainSettings) => invoke<TestResult>('brain_test', { brain }),
  brainChoose: (brain: BrainSettings) => invoke<void>('brain_choose', { brain }),
```

Also add `brain` to the settings mock in `docs/ui-pass/tools/tauri-mock.js` (copy the default block from the spec) so the screenshot tool keeps working.

- [ ] **Step 4:** `cd app && npm run check && npm test && npm run build && cd .. && cargo build -p pindot` → clean.

- [ ] **Step 5: Commit** "Commands for choosing a brain, its key and the model test".

### Task 17: The brain picker in Setup and Settings

**Files:** Create `app/src/lib/BrainPicker.svelte`; modify `app/src/views/Setup.svelte` (section 1), `app/src/views/Settings.svelte` (new Brain section before Claude), `app/src/App.svelte:86` (remove the "for Claude Code" span).

- [ ] **Step 1:** Read `Settings.svelte`'s Claude section and `Setup.svelte`'s section 1 for the class names in use (`row`, `small muted`, buttons). Write `BrainPicker.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type BrainSettings, type FoundServer, type ModelInfo, type ProviderView } from './api';

  /** `own`: Setup's use, with its own "Use this brain" button. In Settings
   *  the page's Save saves `brain`. */
  let { brain = $bindable(), claudeReady = false, own = false }: { brain: BrainSettings; claudeReady?: boolean; own?: boolean } = $props();

  let providers = $state<ProviderView[]>([]);
  let local = $state<FoundServer[] | null>(null);
  let models = $state<ModelInfo[]>([]);
  let modelsError = $state('');
  let key = $state('');
  let keyNote = $state('');
  let testing = $state(false);
  let saved = $state('');
  let problems = $state<string[]>([]);

  const provider = $derived(providers.find((p) => p.id === brain.provider));
  const tested = $derived(brain.tested.find((t) => t.base_url === brain.base_url && t.model === brain.model));
  const choice = $derived(brain.kind === 'claude_code' ? 'claude' : provider?.local ? 'local' : 'api');

  onMount(async () => {
    providers = await api.brainProviders();
    if (brain.kind === 'loop') await loadModels();
  });

  function pick(c: 'claude' | 'local' | 'api') {
    saved = '';
    if (c === 'claude') {
      brain.kind = 'claude_code';
      return;
    }
    brain.kind = 'loop';
    if (c === 'local') {
      findLocal();
    } else if (provider?.local !== false) {
      setProvider('openai');
    }
  }

  async function findLocal() {
    local = null;
    local = await api.brainFindLocal();
    const first = local[0];
    if (first && !local.some((s) => s.provider === brain.provider)) {
      brain.provider = first.provider;
      brain.base_url = first.base_url;
      models = first.models;
      brain.model = first.models[0]?.id ?? '';
    } else {
      await loadModels();
    }
  }

  async function setProvider(id: string) {
    const p = providers.find((x) => x.id === id);
    if (!p) return;
    brain.provider = id;
    if (p.base_url) brain.base_url = p.base_url;
    brain.model = '';
    keyNote = '';
    await loadModels();
  }

  async function loadModels() {
    modelsError = '';
    models = [];
    if (!brain.base_url) return;
    try {
      models = await api.brainModels(brain.provider, brain.base_url);
      if (!brain.model && models[0]) brain.model = models[0].id;
    } catch (e) {
      modelsError = String(e);
    }
  }

  async function saveKey() {
    try {
      const warning = await api.brainKeySet(brain.provider, key);
      key = '';
      keyNote = warning ?? 'Key saved in your system keychain.';
      providers = await api.brainProviders();
      await loadModels();
    } catch (e) {
      keyNote = String(e);
    }
  }

  async function deleteKey() {
    await api.brainKeyDelete(brain.provider);
    keyNote = 'Key deleted.';
    providers = await api.brainProviders();
  }

  async function test() {
    testing = true;
    try {
      const r = await api.brainTest($state.snapshot(brain));
      brain.tested = [r, ...brain.tested.filter((t) => !(t.base_url === r.base_url && t.model === r.model))];
    } catch (e) {
      brain.tested = [{ base_url: brain.base_url, model: brain.model, ok: false, note: String(e), at: 0 }, ...brain.tested];
    } finally {
      testing = false;
    }
  }

  async function use() {
    problems = [];
    try {
      await api.brainChoose($state.snapshot(brain));
      saved = 'Saved. The assistant restarts with this brain.';
      if (brain.kind === 'loop' && !tested) await test();
    } catch (e) {
      problems = Array.isArray(e) ? e : [String(e)];
    }
  }

  const modelLabel = (m: ModelInfo) =>
    [m.id, m.vision ? 'sees pictures' : null, m.tools === false ? 'no tools' : null].filter(Boolean).join(' · ');
</script>

<div class="picker">
  <div class="choices" role="radiogroup" aria-label="Brain">
    <label class="choice"><input type="radio" name="brain" checked={choice === 'claude'} onchange={() => pick('claude')} />
      <span><strong>Claude Code</strong><br /><span class="small muted">Your own Claude Code and plan.{#if claudeReady} Ready.{/if}</span></span></label>
    <label class="choice"><input type="radio" name="brain" checked={choice === 'local'} onchange={() => pick('local')} />
      <span><strong>A model on this computer</strong><br /><span class="small muted">Ollama, LM Studio or llama.cpp. Nothing leaves your computer.</span></span></label>
    <label class="choice"><input type="radio" name="brain" checked={choice === 'api'} onchange={() => pick('api')} />
      <span><strong>An API key</strong><br /><span class="small muted">OpenAI, OpenRouter, DeepSeek, Groq or another OpenAI-compatible service.</span></span></label>
  </div>

  {#if choice === 'local'}
    {#if local === null}
      <p class="small muted">Looking for Ollama, LM Studio and llama.cpp...</p>
    {:else if local.length === 0 && !models.length}
      <p class="small">No model server is running. Start Ollama (<code>ollama serve</code>) or LM Studio's server, then <button type="button" onclick={findLocal}>Look again</button></p>
    {:else}
      <label class="row">Server
        <select value={brain.provider} onchange={(e) => setProvider(e.currentTarget.value)}>
          {#each providers.filter((p) => p.local) as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
        </select>
      </label>
    {/if}
  {:else if choice === 'api'}
    <label class="row">Provider
      <select value={brain.provider} onchange={(e) => setProvider(e.currentTarget.value)}>
        {#each providers.filter((p) => !p.local) as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
      </select>
    </label>
    {#if brain.provider === 'custom'}
      <label class="row">Address <input type="url" bind:value={brain.base_url} placeholder="https://example.com/v1" onchange={loadModels} /></label>
    {/if}
    <div class="row">
      <input type="password" bind:value={key} placeholder={provider?.has_key ? 'A key is saved (paste to replace it)' : 'Paste your API key'} autocomplete="off" />
      <button type="button" onclick={saveKey} disabled={!key.trim()}>Save key</button>
      {#if provider?.has_key}<button type="button" onclick={deleteKey}>Delete key</button>{/if}
    </div>
    {#if keyNote}<p class="small">{keyNote}</p>{/if}
  {/if}

  {#if brain.kind === 'loop'}
    {#if modelsError}<p class="small error">{modelsError}</p>{/if}
    <label class="row">Model
      {#if models.length}
        <select bind:value={brain.model}>
          {#each models as m (m.id)}<option value={m.id}>{modelLabel(m)}</option>{/each}
        </select>
      {:else}
        <input bind:value={brain.model} placeholder="Model name" />
      {/if}
      <button type="button" onclick={loadModels}>Refresh</button>
    </label>
    <label class="row">Pictures
      <select bind:value={brain.vision}>
        <option value="auto">Automatic</option><option value="on">This model can see</option><option value="off">Text only</option>
      </select>
    </label>
    <div class="row">
      <button type="button" onclick={test} disabled={testing || !brain.model}>{testing ? 'Testing...' : 'Test this model'}</button>
      {#if tested}
        <span class="small" class:error={!tested.ok}>{tested.ok ? 'Passed the test.' : `Didn't pass: ${tested.note} You can keep it anyway.`}</span>
      {/if}
    </div>
    {#if brain.provider === 'ollama'}
      <p class="small muted">Ollama gives models a short memory by default. For long requests, start it with <code>OLLAMA_CONTEXT_LENGTH=32768</code>.</p>
    {/if}
  {/if}

  {#if own}
    <div class="row"><button type="button" class="primary" onclick={use}>Use this brain</button>{#if saved}<span class="small">{saved}</span>{/if}</div>
    {#each problems as p}<p class="small error">{p}</p>{/each}
  {/if}
</div>

<style>
  .picker { display: grid; gap: 10px; }
  .choices { display: grid; gap: 8px; }
  .choice { display: flex; gap: 10px; align-items: flex-start; cursor: pointer; }
  .error { color: var(--danger, #e5484d); }
</style>
```

- [ ] **Step 2:** `Setup.svelte` section 1: title "1. Choose a brain"; keep the Claude Code install/sign-in status block, shown only when `settings.brain.kind === 'claude_code'`; add `<BrainPicker bind:brain={settings.brain} claudeReady={claude?.signed_in} own />` (load `settings` with `api.settingsGet()` if Setup doesn't have it). Settings.svelte: a `<section><h2>Brain</h2><BrainPicker bind:brain={view.settings.brain} /></section>` before the Claude section, and a line under the Claude heading: `{#if view.settings.brain.kind === 'loop'}<p class="small muted">These apply to Claude Code only.</p>{/if}`; the same line in the Brain folder section. `App.svelte`: delete `<span>for Claude Code</span>`.

- [ ] **Step 3:** `cd app && npm run check && npm test && npm run build` → clean. Re-render the screens: `bash docs/ui-pass/tools/render-design.sh` (if it needs a running WebKit tool, at least `shots.sh` for `settings` and `setup`) and look at `settings.png` and `setup-first-run.png`.

- [ ] **Step 4: Commit** "Choosing a brain in Setup and Settings".

### Task 18: Docs and rules

**Files:** `CLAUDE.md`, `CONTRIBUTING.md`, `README.md`, `docs/development.md`.

- [ ] **Step 1: CLAUDE.md**, Hard rules: replace the first bullet with:

```markdown
- **The Claude Code brain = the user's interactive `claude` CLI in a PTY.**
  Never `claude -p` or `--print`, never the Agent SDK, never an API key for
  Claude Code. If interactive mode is blocked, stop and ask; do not fall
  back. Pindot's own loop (`brain/native/`) is a separate brain for local
  models and other providers; their keys live only in the OS credential
  store (`native/secrets.rs`), never in a file, log or error.
```

and add a section "### Pindot's own loop (16 Nov 2026)" summarising the spec's decisions (desktop tools only, through the MCP server with `X-Pindot-Session`; one brain at a time; model test warns; conversations in `<data dir>/conversations/` in Chat's line format; vision off for unknown models; Ollama context 8192 unless set; keys in "Pindot AI keys").

- [ ] **Step 2: CONTRIBUTING.md:** replace the brain bullet with: "Claude Code stays the interactive `claude` CLI in a PTY: pull requests that add `claude -p`, the Agent SDK or an API key for Claude Code, or `--dangerously-skip-permissions`, will not be merged. Keys for other providers go through `brain/native/secrets.rs` only."

- [ ] **Step 3: README.md:** "What you need" → a brain: Claude Code, a local model server (Ollama, LM Studio or llama.cpp), or an API key for an OpenAI-compatible service; a "Choosing a brain" subsection under "Using Pindot" (the three choices, the model test, that small local models may not manage tools, the Ollama context tip); Privacy → "With a local model nothing leaves your computer. With an API key, what Claude would see goes to that provider instead." First start step 1 → "Choose a brain".

- [ ] **Step 4: docs/development.md:** a "Pindot's own loop" section listing the files of the file map above and how to try it: `ollama pull qwen3:8b`, start Pindot, Settings > Brain > A model on this computer.

- [ ] **Step 5: Commit** "Docs: brains other than Claude Code".

### Task 19: Full check and a real run

- [ ] **Step 1:** `cargo test --workspace` (log, tail), `cd app && npm run check && npm test`. All pass, no warnings.
- [ ] **Step 2:** `scripts/build-mac.sh` (quit the running Pindot first; retry once if Apple's timestamp server fails), relaunch.
- [ ] **Step 3:** If Ollama is installed (`which ollama`), with a small tool-using model already pulled (do not download large models without asking Tony): build a test copy (`--features test-switches`), run it with a scratch `PINDOT_DATA_DIR` whose settings choose the loop brain, `PINDOT_TEST_AUTO_APPROVE=test-target`, and the Swift test window; type a Chat request "press the Pindot Test Button" via the voice runner's `TypedPrompt` path (or `--test-voice` with a WAV saying it). Check the window logged a click and the activity log shows the loop's session. If Ollama is not installed, say so in the report instead.
- [ ] **Step 4:** Report to Tony: what passed, what was not run and why. Push only when Tony says.
