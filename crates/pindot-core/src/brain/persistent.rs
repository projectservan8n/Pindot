//! The persistent brain for the voice loop: ONE interactive `claude` kept
//! running in a pseudo-terminal across turns, so a spoken request doesn't pay
//! Claude Code's start-up time.
//!
//! A worker thread owns the `ClaudeSession` and takes commands (send a
//! prompt, interrupt, new session, shut down). It reports to the voice
//! machine with `machine::Input`s: session ready or unavailable, the Stop
//! hook, failed turns, and (through the transcript tailer) prompts, assistant
//! text and tool calls as they land.
//!
//! - Start-up checks that `claude` is installed and signed in
//!   (`claude auth status`); if not, it says to run `claude login`. Pindot
//!   never handles the login itself.
//! - Interrupt: Esc in the TUI; if Claude Code doesn't confirm within
//!   `INTERRUPT_CONFIRM`, kill it and start it again with `--resume` (same
//!   session id, so the conversation survives).
//! - A tool-permission prompt can't be answered by voice: press Esc and
//!   report the turn as failed.
//! - If `claude` exits, it is resumed; after repeated failures the brain
//!   reports itself unavailable and retries with a growing delay.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use uuid::Uuid;

use super::config::{ALLOWED_TOOLS_RULE, PermissionMode, write_brain_hook_settings_for, write_brain_mcp_config};
use super::pty::{ClaudeSession, SpawnOptions};
use super::tailer::{TranscriptEvent, TranscriptTailer};
use super::turn::{PERMISSION_PROMPT_MARKERS, SUBMIT_CHECK};
use super::{AuthState, SIGN_IN_HINT, auth_status, find_claude};
use crate::server::ServerState;
use crate::voice::machine::Input;

/// How long Claude Code gets to show "Interrupted" after Esc.
pub const INTERRUPT_CONFIRM: Duration = Duration::from_secs(3);
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// A turn with no Stop hook after this long is stopped and reported.
pub const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RETRY_FIRST: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(300);

pub const NOT_INSTALLED: &str =
    "Claude Code is not installed. Install it from claude.com/claude-code, run `claude login`, then try again.";

#[derive(Debug, Clone)]
pub struct BrainConfig {
    /// `None`: find it (`PINDOT_CLAUDE_BIN`, PATH, usual places).
    pub claude_bin: Option<PathBuf>,
    /// Pindot's own empty working folder (see `session_dir`).
    pub cwd: PathBuf,
    pub mcp_url: String,
    pub stop_hook_url: String,
    pub token: String,
    pub model: Option<String>,
    /// `--effort` (`None` = Claude Code's default).
    pub effort: Option<String>,
    /// Ask (only our tools) or auto mode with a few built-in tools.
    pub permission_mode: PermissionMode,
    pub system_prompt: String,
    /// The user's brain folder, when one is set and exists (`None` = the
    /// isolated session in `cwd`). Checked again at every start, so a folder
    /// that vanished later falls back to `cwd` instead of failing.
    pub brain_folder: Option<PathBuf>,
}

