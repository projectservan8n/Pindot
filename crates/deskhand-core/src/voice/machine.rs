//! The voice loop as a pure state machine: inputs in, effects out, no I/O,
//! no clock of its own (every input carries `now`). The runner executes the
//! effects; the tests drive it directly.
//!
//! Shown state, highest first: Listening (hotkey held), Acting (a tool is
//! running), Speaking, Thinking (transcribing or waiting for Claude), Idle.
//!
//! Rules:
//! - Hotkey down while speaking stops the speech and listens.
//! - Hotkey down while Claude is still working interrupts that turn first
//!   (barge-in): the new words replace the old request.
//! - Esc (the Phase 1 emergency stop) drops the recording, stops speech and
//!   interrupts the turn. Control has already cancelled the action in flight.
//! - With `SpeakMode::EachMessage`, every assistant text message of the live
//!   turn is spoken as it lands in the transcript; at Stop, the final message
//!   is spoken only if it was not spoken already.
//! - Transcript events only count after the live turn's own prompt has shown
//!   up in the transcript, so leftovers of an interrupted turn are ignored.
//! - After `idle_reset` with nothing going on, a used session is replaced by
//!   a fresh one.

use std::time::{Duration, Instant};

use super::{SpeakMode, speakable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    Idle,
    Listening,
    Thinking,
    Acting,
    Speaking,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    HotkeyDown,
    HotkeyUp,
    /// The hold turned out to be a shortcut (another key joined): drop the
    /// recording. Speech or a turn it already stopped stays stopped.
    HotkeyCancel,
    /// The global Esc (Control broadcast `EmergencyStop`).
    Esc,
    /// Speech to text finished for recording `utterance`.
    Transcribed { utterance: u64, text: String },
    TranscribeFailed { utterance: u64, error: String },
    /// The brain started (or restarted) a `claude` session.
    SessionReady { session_id: String },
    /// The brain cannot run: not installed, not signed in, crashed.
    BrainUnavailable { message: String },
    /// The Stop hook fired for `session_id`.
    Stop { session_id: String, last_message: Option<String> },
    /// The turn cannot finish (permission prompt, `claude` exited, timeout).
    TurnFailed { message: String },
    /// Transcript: a typed user prompt was recorded.
    TranscriptPrompt { session_id: String },
    /// Transcript: an assistant text message.
    TranscriptText { session_id: String, text: String },
    TranscriptToolUse { session_id: String, name: String },
    TranscriptToolResult { session_id: String },
    /// The speaker has finished everything up to and including `seq`.
    SpeechIdle { seq: u64 },
    /// Periodic tick (idle reset).
    Tick,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    StartRecording { utterance: u64 },
    /// Stop recording and transcribe; the result comes back as `Transcribed`.
    FinishRecording { utterance: u64 },
    DiscardRecording,
    SendPrompt { text: String },
    InterruptTurn,
    NewSession,
    Speak { seq: u64, text: String },
    StopSpeech,
    /// Arm or disarm the global Esc for the voice interaction.
    ArmEsc,
    DisarmEsc,
    Show(Display),
    /// A short message for the user (overlay pill).
    Notice(String),
}

#[derive(Debug, Default)]
struct Turn {
    /// The turn's own prompt has appeared in the transcript.
    prompt_seen: bool,
    tools_running: u32,
    /// Normalised texts already spoken in this turn.
    spoken: Vec<String>,
}

pub struct Machine {
    speak_mode: SpeakMode,
    idle_reset: Duration,
    session: Option<String>,
    session_used: bool,
    brain_problem: Option<String>,
    listening: bool,
    utterance: u64,
    transcribing: bool,
    turn: Option<Turn>,
    speak_seq: u64,
    speaking: bool,
    last_active: Instant,
    shown: Display,
    armed: bool,
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

impl Machine {
    pub fn new(speak_mode: SpeakMode, idle_reset: Duration, now: Instant) -> Self {
        Self {
            speak_mode,
            idle_reset,
            session: None,
            session_used: false,
            brain_problem: None,
            listening: false,
            utterance: 0,
            transcribing: false,
            turn: None,
            speak_seq: 0,
            speaking: false,
            last_active: now,
            shown: Display::Idle,
            armed: false,
        }
    }

