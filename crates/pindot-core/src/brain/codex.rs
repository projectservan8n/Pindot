//! The ChatGPT plan as a brain: OpenAI's Codex CLI, signed in by the user
//! with `codex login`, run once per turn as `codex exec --json` (then
//! `codex exec resume <thread>` for the turns after). Pindot never handles
//! the sign-in and never passes an API key: `forced_login_method` is
//! `chatgpt` and `OPENAI_API_KEY` is taken out of the child's environment.
//!
//! Like the other brains it gets Pindot's desktop tools only: Codex's own
//! shell, browser and app tools are switched off, its sandbox is read-only,
//! the user's own Codex settings are not loaded (`--ignore-user-config`;
//! the sign-in still comes from `~/.codex`), and the desktop MCP server is
//! reached with the bearer token and an `X-Pindot-Session` header, so
//! approvals, Esc and the activity log apply unchanged. Each turn's events
//! (seen with Codex 0.169: `thread.started`, `item.started`/`completed` for
//! `mcp_tool_call`, `agent_message` and `reasoning`, `turn.completed` or
//! `turn.failed`) become the voice machine's inputs and are written to
//! Pindot's own conversation file, which Chat reads.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::Brain;
use super::native::log::{ConversationLog, TOOL_PREFIX};
use super::native::openai::ToolCall;
use super::persistent::{InputSink, TURN_TIMEOUT};
use crate::server::ServerState;
use crate::voice::machine::Input;

const RETRY_FIRST: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(300);
/// Codex's own tools Pindot switches off (feature names of Codex 0.169).
pub const DISABLED_FEATURES: &[&str] =
    &["shell_tool", "unified_exec", "browser_use", "browser_use_external", "in_app_browser", "apps", "view_image"];
pub const NOT_INSTALLED: &str =
    "Codex isn't installed. Install it with `npm install -g @openai/codex`, then run `codex login` in a terminal.";
pub const SIGNED_OUT: &str = "Codex is not signed in. Run `codex login` in a terminal and sign in with your ChatGPT account.";

#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// `None`: find it (`PINDOT_CODEX_BIN`, the npm package, PATH).
    pub bin: Option<PathBuf>,
    /// `None`: Codex's default model for the plan.
    pub model: Option<String>,
    pub mcp_url: String,
    pub token: String,
    pub system_prompt: String,
    /// Pindot's own empty session folder.
    pub cwd: PathBuf,
    /// Conversations are written under it (`conversations/`).
    pub data_dir: PathBuf,
    /// More `-c key=value` settings (tests: a local model provider).
    pub extra_config: Vec<String>,
    /// Check `codex login status` before starting (off for a local provider).
    pub require_sign_in: bool,
}