/// `<data dir>/session`, user-only: the working folder of the voice brain.
/// Empty and Pindot's own, so trusting it is safe and no project
/// settings or CLAUDE.md from elsewhere apply.
pub fn session_dir() -> Result<PathBuf> {
    let dir = crate::paths::data_dir()?.join("session");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

#[derive(Debug)]
enum Command {
    Send(String),
    Interrupt,
    NewSession,
    Shutdown,
}

pub type InputSink = Arc<dyn Fn(Input) + Send + Sync>;

/// Handle to the brain worker. Dropping it exits `claude`.
pub struct PersistentBrain {
    tx: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl PersistentBrain {
    pub fn start(config: BrainConfig, state: Arc<ServerState>, emit: InputSink) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pindot-brain".into())
            .spawn(move || Worker::new(config, state, emit).run(rx))?;
        Ok(Self { tx, thread: Some(thread) })
    }

    pub fn send(&self, text: &str) {
        let _ = self.tx.send(Command::Send(text.to_string()));
    }

    pub fn interrupt(&self) {
        let _ = self.tx.send(Command::Interrupt);
    }

    pub fn new_session(&self) {
        let _ = self.tx.send(Command::NewSession);
    }
}

impl Drop for PersistentBrain {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct LiveTurn {
    typed_at: Instant,
    started: bool,
    retried: bool,
}

struct Worker {
    config: BrainConfig,
    state: Arc<ServerState>,
    emit: InputSink,
    tailer: Option<TranscriptTailer>,
    session: Option<ClaudeSession>,
    session_id: Uuid,
    /// The session has had at least one prompt, so `--resume` can find it.
    has_turns: bool,
    turn: Option<LiveTurn>,
    failures: u32,
    retry_at: Option<Instant>,
    started_at: Option<Instant>,
    /// Exits soon after a start, in a row.
    crashes: u32,
}

impl Worker {
    fn new(config: BrainConfig, state: Arc<ServerState>, emit: InputSink) -> Self {
        let e = emit.clone();
        let tailer = TranscriptTailer::start(move |sid, ev| {
            let session_id = sid.to_string();
            e(match ev {
                TranscriptEvent::Prompt(_) => Input::TranscriptPrompt { session_id },
                TranscriptEvent::Text(text) => Input::TranscriptText { session_id, text },
                TranscriptEvent::ToolUse(name) => Input::TranscriptToolUse { session_id, name },
                TranscriptEvent::ToolResult => Input::TranscriptToolResult { session_id },
                TranscriptEvent::Interrupted => return,
            })
        })
        .map_err(|e| tracing::error!("transcript tailer did not start: {e}"))
        .ok();
        Self {
            config,
            state,
            emit,
            tailer,
            session: None,
            session_id: Uuid::new_v4(),
            has_turns: false,
            turn: None,
            failures: 0,
            retry_at: Some(Instant::now()),
            started_at: None,
            crashes: 0,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Command>) {
        let mut stop_rx = self.state.stop_events.subscribe();
        let mut prompt_rx = self.state.prompt_events.subscribe();
        loop {
            if self.session.is_none() && self.retry_at.is_some_and(|t| Instant::now() >= t) {
                self.start(self.has_turns);
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::Send(text)) => self.send(&text),
                Ok(Command::Interrupt) => self.interrupt(),
                Ok(Command::NewSession) => {
                    self.close();
                    self.session_id = Uuid::new_v4();
                    self.has_turns = false;
                    self.start(false);
                }
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let me = self.session_id.to_string();
            while let Ok(id) = prompt_rx.try_recv() {
                if id == me {
                    self.has_turns = true;
                    if let Some(t) = self.turn.as_mut() {
                        t.started = true;
                    }
                }
            }
            loop {
                match stop_rx.try_recv() {
                    Ok(ev) if ev.session_id == me => {
                        // Only the live turn's Stop counts (Claude Code sends
                        // none for an interrupted turn, but be sure).
                        if let Some(t) = self.turn.take() {
                            tracing::info!(
                                "voice brain: Stop hook {} ms after the prompt was typed",
                                t.typed_at.elapsed().as_millis()
                            );
                            (self.emit)(Input::Stop { session_id: me.clone(), last_message: ev.last_assistant_message });
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            self.watch();
        }
        self.close();
    }

    /// Check the live turn: lost Enter, permission prompt, exit, timeout.
    fn watch(&mut self) {
        let Some(session) = self.session.as_mut() else { return };
        if !session.is_running() {
            let tail = session.tail(600);
            tracing::warn!("claude exited unexpectedly. Last output:\n{tail}");
            self.session = None;
            self.state.end_session(&self.session_id.to_string());
            if self.turn.take().is_some() {
                (self.emit)(Input::TurnFailed { message: "Claude Code stopped unexpectedly.".into() });
            }
            let quick = self.started_at.is_some_and(|t| t.elapsed() < Duration::from_secs(30));
            self.crashes = if quick { self.crashes + 1 } else { 0 };
            if self.crashes >= 3 {
                self.crashes = 0;
                self.retry_at = Some(Instant::now() + RETRY_MAX);
                (self.emit)(Input::BrainUnavailable {
                    message: "Claude Code keeps stopping. Check that `claude` runs in a terminal.".into(),
                });
                return;
            }
            self.start(self.has_turns);
            return;
        }
        let Some(turn) = self.turn.as_mut() else { return };
        if !turn.started && !turn.retried && turn.typed_at.elapsed() > SUBMIT_CHECK {
            tracing::warn!("no UserPromptSubmit hook {SUBMIT_CHECK:?} after Enter; pressing Enter again");
            let _ = session.press_enter();
            turn.retried = true;
        }
        let screen: String =
            session.screen_text().chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
        if PERMISSION_PROMPT_MARKERS.iter().any(|m| screen.contains(m)) {
            tracing::warn!("claude asked for a tool permission; pressing Esc. Screen:\n{}", session.tail(800));
            let _ = session.interrupt();
            session.mark_seen();
            self.turn = None;
            (self.emit)(Input::TurnFailed {
                message: "Claude wanted a permission I can't grant by voice, so I stopped that request.".into(),
            });
            return;
        }
        if turn.typed_at.elapsed() > TURN_TIMEOUT {
            tracing::warn!("no Stop hook after {TURN_TIMEOUT:?}; interrupting the turn");
            self.interrupt();
            (self.emit)(Input::TurnFailed { message: "That took too long, so I stopped it.".into() });
        }
    }

    fn send(&mut self, text: &str) {
        let Some(session) = self.session.as_mut() else {
            (self.emit)(Input::TurnFailed { message: "Claude Code is not running yet. Try again in a moment.".into() });
            return;
        };
        tracing::info!("voice brain: typing the prompt ({} chars)", text.chars().count());
        let typed = session.type_prompt(text);
        match typed {
            Ok(()) => self.turn = Some(LiveTurn { typed_at: Instant::now(), started: false, retried: false }),
            Err(e) => (self.emit)(Input::TurnFailed { message: format!("Could not type into Claude Code: {e}") }),
        }
    }

    /// Stop the running turn (nothing to do if none is running: a stray Esc
    /// in an idle TUI, twice, would open its rewind menu).
    fn interrupt(&mut self) {
        let Some(turn) = self.turn.take() else { return };
        let t0 = Instant::now();
        if turn.started && let Some(session) = self.session.as_mut() {
            session.mark_seen();
            if session.interrupt().is_ok() {
                while t0.elapsed() < INTERRUPT_CONFIRM {
                    std::thread::sleep(Duration::from_millis(50));
                    let screen = session.screen_text().to_lowercase();
                    if screen.contains("interrupted") {
                        tracing::info!("voice brain: turn interrupted with Esc in {} ms", t0.elapsed().as_millis());
                        session.mark_seen();
                        return;
                    }
                }
            }
            tracing::warn!("Esc did not confirm an interrupt in {INTERRUPT_CONFIRM:?}; restarting claude with --resume");
        } else {
            // The prompt may still sit unsent in the input box: start over.
            tracing::warn!("interrupt before the turn started; restarting claude");
        }
        self.close();
        self.start(self.has_turns);
        tracing::info!("voice brain: interrupt by restart took {} ms", t0.elapsed().as_millis());
    }

    /// Start `claude` (resuming the current session id if `resume`).
    fn start(&mut self, resume: bool) {
        match self.try_start(resume) {
            Ok(()) => {
                self.failures = 0;
                self.retry_at = None;
                self.started_at = Some(Instant::now());
                let id = self.session_id.to_string();
                if let Some(t) = &self.tailer {
                    t.follow(Some(&id));
                }
                (self.emit)(Input::SessionReady { session_id: id });
            }
            Err(Unavailable(message)) => {
                self.failures += 1;
                let delay = (RETRY_FIRST * 2u32.saturating_pow(self.failures.saturating_sub(1))).min(RETRY_MAX);
                tracing::warn!("voice brain unavailable (retry in {delay:?}): {message}");
                self.retry_at = Some(Instant::now() + delay);
                // A failed resume falls back to a fresh session next time.
                self.session_id = Uuid::new_v4();
                self.has_turns = false;
                (self.emit)(Input::BrainUnavailable { message });
            }
        }
    }

    fn try_start(&mut self, resume: bool) -> Result<(), Unavailable> {
        let bin = match &self.config.claude_bin {
            Some(b) => b.clone(),
            None => find_claude().ok_or_else(|| Unavailable(NOT_INSTALLED.into()))?,
        };
        match auth_status(&bin) {
            AuthState::SignedIn { method } => tracing::info!("claude is signed in ({method})"),
            AuthState::SignedOut => return Err(Unavailable(SIGN_IN_HINT.into())),
            // Older CLIs: carry on; start-up still catches a login problem.
            AuthState::Unknown(e) => tracing::warn!("could not check the claude login: {e}"),
        }
        let opts = self.spawn_options(bin, resume).map_err(|e| Unavailable(format!("Pindot could not set up Claude Code: {e:#}")))?;
        tracing::info!(
            "voice brain: starting claude ({} session {})",
            if resume { "resuming" } else { "new" },
            self.session_id
        );
        let t0 = Instant::now();
        let mut session = ClaudeSession::spawn(&opts).map_err(|e| Unavailable(format!("Claude Code could not start: {e:#}")))?;
        // Our own empty folder: answering its trust question is safe.
        if let Err(e) = session.wait_ready(READY_TIMEOUT, true) {
            let msg = format!("{e:#}");
            tracing::warn!("claude did not start: {msg}");
            return Err(Unavailable(if msg.contains("not signed in") {
                SIGN_IN_HINT.into()
            } else {
                "Claude Code did not start. Check that `claude` runs in a terminal.".into()
            }));
        }
        tracing::info!("voice brain: claude ready in {} ms (pid {:?})", t0.elapsed().as_millis(), session.pid());
        session.mark_seen();
        self.session = Some(session);
        Ok(())
    }

    fn spawn_options(&self, bin: PathBuf, resume: bool) -> Result<SpawnOptions> {
        let mut opts = SpawnOptions::new(bin, self.config.cwd.clone());
        let folder = self.config.brain_folder.as_deref().filter(|f| f.is_dir());
        opts.session_id = self.session_id;
        opts.resume = resume;
        let key = self.session_id.to_string();
        let mode = self.config.permission_mode;
        opts.settings = Some(
            write_brain_hook_settings_for(&self.config.stop_hook_url, &self.config.token, mode, folder)
                .context("hook settings")?,
        );
        opts.mcp_config =
            Some(write_brain_mcp_config(&self.config.mcp_url, &self.config.token, &key).context("MCP config")?);
        opts.allowed_tools = vec![ALLOWED_TOOLS_RULE.into()];
        opts.permission_mode = Some(mode.cli().into());
        opts.tools = mode.builtin_tools().iter().map(|t| t.to_string()).collect();
        opts.model = self.config.model.clone();
        opts.effort = self.config.effort.clone();
        opts.append_system_prompt = Some(self.config.system_prompt.clone());
        if let Some(f) = folder {
            super::folder::apply(&mut opts, f);
        }
        Ok(opts)
    }

    fn close(&mut self) {
        self.turn = None;
        if let Some(session) = self.session.take() {
            let pid = session.pid();
            if let Err(e) = session.exit(Duration::from_secs(5)) {
                tracing::warn!("claude exit: {e:#}");
            }
            tracing::info!("voice brain: claude (pid {pid:?}) closed");
            self.state.end_session(&self.session_id.to_string());
        }
    }
}

struct Unavailable(String);
