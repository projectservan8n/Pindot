//! Per-session config files handed to `claude` with `--mcp-config` and
//! `--settings`. They hold the literal bearer token, so they live in the
//! user-only data directory (0600), never in the repo or the working dir.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::json;

use crate::paths::{data_dir, write_private};

/// Name of our MCP server inside Claude Code; tools show up as
/// `mcp__desktop__screenshot`, `mcp__desktop__click`.
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

pub fn hook_settings_json(stop_hook_url: &str, token: &str) -> serde_json::Value {
    json!({
        "hooks": {
            "Stop": [{
                "hooks": [{
                    "type": "http",
                    "url": stop_hook_url,
                    "timeout": 10,
                    "headers": { "Authorization": format!("Bearer {token}") }
                }]
            }]
        }
    })
}

pub fn write_mcp_config(mcp_url: &str, token: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-mcp.json");
    write_private(&p, serde_json::to_string_pretty(&mcp_config_json(mcp_url, token))?.as_bytes())?;
    Ok(p)
}

pub fn write_hook_settings(stop_hook_url: &str, token: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-settings.json");
    write_private(&p, serde_json::to_string_pretty(&hook_settings_json(stop_hook_url, token))?.as_bytes())?;
    Ok(p)
}
