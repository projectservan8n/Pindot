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
        if PERMISSION_PROMPT_MARKERS.iter().any(|m| screen.contains(m)) {
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
