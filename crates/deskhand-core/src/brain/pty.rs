//! Spawn interactive `claude` in a pseudo-terminal and drive its TUI.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use uuid::Uuid;

/// How long the TUI must stay silent after its first output before we treat
/// the splash as settled. Typing earlier loses input.
pub const SPLASH_SILENCE: Duration = Duration::from_millis(1800);
/// Pause between pasting the prompt text and pressing Enter.
pub const SUBMIT_DELAY: Duration = Duration::from_millis(120);
/// The input box must be this quiet before Enter (see `type_prompt`).
pub const ECHO_QUIET: Duration = Duration::from_millis(400);
pub const ECHO_TIMEOUT: Duration = Duration::from_secs(5);

/// Env vars that identify a *parent* Claude Code session. If Deskhand itself
/// was started from inside Claude Code, these must not leak into the child.
const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SSE_PORT",
];

/// Phrases (lowercase, whitespace removed) the CLI prints when it is not
/// signed in or its login expired. Retrying can't fix these: the user must
/// run `claude` and `/login`.
const AUTH_FAILURE_MARKERS: &[&str] = &[
    "notloggedin",
    "loginexpired",
    "oauthtokenhasexpired",
    "oauthaccesstokenhasexpired",
    "failedtoauthenticate",
    "re-authenticatetocontinue",
    "apierror:401",
    "noapikeyavailable",
];

const TRUST_MARKERS: &[&str] = &["doyoutrustthefiles", "trustthisfolder", "yes,itrustthisfolder", "yes,proceed"];

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub claude_bin: PathBuf,
    pub cwd: PathBuf,
    pub session_id: Uuid,
    pub mcp_config: Option<PathBuf>,
    pub settings: Option<PathBuf>,
    pub allowed_tools: Vec<String>,
    /// `--permission-mode` (unset = Claude Code's own default).
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub append_system_prompt: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl SpawnOptions {
    pub fn new(claude_bin: PathBuf, cwd: PathBuf) -> Self {
        Self {
            claude_bin,
            cwd,
            session_id: Uuid::new_v4(),
            mcp_config: None,
            settings: None,
            allowed_tools: vec![],
            permission_mode: None,
            model: None,
            append_system_prompt: None,
            cols: 220,
            rows: 60,
        }
    }

    pub fn args(&self) -> Vec<String> {
        let mut a = vec!["--session-id".to_string(), self.session_id.to_string()];
        if let Some(p) = &self.mcp_config {
            a.push("--mcp-config".into());
            a.push(p.display().to_string());
            a.push("--strict-mcp-config".into());
        }
        if let Some(p) = &self.settings {
            a.push("--settings".into());
            a.push(p.display().to_string());
        }
        if let Some(m) = &self.permission_mode {
            a.push("--permission-mode".into());
            a.push(m.clone());
        }
        if let Some(m) = &self.model {
            a.push("--model".into());
            a.push(m.clone());
        }
        if let Some(s) = &self.append_system_prompt {
            a.push("--append-system-prompt".into());
            a.push(s.clone());
        }
        if !self.allowed_tools.is_empty() {
            // Variadic flag: keep it last so nothing positional follows it.
            a.push("--allowedTools".into());
            a.extend(self.allowed_tools.iter().cloned());
        }
        a
    }
}

#[derive(Default)]
struct OutBuf {
    bytes: Vec<u8>,
    total: usize,
    first: Option<Instant>,
    last: Option<Instant>,
    eof: bool,
}

const OUT_KEEP: usize = 512 * 1024;

pub struct ClaudeSession {
    pub session_id: Uuid,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    out: Arc<Mutex<OutBuf>>,
    reader: Option<JoinHandle<()>>,
    /// Byte offset (in `total`) after which screen checks look, so text from
    /// an already-answered dialog is not matched again.
    scan_from: usize,
}

/// Strip ANSI/VT escape sequences. Cursor-forward (`CSI n C`) becomes a
/// space, because the TUI uses it instead of literal spaces.
pub fn strip_ansi(input: &[u8]) -> String {
    let s = String::from_utf8_lossy(input);
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                let mut final_byte = None;
                for d in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&d) {
                        final_byte = Some(d);
                        break;
                    }
                }
                // Cursor-forward (C) and cursor-to-column (G) stand in for spaces.
                if matches!(final_byte, Some('C') | Some('G')) {
                    out.push(' ');
                }
            }
            // Charset designation, e.g. ESC ( B: three bytes.
            Some('(') | Some(')') | Some('*') | Some('+') => {
                it.next();
                it.next();
            }
            Some(']') => {
                it.next();
                while let Some(d) = it.next() {
                    if d == '\x07' {
                        break;
                    }
                    if d == '\x1b' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            Some(_) => {
                it.next();
            }
            None => {}
        }
    }
    out
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect()
}

