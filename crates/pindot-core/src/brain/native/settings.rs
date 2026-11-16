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
    /// The user's ChatGPT plan through OpenAI's Codex CLI (`brain::codex`).
    Codex,
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

    pub fn uses_claude_code(&self) -> bool {
        self.kind == BrainKind::ClaudeCode
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
        assert!(BrainSettings::default().uses_claude_code());
    }

    #[test]
    fn codex_needs_no_model_and_reads_back() {
        let b = BrainSettings { kind: BrainKind::Codex, model: String::new(), ..BrainSettings::default() };
        assert!(b.problems().is_empty());
        assert!(!b.uses_claude_code() && !b.is_loop());
        let text = serde_json::to_string(&b).unwrap();
        assert!(text.contains("\"kind\":\"codex\""), "{text}");
        assert_eq!(serde_json::from_str::<BrainSettings>(&text).unwrap(), b);
    }

    #[test]
    fn a_loop_brain_is_checked() {
        assert!(loop_brain().problems().is_empty(), "{:?}", loop_brain().problems());
        let b = BrainSettings {
            model: " ".into(),
            base_url: "ftp://x".into(),
            provider: "nope".into(),
            context_tokens: Some(10),
            ..loop_brain()
        };
        assert_eq!(b.problems().len(), 4, "{:?}", b.problems());
        let plain = BrainSettings { provider: "openai".into(), base_url: "http://192.168.1.2/v1".into(), ..loop_brain() };
        assert!(plain.problems().iter().any(|p| p.contains("https://")), "{:?}", plain.problems());
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
