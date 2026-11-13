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
pub mod pty;
pub mod transcript;

use std::path::{Path, PathBuf};
use std::process::Command;

pub use pty::{ClaudeSession, SpawnOptions};

/// Find the `claude` executable: `DESKHAND_CLAUDE_BIN`, then `PATH`, then
/// the usual install locations, then the user's login shell (GUI apps on
/// macOS start with a minimal PATH).
pub fn find_claude() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DESKHAND_CLAUDE_BIN") {
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
