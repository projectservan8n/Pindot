//! Drive one turn of an interactive `claude` session: type the prompt, then
//! wait for this session's `Stop` hook while reporting tool activity. A real
//! Esc (the emergency stop) interrupts the turn by pressing Esc in the TUI.

use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::broadcast::error::TryRecvError;

use super::ClaudeSession;
use crate::server::activity::ActivityEntry;
use crate::server::control::ControlEvent;
use crate::server::{ServerState, StopHookEvent};

/// Squashed lowercase phrases of a tool-permission prompt. We pass
/// `--allowedTools`, so seeing one means the allow rule did not match.
pub const PERMISSION_PROMPT_MARKERS: &[&str] = &["doyouwanttoproceed", "doyouwanttoallow"];

/// Is Claude Code asking for a tool permission? `screen` is the screen's
/// text in lower case without whitespace.
///
/// The question differs by tool ("Do you want to create X?" for a new
/// file, "Do you want to make this edit to X?"), so besides the two known
/// phrases any "Do you want to ..." counts when the dialog's own footer
/// ("Esc to cancel") is there too. The footer keeps the assistant's own
/// questions out: while it works the TUI says "esc to interrupt".
pub fn is_permission_prompt(screen: &str) -> bool {
    PERMISSION_PROMPT_MARKERS.iter().any(|m| screen.contains(m))
        || (screen.contains("doyouwantto") && screen.contains("esctocancel"))
}
/// How long after Enter the `UserPromptSubmit` hook must have arrived.
pub const SUBMIT_CHECK: Duration = Duration::from_secs(6);

#[derive(Debug)]
pub enum TurnEnd {
    /// The Stop hook for this session arrived.
    Stopped(StopHookEvent),
    /// The user pressed Esc; the turn was interrupted.
    Interrupted,
    /// Claude Code asked for a tool permission (screen tail attached).
    PermissionPrompt(String),
    /// `claude` exited (last output attached).
    Exited(String),
    /// No Stop hook in time (last output attached).
    Timeout(String),
}

/// Type `prompt` and wait for the end of the turn. `on_activity` sees every
/// activity-log entry (tool calls, approvals, stops) as it happens.
pub fn drive_turn(
    session: &mut ClaudeSession,
    state: &ServerState,
    prompt: &str,
    timeout: Duration,
    on_activity: &mut dyn FnMut(&ActivityEntry),
) -> Result<TurnEnd> {
    let mut stop_rx = state.stop_events.subscribe();
    let mut act_rx = state.activity.subscribe();
    let mut ctl_rx = state.control.subscribe();
    let mut prompt_rx = state.prompt_events.subscribe();
    session.type_prompt(prompt)?;
    let t0 = Instant::now();
    let want = session.session_id.to_string();
    // The UserPromptSubmit hook proves the turn started. If it hasn't come
    // after `SUBMIT_CHECK`, Enter was lost: press it once more.
    let mut started = false;
    let mut retried = false;
    let mut drain = |rx: &mut tokio::sync::broadcast::Receiver<ActivityEntry>| loop {
        match rx.try_recv() {
            Ok(e) => on_activity(&e),
            Err(TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    };
    loop {
        drain(&mut act_rx);
        while let Ok(id) = prompt_rx.try_recv() {
            started |= id == want;
        }
        if !started && !retried && t0.elapsed() > SUBMIT_CHECK {
            tracing::warn!("no UserPromptSubmit hook {SUBMIT_CHECK:?} after Enter; pressing Enter again");
            session.press_enter()?;
            retried = true;
        }
        match stop_rx.try_recv() {
            Ok(ev) if ev.session_id == want => {
                drain(&mut act_rx);
                return Ok(TurnEnd::Stopped(ev));
            }
            Ok(ev) => tracing::info!("Stop hook for another session {} ignored", ev.session_id),
            Err(_) => {}
        }
        while let Ok(ev) = ctl_rx.try_recv() {
            if ev == ControlEvent::EmergencyStop {
                // The voice loop would also stop speaking here.
                session.interrupt()?;
                std::thread::sleep(Duration::from_millis(500));
                drain(&mut act_rx);
                return Ok(TurnEnd::Interrupted);
            }
        }
        let screen: String =
            session.screen_text().chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
        if is_permission_prompt(&screen) {
            return Ok(TurnEnd::PermissionPrompt(session.tail(1500)));
        }
        if !session.is_running() {
            return Ok(TurnEnd::Exited(session.tail(1500)));
        }
        if t0.elapsed() > timeout {
            return Ok(TurnEnd::Timeout(session.tail(1500)));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn squashed(screen: &str) -> String {
        screen.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
    }

    // The dialogs as claude 2.1.325 printed them (captured on 16 Nov 2026,
    // box-drawing characters left out). The TUI redraws only what changed,
    // so the option lines can arrive broken, as in EDIT.
    const READ_OUTSIDE: &str = "Read file
 Search(pattern: kumquat, path: outside)
 Do you want to proceed?
 > 1. Yes
   2. Yes, allow reading from outside during this session
   3. No
 Esc to cancel";
    const NEW_FILE: &str = " Do you want to create outside.txt? 1. Yes2. Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session (shift+tab)3. NoEsc to cancel - Tab to amend";
    const EDIT: &str = " Do you want to make this edit to CLAUDE.md? 1 Yes2 Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session (shift+tab)3. No
Esc to cancel";

    #[test]
    fn the_dialog_for_a_tool_is_a_permission_prompt() {
        assert!(is_permission_prompt(&squashed(READ_OUTSIDE)));
        assert!(is_permission_prompt(&squashed("Do you want to allow Claude to fetch this content?")));
    }

    /// A new file asks "Do you want to create X?" and an edit "Do you want
    /// to make this edit to X?". Neither was known, so a write the rules do
    /// not allow left the turn at the dialog until the 15 minute limit.
    #[test]
    fn the_dialogs_for_a_new_file_and_for_an_edit_are_permission_prompts() {
        assert!(is_permission_prompt(&squashed(NEW_FILE)), "a new file");
        assert!(is_permission_prompt(&squashed(EDIT)), "an edit");
    }

    /// Only a dialog counts: the assistant asking something in its own
    /// words must not get the turn stopped.
    #[test]
    fn the_assistants_own_question_is_not_a_permission_prompt() {
        assert!(!is_permission_prompt(&squashed("I found three copies. Do you want to create a folder for them?")));
        assert!(!is_permission_prompt(&squashed("Do you want to make this edit to the title as well?")));
        assert!(!is_permission_prompt(&squashed("Thinking... (esc to interrupt)")));
    }
}