/// A TOML basic string, for `-c key=value`.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The command line of one turn: a new thread, or `resume` of `thread`.
/// The prompt goes on stdin (`-`).
pub fn exec_args(cfg: &CodexConfig, session: &str, thread: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec!["exec".into()];
    if thread.is_some() {
        a.push("resume".into());
    }
    a.extend(["--json", "--skip-git-repo-check", "--ignore-user-config"].map(String::from));
    let mut config = vec![
        "sandbox_mode=\"read-only\"".to_string(),
        "project_root_markers=[]".into(),
        "check_for_update_on_startup=false".into(),
        "history.persistence=\"none\"".into(),
        "forced_login_method=\"chatgpt\"".into(),
        format!("mcp_servers.desktop.url={}", toml_string(&cfg.mcp_url)),
        "mcp_servers.desktop.bearer_token_env_var=\"PINDOT_MCP_TOKEN\"".into(),
        format!("mcp_servers.desktop.http_headers={{\"X-Pindot-Session\"={}}}", toml_string(session)),
        "mcp_servers.desktop.default_tools_approval_mode=\"approve\"".into(),
        format!("developer_instructions={}", toml_string(&cfg.system_prompt)),
    ];
    config.extend(cfg.extra_config.iter().cloned());
    for c in config {
        a.push("-c".into());
        a.push(c);
    }
    for f in DISABLED_FEATURES {
        a.push("--disable".into());
        a.push((*f).into());
    }
    if let Some(m) = cfg.model.as_deref().filter(|m| !m.trim().is_empty()) {
        a.push("-m".into());
        a.push(m.trim().into());
    }
    if let Some(t) = thread {
        a.push(t.into());
    }
    a.push("-".into());
    a
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexEvent {
    Thread(String),
    ToolStart { id: String, name: String, arguments: Value },
    ToolDone { id: String, text: String, is_error: bool },
    Message(String),
    Reasoning(String),
    /// Something Codex reported that does not end the turn.
    Warning(String),
    TurnDone,
    TurnFailed(String),
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// One line of `codex exec --json`.
pub fn parse_event(line: &str) -> Option<CodexEvent> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let item = v.get("item");
    let kind = item.map(|i| str_at(i, "/type")).unwrap_or("");
    let tool_name = |i: &Value| {
        let (server, tool) = (str_at(i, "/server"), str_at(i, "/tool"));
        if server.is_empty() || server == "desktop" { tool.to_string() } else { format!("{server}.{tool}") }
    };
    match v.get("type").and_then(Value::as_str)? {
        "thread.started" => Some(CodexEvent::Thread(str_at(&v, "/thread_id").to_string())),
        "item.started" if kind == "mcp_tool_call" => {
            let i = item?;
            Some(CodexEvent::ToolStart {
                id: str_at(i, "/id").into(),
                name: tool_name(i),
                arguments: i.get("arguments").cloned().unwrap_or(Value::Null),
            })
        }
        "item.completed" => {
            let i = item?;
            match kind {
                "mcp_tool_call" => {
                    let err = i.pointer("/error/message").and_then(Value::as_str);
                    let text = match err {
                        Some(e) => e.to_string(),
                        None => i
                            .pointer("/result/content")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|c| c.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    Some(CodexEvent::ToolDone { id: str_at(i, "/id").into(), text, is_error: err.is_some() || str_at(i, "/status") == "failed" })
                }
                "agent_message" => Some(CodexEvent::Message(str_at(i, "/text").to_string())),
                "reasoning" => Some(CodexEvent::Reasoning(str_at(i, "/text").to_string())),
                "error" => Some(CodexEvent::Warning(str_at(i, "/message").to_string())),
                _ => None,
            }
        }
        "turn.completed" => Some(CodexEvent::TurnDone),
        "turn.failed" => Some(CodexEvent::TurnFailed(str_at(&v, "/error/message").to_string())),
        "error" => Some(CodexEvent::Warning(str_at(&v, "/message").to_string())),
        _ => None,
    }
}

/// The native binary inside Codex's npm package (the `codex` command npm
/// installs is a Node script, and an app started from the Finder may not
/// find `node`).
pub fn native_in_package(package_root: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let scoped = package_root.join("node_modules").join("@openai");
    for platform in std::fs::read_dir(scoped).ok()?.flatten() {
        let vendor = platform.path().join("vendor");
        for triple in std::fs::read_dir(vendor).into_iter().flatten().flatten() {
            let bin = triple.path().join("bin").join(name);
            if bin.is_file() {
                return Some(bin);
            }
        }
    }
    None
}

fn is_script(p: &Path) -> bool {
    let mut head = [0u8; 2];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"#!"
}

/// `codex`: `PINDOT_CODEX_BIN`, the native binary of the npm package
/// (found from the `codex` on PATH or the usual npm folders), else a
/// `codex` on PATH or in Homebrew's folder that is not a script.
pub fn find_codex() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PINDOT_CODEX_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("codex")).collect()).unwrap_or_default();
    let home = dirs::home_dir().unwrap_or_default();
    candidates.extend([home.join(".npm-global/bin/codex"), "/opt/homebrew/bin/codex".into(), "/usr/local/bin/codex".into()]);
    for c in &candidates {
        // npm's `bin/codex` links to `<package>/bin/codex.js`.
        if let Ok(real) = std::fs::canonicalize(c)
            && real.file_name().is_some_and(|n| n == "codex.js")
            && let Some(bin) = real.parent().and_then(Path::parent).and_then(native_in_package)
        {
            return Some(bin);
        }
    }
    let roots = [
        home.join(".npm-global/lib/node_modules/@openai/codex"),
        "/opt/homebrew/lib/node_modules/@openai/codex".into(),
        "/usr/local/lib/node_modules/@openai/codex".into(),
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("npm/node_modules/@openai/codex"),
    ];
    if let Some(bin) = roots.iter().find_map(|r| native_in_package(r)) {
        return Some(bin);
    }
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(name)).collect::<Vec<_>>()).unwrap_or_default();
    on_path.into_iter().chain([PathBuf::from("/opt/homebrew/bin").join(name)]).find(|p| p.is_file() && !is_script(p))
}