impl ClaudeSession {
    pub fn spawn(opts: &SpawnOptions) -> Result<Self> {
        let args = opts.args();
        // The product rule: interactive only. Refuse print mode outright.
        if args.iter().any(|a| a == "-p" || a == "--print") {
            bail!("refusing to start claude in print mode (-p); Deskhand only drives the interactive CLI");
        }
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: opts.rows, cols: opts.cols, pixel_width: 0, pixel_height: 0 })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&opts.claude_bin);
        cmd.args(&args);
        cmd.cwd(&opts.cwd);
        for k in PARENT_SESSION_VARS {
            cmd.env_remove(k);
        }
        // Subscription login only: a stray key would switch to API billing.
        cmd.env("ANTHROPIC_API_KEY", "");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd).context("spawning claude in the pty")?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;

        let out = Arc::new(Mutex::new(OutBuf::default()));
        let out_r = out.clone();
        // Debug aid: DESKHAND_PTY_LOG=/path appends the raw TUI output there.
        let mut raw_log = std::env::var_os("DESKHAND_PTY_LOG")
            .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
        let reader_thread = thread::Builder::new().name("claude-pty-reader".into()).spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        out_r.lock().unwrap().eof = true;
                        break;
                    }
                    Ok(n) => {
                        if let Some(f) = raw_log.as_mut() {
                            let _ = f.write_all(&buf[..n]);
                        }
                        let mut o = out_r.lock().unwrap();
                        let now = Instant::now();
                        o.first.get_or_insert(now);
                        o.last = Some(now);
                        o.total += n;
                        o.bytes.extend_from_slice(&buf[..n]);
                        if o.bytes.len() > OUT_KEEP {
                            let cut = o.bytes.len() - OUT_KEEP;
                            o.bytes.drain(..cut);
                        }
                    }
                }
            }
        })?;

        Ok(Self {
            session_id: opts.session_id,
            master: pair.master,
            writer,
            child,
            out,
            reader: Some(reader_thread),
            scan_from: 0,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Text printed since `scan_from`, escapes stripped.
    pub fn screen_text(&self) -> String {
        let o = self.out.lock().unwrap();
        let start_total = o.total - o.bytes.len();
        let from = self.scan_from.saturating_sub(start_total).min(o.bytes.len());
        strip_ansi(&o.bytes[from..])
    }

    /// The last `n` characters of output, escapes stripped (for errors/logs).
    pub fn tail(&self, n: usize) -> String {
        let t = self.screen_text();
        let skip = t.chars().count().saturating_sub(n);
        t.chars().skip(skip).collect()
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for the TUI to be ready for input: first output, then
    /// `SPLASH_SILENCE` of quiet. Answers the folder-trust dialog only when
    /// `accept_trust` is set. Fails fast on a login problem.
    pub fn wait_ready(&mut self, timeout: Duration, accept_trust: bool) -> Result<()> {
        let start = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(100));
            if !self.is_running() {
                bail!("claude exited during startup. Last output:\n{}", self.tail(1500));
            }
            let text = squash(&self.screen_text());
            if AUTH_FAILURE_MARKERS.iter().any(|m| text.contains(m)) {
                bail!("claude is not signed in. Run `claude` in a terminal and use /login. Output:\n{}", self.tail(800));
            }
            if TRUST_MARKERS.iter().any(|m| text.contains(m)) {
                if !accept_trust {
                    bail!("claude is asking whether to trust the working folder; refusing to answer without accept_trust");
                }
                // Let the dialog finish drawing, then select "Yes" explicitly:
                // in 2.1.3xx the default (first) option is "No, exit".
                thread::sleep(Duration::from_millis(600));
                self.choose_trust_yes()?;
                tracing::info!("answered the folder-trust dialog: Yes, I trust this folder");
                continue;
            }
            let (first, last) = {
                let o = self.out.lock().unwrap();
                (o.first, o.last)
            };
            if first.is_some() && last.map(|l| l.elapsed() >= SPLASH_SILENCE).unwrap_or(false) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("claude did not become ready in {timeout:?}. Last output:\n{}", self.tail(1500));
            }
        }
    }

    /// Move the dialog's `❯` marker to the "Yes" option and press Enter.
    /// Never presses Enter unless the marked option starts with "yes", so a
    /// reordered dialog can't make us pick "No, exit" (or anything else).
    fn choose_trust_yes(&mut self) -> Result<()> {
        for _ in 0..4 {
            let text = squash(&self.screen_text());
            let selected = text.rfind('❯').map(|i| text[i + '❯'.len_utf8()..].to_string());
            match selected {
                Some(s) if s.starts_with("yes") => {
                    self.scan_from = self.out.lock().unwrap().total;
                    return self.write_raw(b"\r");
                }
                Some(_) => {
                    self.scan_from = self.out.lock().unwrap().total;
                    self.write_raw(b"\x1b[B")?;
                    thread::sleep(Duration::from_millis(400));
                }
                None => break,
            }
        }
        bail!("could not select \"Yes, I trust this folder\" in the trust dialog. Screen:\n{}", self.tail(800))
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Type a one-line prompt and press Enter.
    ///
    /// Claude Code takes a burst of input as a paste, and an Enter that
    /// arrives while it is still taking the paste in is lost (a ~550-char
    /// prompt sat unsent for 5 minutes this way). So wait until the end of
    /// the prompt is echoed (or a paste placeholder shows) and the screen
    /// has been quiet for `ECHO_QUIET`, at most `ECHO_TIMEOUT`, then press
    /// Enter. `drive_turn` checks the turn really started.
    pub fn type_prompt(&mut self, prompt: &str) -> Result<()> {
        let one_line: String = prompt.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        self.scan_from = self.out.lock().unwrap().total;
        self.write_raw(one_line.as_bytes())?;
        thread::sleep(SUBMIT_DELAY);
        let tail: String = squash(&one_line).chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
        let start = Instant::now();
        while start.elapsed() < ECHO_TIMEOUT {
            let screen = squash(&self.screen_text());
            let echoed = screen.contains(&tail) || screen.contains("pastedtext");
            let quiet = self.out.lock().unwrap().last.map(|l| l.elapsed() >= ECHO_QUIET).unwrap_or(true);
            if echoed && quiet {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.press_enter()
    }

    /// Press Enter (submits what is in the input box).
    pub fn press_enter(&mut self) -> Result<()> {
        self.write_raw(b"\r")
    }

    /// Press Esc (interrupts the running turn).
    pub fn interrupt(&mut self) -> Result<()> {
        self.write_raw(b"\x1b")
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| anyhow!("{e}"))
    }

    /// Ctrl+C (clears anything left in the input box, or stops a running
    /// turn), then `/exit`, then kill if it hasn't gone within `grace`.
    /// Without the Ctrl+C, `/exit` was once appended to an unsent prompt and
    /// Enter submitted the lot as a new turn.
    pub fn exit(mut self, grace: Duration) -> Result<()> {
        let _ = self.write_raw(b"\x03");
        thread::sleep(Duration::from_millis(400));
        let _ = self.write_raw(b"/exit");
        thread::sleep(SUBMIT_DELAY);
        let _ = self.write_raw(b"\r");
        let start = Instant::now();
        while start.elapsed() < grace {
            if !self.is_running() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        tracing::warn!("claude did not exit after /exit; killing it");
        self.child.kill()?;
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // The reader thread ends when the pty closes; don't block on it.
        drop(self.reader.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_osc_and_turns_cursor_forward_into_space() {
        let s = strip_ansi(b"\x1b[1mNot\x1b[1Clogged\x1b[0m in\x1b]0;title\x07!\x1b(B");
        assert_eq!(s, "Not logged in!");
        assert!(squash(&s).contains("notloggedin"));
        // 2.1.3xx positions words with CSI n G (cursor to column).
        let s = strip_ansi("\x1b[4GNo,\x1b[8Gexit\r\n\x1b[2G\x1b[38;2;1;2;3m❯\x1b[4GYes,\x1b[9GI".as_bytes());
        assert_eq!(squash(&s), "no,exit❯yes,i");
    }

    #[test]
    fn args_never_print_mode_and_allowed_tools_last() {
        let mut o = SpawnOptions::new("/bin/claude".into(), "/tmp".into());
        o.mcp_config = Some("/x/mcp.json".into());
        o.settings = Some("/x/s.json".into());
        o.allowed_tools = vec!["mcp__desktop".into()];
        let a = o.args();
        assert!(!a.iter().any(|x| x == "-p" || x == "--print"));
        assert_eq!(a.last().unwrap(), "mcp__desktop");
        assert!(a.contains(&"--strict-mcp-config".to_string()));
    }
}