    pub fn display(&self) -> Display {
        if self.listening {
            Display::Listening
        } else if self.turn.as_ref().is_some_and(|t| t.tools_running > 0) {
            Display::Acting
        } else if self.speaking {
            Display::Speaking
        } else if self.transcribing || self.turn.is_some() {
            Display::Thinking
        } else {
            Display::Idle
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// True while a turn is live (prompt sent, no Stop yet).
    pub fn turn_live(&self) -> bool {
        self.turn.is_some()
    }

    pub fn handle(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        if input != Input::Tick {
            self.last_active = now;
        }
        match input {
            Input::HotkeyDown => {
                if !self.listening {
                    if self.speaking {
                        fx.push(Effect::StopSpeech);
                        self.speaking = false;
                    }
                    if self.turn.take().is_some() {
                        fx.push(Effect::InterruptTurn);
                    }
                    // A transcription still running for an older recording is dropped.
                    self.transcribing = false;
                    self.utterance += 1;
                    self.listening = true;
                    fx.push(Effect::StartRecording { utterance: self.utterance });
                }
            }
            Input::HotkeyUp => {
                if self.listening {
                    self.listening = false;
                    self.transcribing = true;
                    fx.push(Effect::FinishRecording { utterance: self.utterance });
                }
            }
            Input::HotkeyCancel => {
                if self.listening {
                    self.listening = false;
                    fx.push(Effect::DiscardRecording);
                }
            }
            Input::Esc => {
                if self.listening {
                    self.listening = false;
                    fx.push(Effect::DiscardRecording);
                }
                if self.transcribing {
                    self.transcribing = false;
                    self.utterance += 1;
                }
                if self.speaking {
                    self.speaking = false;
                    fx.push(Effect::StopSpeech);
                }
                if self.turn.take().is_some() {
                    fx.push(Effect::InterruptTurn);
                }
            }
            Input::Transcribed { utterance, text } => {
                if utterance == self.utterance && self.transcribing {
                    self.transcribing = false;
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        fx.push(Effect::Notice("I didn't catch that.".into()));
                    } else if let Some(problem) = &self.brain_problem {
                        fx.push(Effect::Notice(problem.clone()));
                    } else if self.session.is_none() {
                        fx.push(Effect::Notice("Claude Code is still starting. Try again in a moment.".into()));
                    } else {
                        self.session_used = true;
                        self.turn = Some(Turn::default());
                        fx.push(Effect::SendPrompt { text });
                    }
                }
            }
            Input::TranscribeFailed { utterance, error } => {
                if utterance == self.utterance && self.transcribing {
                    self.transcribing = false;
                    fx.push(Effect::Notice(format!("Speech recognition failed: {error}")));
                }
            }
            Input::SessionReady { session_id } => {
                self.brain_problem = None;
                self.session = Some(session_id);
            }
            Input::BrainUnavailable { message } => {
                self.session = None;
                self.brain_problem = Some(message.clone());
                self.turn = None;
                fx.push(Effect::Notice(message));
            }
            Input::Stop { session_id, last_message } => {
                if self.session.as_deref() == Some(session_id.as_str())
                    && let Some(turn) = self.turn.take()
                {
                    if let Some(msg) = last_message {
                        let say = speakable(&msg);
                        if !say.is_empty() && !turn.spoken.contains(&norm(&say)) {
                            self.speak(say, &mut fx);
                        }
                    }
                }
            }
            Input::TurnFailed { message } => {
                if self.turn.take().is_some() {
                    fx.push(Effect::Notice(message));
                }
            }
            Input::TranscriptPrompt { session_id } => {
                if self.is_current(&session_id)
                    && let Some(t) = self.turn.as_mut()
                {
                    t.prompt_seen = true;
                }
            }
            Input::TranscriptText { session_id, text } => {
                if self.speak_mode == SpeakMode::EachMessage
                    && self.is_current(&session_id)
                    && let Some(t) = self.turn.as_mut()
                    && t.prompt_seen
                {
                    let say = speakable(&text);
                    let n = norm(&say);
                    if !say.is_empty() && !t.spoken.contains(&n) {
                        t.spoken.push(n);
                        self.speak(say, &mut fx);
                    }
                }
            }
            Input::TranscriptToolUse { session_id, .. } => {
                if self.is_current(&session_id)
                    && let Some(t) = self.turn.as_mut()
                    && t.prompt_seen
                {
                    t.tools_running += 1;
                }
            }
            Input::TranscriptToolResult { session_id } => {
                if self.is_current(&session_id)
                    && let Some(t) = self.turn.as_mut()
                    && t.prompt_seen
                {
                    t.tools_running = t.tools_running.saturating_sub(1);
                }
            }
            Input::SpeechIdle { seq } => {
                if seq >= self.speak_seq {
                    self.speaking = false;
                }
            }
            Input::Tick => {
                if self.display() == Display::Idle
                    && self.session_used
                    && now.duration_since(self.last_active) >= self.idle_reset
                {
                    self.session_used = false;
                    self.session = None;
                    fx.push(Effect::NewSession);
                }
            }
        }
        self.sync_display(&mut fx);
        fx
    }

    fn is_current(&self, session_id: &str) -> bool {
        self.session.as_deref() == Some(session_id)
    }

    fn speak(&mut self, text: String, fx: &mut Vec<Effect>) {
        self.speak_seq += 1;
        self.speaking = true;
        fx.push(Effect::Speak { seq: self.speak_seq, text });
    }

    /// Emit `Show` and Esc arming when the shown state changes. Esc is armed
    /// for the whole interaction: from hotkey down until back to Idle.
    fn sync_display(&mut self, fx: &mut Vec<Effect>) {
        let d = self.display();
        let want_armed = d != Display::Idle;
        if want_armed && !self.armed {
            // Before anything else, so Esc works from the first moment.
            fx.insert(0, Effect::ArmEsc);
        }
        if d != self.shown {
            fx.push(Effect::Show(d));
            self.shown = d;
        }
        if !want_armed && self.armed {
            fx.push(Effect::DisarmEsc);
        }
        self.armed = want_armed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "sess-1";

    fn ready(mode: SpeakMode) -> (Machine, Instant) {
        let t = Instant::now();
        let mut m = Machine::new(mode, Duration::from_secs(600), t);
        m.handle(Input::SessionReady { session_id: S.into() }, t);
        (m, t)
    }

    fn has(fx: &[Effect], e: &Effect) -> bool {
        fx.contains(e)
    }

    fn speaks(fx: &[Effect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Speak { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Hold, release, transcribe: returns the effects of the transcription.
    fn ask(m: &mut Machine, t: Instant, text: &str) -> Vec<Effect> {
        m.handle(Input::HotkeyDown, t);
        m.handle(Input::HotkeyUp, t);
        let u = m.utterance;
        m.handle(Input::Transcribed { utterance: u, text: text.into() }, t)
    }

    #[test]
    fn full_turn_walks_listening_thinking_acting_speaking_idle() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        let fx = m.handle(Input::HotkeyDown, t);
        assert_eq!(fx, vec![Effect::ArmEsc, Effect::StartRecording { utterance: 1 }, Effect::Show(Display::Listening)]);
        let fx = m.handle(Input::HotkeyUp, t);
        assert_eq!(fx, vec![Effect::FinishRecording { utterance: 1 }, Effect::Show(Display::Thinking)]);
        let fx = m.handle(Input::Transcribed { utterance: 1, text: " open text edit ".into() }, t);
        assert_eq!(fx, vec![Effect::SendPrompt { text: "open text edit".into() }]);
        assert_eq!(m.display(), Display::Thinking);

        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        let fx = m.handle(Input::TranscriptToolUse { session_id: S.into(), name: "mcp__desktop__open_app".into() }, t);
        assert_eq!(fx, vec![Effect::Show(Display::Acting)]);
        let fx = m.handle(Input::TranscriptToolResult { session_id: S.into() }, t);
        assert_eq!(fx, vec![Effect::Show(Display::Thinking)]);

        let fx = m.handle(Input::TranscriptText { session_id: S.into(), text: "**Done.** TextEdit is open.".into() }, t);
        assert_eq!(speaks(&fx), vec!["Done. TextEdit is open."]);
        assert!(has(&fx, &Effect::Show(Display::Speaking)));

        // The Stop hook repeats the same final message: not spoken twice.
        let fx = m.handle(Input::Stop { session_id: S.into(), last_message: Some("**Done.** TextEdit is open.".into()) }, t);
        assert!(speaks(&fx).is_empty());
        assert!(!m.turn_live());
        assert_eq!(m.display(), Display::Speaking);

        let fx = m.handle(Input::SpeechIdle { seq: 1 }, t);
        assert_eq!(fx, vec![Effect::Show(Display::Idle), Effect::DisarmEsc]);
    }

    #[test]
    fn at_stop_mode_speaks_only_the_final_message() {
        let (mut m, t) = ready(SpeakMode::AtStop);
        ask(&mut m, t, "hello");
        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        let fx = m.handle(Input::TranscriptText { session_id: S.into(), text: "Working on it.".into() }, t);
        assert!(speaks(&fx).is_empty());
        let fx = m.handle(Input::Stop { session_id: S.into(), last_message: Some("Hi there.".into()) }, t);
        assert_eq!(speaks(&fx), vec!["Hi there."]);
    }

    #[test]
    fn final_message_is_spoken_at_stop_when_the_transcript_lags() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        ask(&mut m, t, "hello");
        let fx = m.handle(Input::Stop { session_id: S.into(), last_message: Some("Hi.".into()) }, t);
        assert_eq!(speaks(&fx), vec!["Hi."]);
        // The transcript catches up after the turn ended: nothing more.
        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        let fx = m.handle(Input::TranscriptText { session_id: S.into(), text: "Hi.".into() }, t);
        assert!(speaks(&fx).is_empty());
    }

    #[test]
    fn hotkey_during_speech_stops_it_and_listens() {
        let (mut m, t) = ready(SpeakMode::AtStop);
        ask(&mut m, t, "hello");
        m.handle(Input::Stop { session_id: S.into(), last_message: Some("A long answer.".into()) }, t);
        assert_eq!(m.display(), Display::Speaking);
        let fx = m.handle(Input::HotkeyDown, t);
        assert_eq!(fx[0], Effect::StopSpeech);
        assert!(!has(&fx, &Effect::InterruptTurn), "the turn had ended; nothing to interrupt");
        assert!(has(&fx, &Effect::Show(Display::Listening)));
        assert!(!has(&fx, &Effect::DisarmEsc));
        // A late idle report for the stopped speech changes nothing.
        let fx = m.handle(Input::SpeechIdle { seq: 1 }, t);
        assert!(fx.is_empty());
        assert_eq!(m.display(), Display::Listening);
    }

    #[test]
    fn hotkey_while_claude_works_interrupts_the_turn() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        ask(&mut m, t, "open text edit");
        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        m.handle(Input::TranscriptToolUse { session_id: S.into(), name: "x".into() }, t);
        assert_eq!(m.display(), Display::Acting);
        let fx = m.handle(Input::HotkeyDown, t);
        assert!(has(&fx, &Effect::InterruptTurn));
        assert!(has(&fx, &Effect::Show(Display::Listening)));
        // Leftovers of the interrupted turn are ignored until the new prompt shows.
        let fx = ask_rest(&mut m, t, "type hello");
        assert_eq!(fx, vec![Effect::SendPrompt { text: "type hello".into() }]);
        let fx = m.handle(Input::TranscriptText { session_id: S.into(), text: "Old reply.".into() }, t);
        assert!(speaks(&fx).is_empty());
        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        let fx = m.handle(Input::TranscriptText { session_id: S.into(), text: "New reply.".into() }, t);
        assert_eq!(speaks(&fx), vec!["New reply."]);
    }

    fn ask_rest(m: &mut Machine, t: Instant, text: &str) -> Vec<Effect> {
        m.handle(Input::HotkeyUp, t);
        let u = m.utterance;
        m.handle(Input::Transcribed { utterance: u, text: text.into() }, t)
    }

    #[test]
    fn hotkey_cancel_drops_the_recording() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        m.handle(Input::HotkeyDown, t);
        let fx = m.handle(Input::HotkeyCancel, t);
        assert!(has(&fx, &Effect::DiscardRecording));
        assert!(has(&fx, &Effect::Show(Display::Idle)));
        // No transcription follows, so nothing is sent.
        let fx = m.handle(Input::HotkeyUp, t);
        assert!(fx.iter().all(|e| !matches!(e, Effect::FinishRecording { .. })));
        // A cancel with nothing recording does nothing.
        assert!(!has(&m.handle(Input::HotkeyCancel, t), &Effect::DiscardRecording));
    }

    #[test]
    fn esc_stops_everything() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        ask(&mut m, t, "open text edit");
        m.handle(Input::TranscriptPrompt { session_id: S.into() }, t);
        m.handle(Input::TranscriptText { session_id: S.into(), text: "Opening it.".into() }, t);
        m.handle(Input::TranscriptToolUse { session_id: S.into(), name: "x".into() }, t);
        let fx = m.handle(Input::Esc, t);
        assert!(has(&fx, &Effect::StopSpeech));
        assert!(has(&fx, &Effect::InterruptTurn));
        assert!(has(&fx, &Effect::Show(Display::Idle)));
        assert!(has(&fx, &Effect::DisarmEsc));
        // Esc while recording throws the audio away.
        m.handle(Input::HotkeyDown, t);
        let fx = m.handle(Input::Esc, t);
        assert!(has(&fx, &Effect::DiscardRecording));
        // Esc while transcribing: the late result is dropped.
        m.handle(Input::HotkeyDown, t);
        m.handle(Input::HotkeyUp, t);
        let u = m.utterance;
        m.handle(Input::Esc, t);
        let fx = m.handle(Input::Transcribed { utterance: u, text: "too late".into() }, t);
        assert!(fx.is_empty());
    }