/// `codex login status`: exit 0 when signed in.
pub fn signed_in(bin: &Path) -> Result<bool, String> {
    let mut cmd = Command::new(bin);
    cmd.args(["login", "status"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    super::hidden(&mut cmd);
    cmd.status().map(|s| s.success()).map_err(|e| format!("Codex could not be started: {e}"))
}

enum Command_ {
    Send(String),
    NewSession,
    Shutdown,
}

/// Handle to the Codex worker. Dropping it stops any turn and ends the
/// conversation.
pub struct CodexBrain {
    tx: mpsc::Sender<Command_>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CodexBrain {
    pub fn start(config: CodexConfig, state: Arc<ServerState>, emit: InputSink) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("pindot-codex-brain".into())
            .spawn(move || Worker { config, state, emit, cancel: c, bin: None, session: None, problem: None }.run(rx))?;
        Ok(Self { tx, cancel, thread: Some(thread) })
    }
}

impl Brain for CodexBrain {
    fn send(&self, text: &str) {
        let _ = self.tx.send(Command_::Send(text.to_string()));
    }
    fn interrupt(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    fn new_session(&self) {
        let _ = self.tx.send(Command_::NewSession);
    }
}

impl Drop for CodexBrain {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Command_::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Session {
    id: String,
    /// Codex's thread, once the first turn started one.
    thread: Option<String>,
    log: Option<ConversationLog>,
}

struct Worker {
    config: CodexConfig,
    state: Arc<ServerState>,
    emit: InputSink,
    cancel: Arc<AtomicBool>,
    bin: Option<PathBuf>,
    session: Option<Session>,
    problem: Option<String>,
}

/// How one turn ended.
enum Ended {
    Done(Option<String>),
    Failed(String),
    Interrupted,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Command_>) {
        let mut retry = RETRY_FIRST;
        let mut next_try = Instant::now();
        loop {
            if self.session.is_none() && Instant::now() >= next_try {
                match self.open_session() {
                    Ok(()) => retry = RETRY_FIRST,
                    Err(msg) => {
                        if self.problem.as_deref() != Some(msg.as_str()) {
                            println!("codex brain: {msg}");
                            (self.emit)(Input::BrainUnavailable { message: msg.clone() });
                        }
                        self.problem = Some(msg);
                        next_try = Instant::now() + retry;
                        retry = (retry * 2).min(RETRY_MAX);
                    }
                }
            }
            let wait = if self.session.is_none() {
                next_try.saturating_duration_since(Instant::now()).max(Duration::from_millis(100))
            } else {
                Duration::from_secs(3600)
            };
            match rx.recv_timeout(wait) {
                Ok(Command_::Send(text)) => self.turn(&text),
                Ok(Command_::NewSession) => {
                    self.close_session();
                    next_try = Instant::now();
                    retry = RETRY_FIRST;
                }
                Ok(Command_::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.close_session();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn open_session(&mut self) -> Result<(), String> {
        let bin = self.config.bin.clone().or_else(find_codex).ok_or_else(|| NOT_INSTALLED.to_string())?;
        if self.config.require_sign_in && !signed_in(&bin)? {
            return Err(SIGNED_OUT.into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let log = ConversationLog::create(&super::transcript::conversations_dir(&self.config.data_dir), &id)
            .map_err(|e| tracing::warn!("conversation file not created: {e:#}"))
            .ok();
        println!("codex brain: session {id} ready ({})", bin.display());
        self.bin = Some(bin);
        self.session = Some(Session { id: id.clone(), thread: None, log });
        self.problem = None;
        (self.emit)(Input::SessionReady { session_id: id });
        Ok(())
    }

    fn close_session(&mut self) {
        if let Some(s) = self.session.take() {
            self.state.end_session(&s.id);
        }
    }

    fn turn(&mut self, text: &str) {
        let (Some(s), Some(bin)) = (self.session.as_ref(), self.bin.clone()) else {
            let message = self.problem.clone().unwrap_or_else(|| "The assistant is still starting. Try again in a moment.".into());
            (self.emit)(Input::TurnFailed { message });
            return;
        };
        self.cancel.store(false, Ordering::SeqCst);
        let sid = s.id.clone();
        if let Some(l) = &s.log {
            l.user(text);
        }
        (self.emit)(Input::TranscriptPrompt { session_id: sid.clone() });
        let mut ended = self.run_codex(&bin, text, &sid);
        // A thread Codex can no longer find: start a fresh one, once.
        if let Ended::Failed(m) = &ended
            && self.session.as_ref().is_some_and(|s| s.thread.is_some())
            && resume_failed(m)
        {
            if let Some(s) = self.session.as_mut() {
                s.thread = None;
            }
            ended = self.run_codex(&bin, text, &sid);
        }
        match ended {
            Ended::Done(last) => (self.emit)(Input::Stop { session_id: sid, last_message: last }),
            Ended::Failed(message) => (self.emit)(Input::TurnFailed { message }),
            Ended::Interrupted => {
                if let Some(l) = self.session.as_ref().and_then(|s| s.log.as_ref()) {
                    l.interrupted();
                }
            }
        }
    }

    /// One `codex exec` run for `text`, its events turned into inputs.
    fn run_codex(&mut self, bin: &Path, text: &str, sid: &str) -> Ended {
        let thread = self.session.as_ref().and_then(|s| s.thread.clone());
        let mut cmd = Command::new(bin);
        cmd.args(exec_args(&self.config, sid, thread.as_deref()))
            .current_dir(&self.config.cwd)
            .env("PINDOT_MCP_TOKEN", &self.config.token)
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::hidden(&mut cmd);
        let mut child: Child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Ended::Failed(format!("Codex could not be started: {e}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let (tx, rx) = mpsc::channel::<String>();
        if let Some(out) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            });
        }
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(err) = child.stderr.take() {
            let tail = stderr_tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut t = tail.lock().unwrap();
                    t.push_str(&line);
                    t.push('\n');
                    if t.len() > 4000 {
                        let cut = t.len() - 4000;
                        let at = t.char_indices().find(|(i, _)| *i >= cut).map_or(0, |(i, _)| i);
                        t.drain(..at);
                    }
                }
            });
        }
        let started = Instant::now();
        let mut last: Option<String> = None;
        let ended = loop {
            if self.cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                break Ended::Interrupted;
            }
            if started.elapsed() > TURN_TIMEOUT {
                let _ = child.kill();
                break Ended::Failed("This took longer than 15 minutes, so I stopped.".into());
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => match parse_event(&line) {
                    Some(ev) => {
                        if let Some(end) = self.on_event(ev, sid, &mut last) {
                            break end;
                        }
                    }
                    None => {}
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = child.wait().ok();
                    let tail = stderr_tail.lock().unwrap().trim().to_string();
                    let why = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                    break Ended::Failed(describe_failure(&why, status.and_then(|s| s.code())));
                }
            }
        };
        let _ = child.wait();
        ended
    }

    fn on_event(&mut self, ev: CodexEvent, sid: &str, last: &mut Option<String>) -> Option<Ended> {
        let log = self.session.as_ref().and_then(|s| s.log.as_ref());
        match ev {
            CodexEvent::Thread(t) => {
                if let Some(s) = self.session.as_mut() {
                    s.thread = Some(t);
                }
            }
            CodexEvent::ToolStart { id, name, arguments } => {
                if let Some(l) = log {
                    l.assistant("", "", &[ToolCall { id, name: name.clone(), arguments: arguments.to_string() }]);
                }
                (self.emit)(Input::TranscriptToolUse { session_id: sid.into(), name: format!("{TOOL_PREFIX}{name}") });
            }
            CodexEvent::ToolDone { id, text, is_error } => {
                if let Some(l) = log {
                    l.tool_result(&id, &text, is_error);
                }
                (self.emit)(Input::TranscriptToolResult { session_id: sid.into() });
            }
            CodexEvent::Message(text) => {
                if let Some(l) = log {
                    l.assistant("", &text, &[]);
                }
                if !text.trim().is_empty() {
                    *last = Some(text.clone());
                    (self.emit)(Input::TranscriptText { session_id: sid.into(), text });
                }
            }
            CodexEvent::Reasoning(text) => {
                if let Some(l) = log {
                    l.assistant(&text, "", &[]);
                }
            }
            CodexEvent::Warning(w) => println!("codex brain: {w}"),
            CodexEvent::TurnDone => return Some(Ended::Done(last.take())),
            CodexEvent::TurnFailed(m) => return Some(Ended::Failed(describe_failure(&m, None))),
        }
        None
    }
}

/// Codex's words for a thread it can no longer resume.
fn resume_failed(m: &str) -> bool {
    let m = m.to_ascii_lowercase();
    m.contains("no rollout found") || (m.contains("not found") && (m.contains("thread") || m.contains("session") || m.contains("rollout")))
}

/// A failed turn in words for the user.
fn describe_failure(why: &str, code: Option<i32>) -> String {
    let w = why.to_ascii_lowercase();
    if w.contains("not logged in") || w.contains("login") && w.contains("required") || w.contains("401") {
        return SIGNED_OUT.into();
    }
    if w.contains("usage limit") || w.contains("rate limit") || w.contains("429") {
        return "Your ChatGPT plan's Codex limit is reached for now. Try again later.".into();
    }
    match (why.trim().is_empty(), code) {
        (true, Some(c)) => format!("Codex stopped (exit code {c})."),
        (true, None) => "Codex stopped without an answer.".into(),
        (false, _) => format!("Codex stopped: {}", why.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CodexConfig {
        CodexConfig {
            bin: None,
            model: Some("gpt-x".into()),
            mcp_url: "http://127.0.0.1:47823/mcp".into(),
            token: "tok".into(),
            system_prompt: "Say \"hi\".\nBe brief.".into(),
            cwd: PathBuf::from("/tmp/s"),
            data_dir: PathBuf::from("/tmp/d"),
            extra_config: vec![],
            require_sign_in: true,
        }
    }

    #[test]
    fn a_new_turn_and_a_resumed_one_get_pindots_tools_only() {
        let a = exec_args(&cfg(), "sess-1", None);
        assert_eq!(&a[..2], ["exec", "--json"]);
        assert_eq!(a.last().map(String::as_str), Some("-"), "the prompt comes on stdin");
        let joined = a.join(" ");
        for want in [
            "sandbox_mode=\"read-only\"",
            "forced_login_method=\"chatgpt\"",
            "mcp_servers.desktop.url=\"http://127.0.0.1:47823/mcp\"",
            "mcp_servers.desktop.bearer_token_env_var=\"PINDOT_MCP_TOKEN\"",
            "mcp_servers.desktop.http_headers={\"X-Pindot-Session\"=\"sess-1\"}",
            "--disable shell_tool",
            "--disable unified_exec",
            "--ignore-user-config",
            "-m gpt-x",
            r#"developer_instructions="Say \"hi\".\nBe brief.""#,
        ] {
            assert!(joined.contains(want), "missing {want} in {joined}");
        }
        let r = exec_args(&cfg(), "sess-1", Some("thread-9"));
        assert_eq!(&r[..3], ["exec", "resume", "--json"]);
        assert_eq!(&r[r.len() - 2..], ["thread-9", "-"]);
        assert!(!r.contains(&"-s".to_string()) && !r.contains(&"-C".to_string()), "resume takes neither");
    }

    #[test]
    fn events_from_a_real_run_are_read() {
        // Lines as Codex 0.169 printed them for a desktop_context call.
        let lines = [
            r#"{"type":"thread.started","thread_id":"01a1e530-c617-72a1-9dec-c792fda2e1d4"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Model metadata for `qwen3:4b` not found."}}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"reasoning","text":"I need to call the tool."}}"#,
            r#"{"type":"item.started","item":{"id":"item_2","type":"mcp_tool_call","server":"desktop","tool":"desktop_context","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"desktop","tool":"desktop_context","arguments":{},"result":{"content":[{"type":"text","text":"Displays:\n  0: main"}]},"error":null,"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"There is **1 display**."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":12826,"output_tokens":609}}"#,
        ];
        let ev: Vec<CodexEvent> = lines.iter().filter_map(|l| parse_event(l)).collect();
        assert_eq!(
            ev,
            vec![
                CodexEvent::Thread("01a1e530-c617-72a1-9dec-c792fda2e1d4".into()),
                CodexEvent::Warning("Model metadata for `qwen3:4b` not found.".into()),
                CodexEvent::Reasoning("I need to call the tool.".into()),
                CodexEvent::ToolStart { id: "item_2".into(), name: "desktop_context".into(), arguments: serde_json::json!({}) },
                CodexEvent::ToolDone { id: "item_2".into(), text: "Displays:\n  0: main".into(), is_error: false },
                CodexEvent::Message("There is **1 display**.".into()),
                CodexEvent::TurnDone,
            ]
        );
        assert_eq!(parse_event(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#), Some(CodexEvent::TurnFailed("usage limit reached".into())));
        let failed_tool = r#"{"type":"item.completed","item":{"id":"i","type":"mcp_tool_call","server":"desktop","tool":"click","arguments":{},"result":null,"error":{"message":"refused"},"status":"failed"}}"#;
        assert_eq!(parse_event(failed_tool), Some(CodexEvent::ToolDone { id: "i".into(), text: "refused".into(), is_error: true }));
        assert_eq!(parse_event("not json"), None);
    }

    #[test]
    fn the_native_binary_is_found_inside_the_npm_package() {
        let root = std::env::temp_dir().join(format!("pindot-codex-pkg-{}", uuid::Uuid::new_v4()));
        let name = if cfg!(windows) { "codex.exe" } else { "codex" };
        let bin_dir = root.join("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        assert_eq!(native_in_package(&root), None);
        std::fs::write(bin_dir.join(name), "x").unwrap();
        assert_eq!(native_in_package(&root), Some(bin_dir.join(name)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string("a \"b\"\\c\nd"), r#""a \"b\"\\c\nd""#);
    }
}
