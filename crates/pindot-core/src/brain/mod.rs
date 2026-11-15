//! The brain: the user's own `claude` CLI, signed in with `claude login`,
//! run INTERACTIVELY inside a pseudo-terminal.
//!
//! Hard rules (see CLAUDE.md):
//! - never `claude -p`, never the Agent SDK, never an API key;
//! - `ANTHROPIC_API_KEY` is blanked in the child env so a stray key can't
//!   switch the CLI to metered API billing;
//! - the MCP server and hooks are passed per session with `--mcp-config
//!   --strict-mcp-config` and `--settings`, so nothing global changes.
//!
//! We type the prompt into the TUI, learn that the turn finished from the
//! `Stop` HTTP hook, and read the answer from the hook's
//! `last_assistant_message` (primary) and the session transcript (check).

pub mod config;
pub mod persistent;
pub mod pty;
pub mod tailer;
pub mod transcript;
pub mod turn;

use std::path::{Path, PathBuf};
use std::process::Command;

pub use pty::{ClaudeSession, SpawnOptions};

/// Find the `claude` executable: `PINDOT_CLAUDE_BIN`, then `PATH`, then
/// the usual install locations, then the user's login shell (GUI apps on
/// macOS start with a minimal PATH).
pub fn find_claude() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PINDOT_CLAUDE_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join(exe);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        for rel in [".local/bin/claude", ".claude/local/claude", ".npm-global/bin/claude", ".bun/bin/claude"] {
            let c = home.join(rel);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    for abs in ["/opt/homebrew/bin/claude", "/usr/local/bin/claude"] {
        if Path::new(abs).is_file() {
            return Some(PathBuf::from(abs));
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        if let Ok(out) = Command::new(shell).args(["-lc", "command -v claude"]).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() && Path::new(&s).is_file() {
                return Some(PathBuf::from(s));
            }
        }
    }
    None
}

/// `claude --version`, for diagnostics.
pub fn claude_version(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").env("ANTHROPIC_API_KEY", "").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    /// Signed in; `method` as Claude Code reports it (e.g. `claude.ai`).
    SignedIn { method: String },
    SignedOut,
    /// The check itself failed (old CLI without `auth status`, bad output).
    Unknown(String),
}

/// The message Pindot shows when `claude` is not signed in. Pindot never
/// handles the login itself.
pub const SIGN_IN_HINT: &str = "Claude Code is not signed in. Open a terminal, run `claude login`, then try again.";

/// Ask the CLI whether it is signed in (`claude auth status --json`), with
/// `ANTHROPIC_API_KEY` blanked like every child we start. Only `loggedIn`
/// and `authMethod` are read; the rest (account details) is ignored.
pub fn auth_status(bin: &Path) -> AuthState {
    let mut cmd = Command::new(bin);
    cmd.args(["auth", "status", "--json"]).env("ANTHROPIC_API_KEY", "");
    for k in pty::PARENT_SESSION_VARS {
        cmd.env_remove(k);
    }
    match cmd.output() {
        Ok(out) => parse_auth_status(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => AuthState::Unknown(format!("could not run {} auth status: {e}", bin.display())),
    }
}

pub fn parse_auth_status(stdout: &str) -> AuthState {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        return AuthState::Unknown("unexpected `claude auth status` output".into());
    };
    match v.get("loggedIn").and_then(serde_json::Value::as_bool) {
        Some(true) => AuthState::SignedIn {
            method: v.get("authMethod").and_then(serde_json::Value::as_str).unwrap_or("unknown").to_string(),
        },
        Some(false) => AuthState::SignedOut,
        None => AuthState::Unknown("`claude auth status` has no loggedIn field".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_reads_only_logged_in_and_method() {
        let s = parse_auth_status(r#"{"loggedIn":true,"authMethod":"claude.ai","email":"x@y.z","orgName":"Org"}"#);
        assert_eq!(s, AuthState::SignedIn { method: "claude.ai".into() });
        assert_eq!(parse_auth_status(r#"{"loggedIn":false}"#), AuthState::SignedOut);
        assert!(matches!(parse_auth_status("Not logged in"), AuthState::Unknown(_)));
    }
}