    #[test]
    fn stale_transcription_and_other_sessions_are_ignored() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        m.handle(Input::HotkeyDown, t);
        m.handle(Input::HotkeyUp, t);
        // Pressed again before the first transcription came back.
        m.handle(Input::HotkeyDown, t);
        let fx = m.handle(Input::Transcribed { utterance: 1, text: "first".into() }, t);
        assert!(fx.is_empty());
        let fx = ask_rest(&mut m, t, "second");
        assert_eq!(fx, vec![Effect::SendPrompt { text: "second".into() }]);
        // A Stop hook from some other claude does not end our turn.
        m.handle(Input::Stop { session_id: "other".into(), last_message: Some("x".into()) }, t);
        assert!(m.turn_live());
        m.handle(Input::TranscriptPrompt { session_id: "other".into() }, t);
        let fx = m.handle(Input::TranscriptText { session_id: "other".into(), text: "x".into() }, t);
        assert!(speaks(&fx).is_empty());
    }

    #[test]
    fn empty_speech_and_missing_brain_give_a_notice() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        let fx = ask(&mut m, t, "  ");
        assert!(has(&fx, &Effect::Notice("I didn't catch that.".into())));
        assert_eq!(m.display(), Display::Idle);
        m.handle(Input::BrainUnavailable { message: "Run claude login in a terminal.".into() }, t);
        let fx = ask(&mut m, t, "hello");
        assert!(has(&fx, &Effect::Notice("Run claude login in a terminal.".into())));
        assert!(!fx.iter().any(|e| matches!(e, Effect::SendPrompt { .. })));
    }

    #[test]
    fn idle_reset_starts_a_new_session_only_when_used_and_idle() {
        let (mut m, t) = ready(SpeakMode::AtStop);
        // Unused session: never reset.
        assert!(m.handle(Input::Tick, t + Duration::from_secs(3600)).is_empty());
        ask(&mut m, t, "hello");
        m.handle(Input::Stop { session_id: S.into(), last_message: Some("Hi.".into()) }, t);
        m.handle(Input::SpeechIdle { seq: 1 }, t + Duration::from_secs(2));
        assert!(m.handle(Input::Tick, t + Duration::from_secs(300)).is_empty());
        let fx = m.handle(Input::Tick, t + Duration::from_secs(603));
        assert_eq!(fx, vec![Effect::NewSession]);
        assert!(m.session_id().is_none());
        // Until the new session is ready, speech gets a notice instead of a turn.
        let later = t + Duration::from_secs(700);
        let fx = ask(&mut m, later, "hello");
        assert!(fx.iter().any(|e| matches!(e, Effect::Notice(_))));
        m.handle(Input::SessionReady { session_id: "sess-2".into() }, later);
        let fx = ask(&mut m, later, "hello");
        assert_eq!(fx, vec![Effect::SendPrompt { text: "hello".into() }]);
        // A busy machine never resets.
        assert!(m.handle(Input::Tick, later + Duration::from_secs(3600)).is_empty());
    }

    #[test]
    fn turn_failure_is_reported_and_ends_the_turn() {
        let (mut m, t) = ready(SpeakMode::EachMessage);
        ask(&mut m, t, "delete everything");
        let fx = m.handle(Input::TurnFailed { message: "Claude asked for a permission.".into() }, t);
        assert!(has(&fx, &Effect::Notice("Claude asked for a permission.".into())));
        assert!(has(&fx, &Effect::DisarmEsc));
        assert!(!m.turn_live());
    }
}
