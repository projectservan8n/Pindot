//! Per-session config files handed to `claude` with `--mcp-config` and
//! `--settings`. They hold the literal bearer token, so they live in the
//! user-only data directory (0600), never in the repo or the working dir.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::json;

use crate::paths::{data_dir, write_private};

/// Name of our MCP server inside Claude Code; tools show up as
/// `mcp__desktop__read_window`, `mcp__desktop__click` and so on.
pub const MCP_SERVER_NAME: &str = "desktop";

/// Permission rule that allows every tool of our server (`--allowedTools`).
pub const ALLOWED_TOOLS_RULE: &str = "mcp__desktop";

pub fn mcp_config_json(mcp_url: &str, token: &str) -> serde_json::Value {
    json!({
        "mcpServers": {
            MCP_SERVER_NAME: {
                "type": "http",
                "url": mcp_url,
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    })
}

/// `Stop` tells the brain a turn finished; `UserPromptSubmit` tells the
/// server a new user turn began (which lifts an Esc halt). Both POST to our
/// loopback server with the bearer token.
pub fn hook_settings_json(stop_hook_url: &str, token: &str) -> serde_json::Value {
    let prompt_hook_url = stop_hook_url.replace("/hooks/stop", "/hooks/prompt");
    let hook = |url: &str| {
        json!([{
            "hooks": [{
                "type": "http",
                "url": url,
                "timeout": 10,
                "headers": { "Authorization": format!("Bearer {token}") }
            }]
        }])
    };
    json!({
        "hooks": {
            "Stop": hook(stop_hook_url),
            "UserPromptSubmit": hook(&prompt_hook_url),
        }
    })
}

pub fn write_mcp_config(mcp_url: &str, token: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-mcp.json");
    write_private(&p, serde_json::to_string_pretty(&mcp_config_json(mcp_url, token))?.as_bytes())?;
    Ok(p)
}

/// The MCP config for one `claude` that Deskhand starts: as
/// `mcp_config_json`, plus the `X-Deskhand-Session` header naming its
/// session, so approvals and refs survive Claude Code's stateless
/// (MCP 2026-07-28) tool calls.
pub fn brain_mcp_config_json(mcp_url: &str, token: &str, session_key: &str) -> serde_json::Value {
    let mut v = mcp_config_json(mcp_url, token);
    v["mcpServers"][MCP_SERVER_NAME]["headers"]["X-Deskhand-Session"] = json!(session_key);
    v
}

pub fn write_brain_mcp_config(mcp_url: &str, token: &str, session_key: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-mcp-brain.json");
    write_private(&p, serde_json::to_string_pretty(&brain_mcp_config_json(mcp_url, token, session_key))?.as_bytes())?;
    Ok(p)
}

pub fn write_hook_settings(stop_hook_url: &str, token: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-settings.json");
    write_private(&p, serde_json::to_string_pretty(&hook_settings_json(stop_hook_url, token))?.as_bytes())?;
    Ok(p)
}
