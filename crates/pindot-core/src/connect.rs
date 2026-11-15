//! "Connect to Claude Code": register Pindot's MCP server with the user's
//! own `claude` (terminal mode), so any `claude` session gets the desktop
//! tools.
//!
//! The button runs
//! `claude mcp add --transport http --scope user desktop http://127.0.0.1:<port>/mcp --header "Authorization: Bearer <token>"`
//! and "Disconnect" runs `claude mcp remove --scope user desktop`. Both
//! change the user's own Claude Code config (`~/.claude.json`), so they only
//! ever run when the user presses the button. The status is read from that
//! file without changing it.
//!
//! Terminal sessions do not send Pindot's session header, so they all
//! share one Pindot session (approvals, input lock). See the README.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// The MCP server name Claude Code sees (tools are `mcp__desktop__*`).
pub const SERVER_NAME: &str = "desktop";

pub fn mcp_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Arguments for `claude` that add the user-scope MCP server.
pub fn add_args(port: u16, token: &str) -> Vec<String> {
    [
        "mcp",
        "add",
        "--transport",
        "http",
        "--scope",
        "user",
        SERVER_NAME,
        &mcp_url(port),
        "--header",
        &format!("Authorization: Bearer {token}"),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Arguments for `claude` that remove it again.
pub fn remove_args() -> Vec<String> {
    ["mcp", "remove", "--scope", "user", SERVER_NAME].iter().map(|s| s.to_string()).collect()
}

/// The command as a user would paste it into a terminal (POSIX quoting).
/// `mask_token` hides the token for display.
pub fn shell_line(args: &[String], mask_token: bool) -> String {
    let mut parts = vec!["claude".to_string()];
    for a in args {
        let a = match a.strip_prefix("Authorization: Bearer ") {
            Some(_) if mask_token => "Authorization: Bearer <token>".to_string(),
            _ => a.clone(),
        };
        parts.push(shell_quote(&a));
    }
    parts.join(" ")
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// What `~/.claude.json` says about the `desktop` server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectStatus {
    NotConnected,
    /// Registered with this URL and token.
    Connected,
    /// Registered, but with another URL or an old token (Connect again).
    Outdated { reason: String },
    /// Another MCP server already uses the name `desktop`.
    NameTaken { detail: String },
    Unknown { error: String },
}

/// `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`), read only.
pub fn claude_json_path() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join(".claude.json"));
    }
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

pub fn status(port: u16, token: &str) -> ConnectStatus {
    let Some(path) = claude_json_path() else { return ConnectStatus::Unknown { error: "no home folder".into() } };
    status_from_file(&path, port, token)
}

pub fn status_from_file(path: &Path, port: u16, token: &str) -> ConnectStatus {
    match std::fs::read_to_string(path) {
        Ok(text) => status_from_json(&text, port, token),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ConnectStatus::NotConnected,
        Err(e) => ConnectStatus::Unknown { error: format!("reading {}: {e}", path.display()) },
    }
}

/// Only `mcpServers.desktop` is looked at; nothing else in the file is
/// read into memory beyond the parse.
pub fn status_from_json(text: &str, port: u16, token: &str) -> ConnectStatus {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return ConnectStatus::Unknown { error: format!("~/.claude.json is not JSON: {e}") },
    };
    let Some(entry) = v.get("mcpServers").and_then(|m| m.get(SERVER_NAME)) else {
        return ConnectStatus::NotConnected;
    };
    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or_default();
    let kind = entry.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    let localhost = url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:");
    if kind != "http" || !localhost || !url.ends_with("/mcp") {
        return ConnectStatus::NameTaken { detail: format!("\"{SERVER_NAME}\" is {kind} {url}") };
    }
    if url != mcp_url(port) {
        return ConnectStatus::Outdated { reason: format!("it points to {url}, Pindot is on port {port}") };
    }
    let auth = entry
        .get("headers")
        .and_then(|h| h.as_object())
        .and_then(|h| h.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")))
        .and_then(|(_, v)| v.as_str())
        .unwrap_or_default();
    if auth != format!("Bearer {token}") {
        return ConnectStatus::Outdated { reason: "it has an old token (the token was regenerated)".into() };
    }
    ConnectStatus::Connected
}

/// FOR THE USER'S BUTTON ONLY: runs `claude` with `args` and returns its
/// output. Changes the user's Claude Code config.
pub fn run(claude: &Path, args: &[String]) -> Result<String> {
    let mut cmd = Command::new(claude);
    cmd.args(args).env("ANTHROPIC_API_KEY", "");
    for k in crate::brain::pty::PARENT_SESSION_VARS {
        cmd.env_remove(k);
    }
    let out = cmd.output().with_context(|| format!("running {}", claude.display()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("claude {} failed ({}): {}", args.first().map(String::as_str).unwrap_or(""), out.status, text.trim());
    }
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_command_is_built_exactly() {
        let args = add_args(47823, "abc123");
        assert_eq!(
            args,
            [
                "mcp",
                "add",
                "--transport",
                "http",
                "--scope",
                "user",
                "desktop",
                "http://127.0.0.1:47823/mcp",
                "--header",
                "Authorization: Bearer abc123"
            ]
        );
        // The header stays one argument (no shell in between).
        assert_eq!(args.iter().filter(|a| a.contains("Bearer")).count(), 1);
        assert_eq!(
            shell_line(&args, false),
            "claude mcp add --transport http --scope user desktop http://127.0.0.1:47823/mcp --header 'Authorization: Bearer abc123'"
        );
        assert_eq!(
            shell_line(&args, true),
            "claude mcp add --transport http --scope user desktop http://127.0.0.1:47823/mcp --header 'Authorization: Bearer <token>'"
        );
        assert_eq!(add_args(50000, "t")[7], "http://127.0.0.1:50000/mcp");
    }

    #[test]
    fn remove_command_is_built_exactly() {
        assert_eq!(remove_args(), ["mcp", "remove", "--scope", "user", "desktop"]);
        assert_eq!(shell_line(&remove_args(), true), "claude mcp remove --scope user desktop");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn status_is_read_from_claude_json() {
        let entry = |url: &str, tok: &str| {
            format!(
                r#"{{"numStartups":3,"mcpServers":{{"other":{{"type":"stdio","command":"x"}},"desktop":{{"type":"http","url":"{url}","headers":{{"Authorization":"Bearer {tok}"}}}}}}}}"#
            )
        };
        assert_eq!(status_from_json(&entry("http://127.0.0.1:47823/mcp", "tok"), 47823, "tok"), ConnectStatus::Connected);
        assert!(matches!(
            status_from_json(&entry("http://127.0.0.1:47823/mcp", "old"), 47823, "tok"),
            ConnectStatus::Outdated { .. }
        ));
        assert!(matches!(
            status_from_json(&entry("http://127.0.0.1:40000/mcp", "tok"), 47823, "tok"),
            ConnectStatus::Outdated { .. }
        ));
        assert!(matches!(
            status_from_json(&entry("https://example.com/mcp", "tok"), 47823, "tok"),
            ConnectStatus::NameTaken { .. }
        ));
        assert_eq!(status_from_json(r#"{"mcpServers":{}}"#, 47823, "tok"), ConnectStatus::NotConnected);
        assert_eq!(status_from_json("{}", 47823, "tok"), ConnectStatus::NotConnected);
        assert!(matches!(status_from_json("not json", 47823, "tok"), ConnectStatus::Unknown { .. }));
        let missing = std::env::temp_dir().join("dh-no-such-claude.json");
        assert_eq!(status_from_file(&missing, 47823, "tok"), ConnectStatus::NotConnected);
    }
}
