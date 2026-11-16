//! What has to be true before the assistant (Chat) starts, and what
//! has to be true on top of that before hold-to-talk starts.
//!
//! Chat needs only the local server, a finished Setup and a brain: Claude
//! Code, or a loop brain (Pindot's own) whose settings are complete.
//! Voice also needs hold-to-talk switched on, both voice models and the
//! microphone, accessibility and input-monitoring permissions. Typed chat
//! never waits for any of the voice parts.

/// What the app knows right now. Plain facts, so the decision is testable.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `--voice`: start without waiting for Setup.
    pub force: bool,
    pub voice_enabled: bool,
    pub onboarding_done: bool,
    pub server_running: bool,
    pub stt_ready: bool,
    pub tts_ready: bool,
    pub claude_installed: bool,
    /// Installed and signed out (and no other problem was found).
    pub claude_signed_out: bool,
    /// The words for every missing permission that blocks voice.
    pub permission_problems: Vec<String>,
    /// Pindot's own loop is the brain: Claude Code is not needed.
    pub loop_brain: bool,
    /// What is wrong with the loop brain's settings, in words.
    pub brain_problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Why the assistant cannot start at all. Empty: Chat can run.
    pub chat: Vec<String>,
    /// Why hold-to-talk cannot start. Empty: voice can run.
    pub voice: Vec<String>,
    /// The user switched hold-to-talk off (so `voice` is just that note).
    pub voice_switched_off: bool,
}

impl Plan {
    pub fn of(f: &Facts) -> Plan {
        let mut chat = Vec::new();
        if !f.force && !f.onboarding_done {
            chat.push("finish Setup first".to_string());
        }
        if !f.server_running {
            chat.push("the MCP server is not running".into());
        }
        if f.loop_brain {
            chat.extend(f.brain_problems.iter().cloned());
        } else if !f.claude_installed {
            chat.push("Claude Code (`claude`) is not installed".into());
        } else if f.claude_signed_out {
            chat.push("Claude Code is not signed in (run `claude login`)".into());
        }

        let mut voice = Vec::new();
        let switched_off = !f.force && !f.voice_enabled;
        if switched_off {
            voice.push("hold-to-talk is turned off".to_string());
        } else {
            if !f.stt_ready {
                voice.push("the speech-to-text model is not downloaded".into());
            }
            if !f.tts_ready {
                voice.push("the text-to-speech model is not downloaded".into());
            }
            voice.extend(f.permission_problems.iter().cloned());
        }
        Plan { chat, voice, voice_switched_off: switched_off }
    }

    pub fn chat_can_run(&self) -> bool {
        self.chat.is_empty()
    }

    pub fn voice_can_run(&self) -> bool {
        self.chat.is_empty() && self.voice.is_empty()
    }

    /// Everything that is in the way of the whole thing, in words. When the
    /// user switched hold-to-talk off, that alone.
    pub fn all(&self) -> Vec<String> {
        if self.voice_switched_off {
            return self.voice.clone();
        }
        self.chat.iter().chain(self.voice.iter()).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_for_chat() -> Facts {
        Facts { onboarding_done: true, server_running: true, claude_installed: true, voice_enabled: true, ..Facts::default() }
    }

    #[test]
    fn chat_runs_with_no_models_and_no_microphone_permission() {
        let f = Facts {
            stt_ready: false,
            tts_ready: false,
            permission_problems: vec!["Microphone permission is missing".into(), "Accessibility permission is missing".into()],
            ..ready_for_chat()
        };
        let p = Plan::of(&f);
        assert!(p.chat_can_run(), "{:?}", p.chat);
        assert!(!p.voice_can_run());
        assert_eq!(p.voice.len(), 4);
    }

    #[test]
    fn chat_runs_with_voice_switched_off() {
        let f = Facts { voice_enabled: false, ..ready_for_chat() };
        let p = Plan::of(&f);
        assert!(p.chat_can_run());
        assert!(!p.voice_can_run());
        assert_eq!(p.all(), vec!["hold-to-talk is turned off".to_string()]);
    }

    #[test]
    fn chat_still_needs_the_server_claude_and_setup() {
        let p = Plan::of(&Facts { voice_enabled: true, ..Facts::default() });
        assert!(!p.chat_can_run());
        assert_eq!(p.chat.len(), 3);
        let signed_out = Plan::of(&Facts { claude_signed_out: true, ..ready_for_chat() });
        assert_eq!(signed_out.chat, vec!["Claude Code is not signed in (run `claude login`)".to_string()]);
    }

    #[test]
    fn force_skips_setup_and_the_voice_switch() {
        let f = Facts { force: true, server_running: true, claude_installed: true, stt_ready: true, tts_ready: true, ..Facts::default() };
        let p = Plan::of(&f);
        assert!(p.voice_can_run(), "{p:?}");
    }

    #[test]
    fn everything_ready_has_no_blockers() {
        let f = Facts { stt_ready: true, tts_ready: true, ..ready_for_chat() };
        assert!(Plan::of(&f).all().is_empty());
    }

    #[test]
    fn a_loop_brain_needs_no_claude_code_but_needs_its_settings() {
        let f = Facts { loop_brain: true, claude_installed: false, ..ready_for_chat() };
        assert!(Plan::of(&f).chat_can_run(), "{:?}", Plan::of(&f).chat);
        let f = Facts { loop_brain: true, claude_installed: false, brain_problems: vec!["Brain: choose a model".into()], ..ready_for_chat() };
        assert_eq!(Plan::of(&f).chat, vec!["Brain: choose a model".to_string()]);
    }
}
