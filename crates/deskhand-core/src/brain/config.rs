//! Per-session config files handed to `claude` with `--mcp-config` and
//! `--settings`. They hold the literal bearer token, so they live in the
//! user-only data directory (0600), never in the repo or the working dir.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::paths::{data_dir, write_private};

/// Name of our MCP server inside Claude Code; tools show up as
/// `mcp__desktop__read_window`, `mcp__desktop__click` and so on.
pub const MCP_SERVER_NAME: &str = "desktop";

/// Permission rule that allows every tool of our server (`--allowedTools`).
pub const ALLOWED_TOOLS_RULE: &str = "mcp__desktop";

/// How the voice brain handles tools that are not ours (a setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Only Deskhand's desktop tools: no built-in tools at all, because a
    /// permission prompt can't be answered by voice.
    #[default]
    Ask,
    /// Claude Code's auto mode with a small set of built-in tools: its
    /// classifier, not the user, approves each action. The settings screen
    /// shows a warning before it can be chosen.
    Auto,
}

impl PermissionMode {
    /// `--permission-mode` (and `permissions.defaultMode`).
    pub fn cli(self) -> &'static str {
        match self {
            PermissionMode::Ask => "default",
            PermissionMode::Auto => "auto",
        }
    }

    /// `--tools`: the built-in tools the brain gets. The full set costs about
    /// 31k tokens on every call (measured: a 40k-token baseline drops to
    /// about 5k with none and 10k with these).
    pub fn builtin_tools(self) -> &'static [&'static str] {
        match self {
            PermissionMode::Ask => &[],
            PermissionMode::Auto => &["Bash", "Read", "Edit", "Write", "Glob", "Grep", "WebFetch", "WebSearch"],
        }
    }
}

/// `--model` aliases offered in Settings; a full `claude-...` id also works.
pub const CLAUDE_MODELS: &[&str] = &["haiku", "sonnet", "opus", "fable"];
/// `--effort` levels of claude 2.1.308.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

pub fn model_ok(m: &str) -> bool {
    CLAUDE_MODELS.contains(&m)
        || (m.starts_with("claude-") && m.chars().all(|c| c.is_ascii_alphanumeric() || "-.[]".contains(c)))
}

pub fn effort_ok(e: &str) -> bool {
    EFFORT_LEVELS.contains(&e)
}

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
    brain_hook_settings_json(stop_hook_url, token, PermissionMode::Ask)
}

/// As `hook_settings_json`, with the voice brain's permission mode.
pub fn brain_hook_settings_json(stop_hook_url: &str, token: &str, mode: PermissionMode) -> serde_json::Value {
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
        // Ask before any tool that is not ours (with `--allowedTools` for
        // ours), or auto mode if the user chose it. Set here as well as on
        // the command line, so a project or local settings file can't
        // change the mode.
        "permissions": { "defaultMode": mode.cli() },
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

/// The voice brain's own settings file, so its mode never leaks into the
/// shared `claude-settings.json`.
pub fn write_brain_hook_settings(stop_hook_url: &str, token: &str, mode: PermissionMode) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-settings-brain.json");
    let v = brain_hook_settings_json(stop_hook_url, token, mode);
    write_private(&p, serde_json::to_string_pretty(&v)?.as_bytes())?;
    Ok(p)
}

pub fn write_hook_settings(stop_hook_url: &str, token: &str) -> Result<PathBuf> {
    let p = data_dir()?.join("claude-settings.json");
    write_private(&p, serde_json::to_string_pretty(&hook_settings_json(stop_hook_url, token))?.as_bytes())?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOP: &str = "http://127.0.0.1:47823/hooks/stop";

    #[test]
    fn ask_mode_has_no_built_in_tools_and_asks_for_anything_else() {
        assert_eq!(PermissionMode::default(), PermissionMode::Ask);
        assert!(PermissionMode::Ask.builtin_tools().is_empty());
        assert_eq!(PermissionMode::Ask.cli(), "default");
        let v = brain_hook_settings_json(STOP, "tok", PermissionMode::Ask);
        assert_eq!(v["permissions"]["defaultMode"], "default");
        assert!(v["hooks"]["Stop"].is_array() && v["hooks"]["UserPromptSubmit"].is_array());
    }

    #[test]
    fn auto_mode_gets_a_small_tool_set_and_the_auto_mode() {
        let tools = PermissionMode::Auto.builtin_tools();
        for t in ["Bash", "Read", "Edit", "Write", "WebFetch"] {
            assert!(tools.contains(&t), "{t} missing");
        }
        // Tools that cost thousands of tokens each or need answers a voice
        // user can't give.
        for t in ["Artifact", "Agent", "Workflow", "AskUserQuestion", "Skill", "ToolSearch"] {
            assert!(!tools.contains(&t), "{t} should be left out");
        }
        assert_eq!(PermissionMode::Auto.cli(), "auto");
        let v = brain_hook_settings_json(STOP, "tok", PermissionMode::Auto);
        assert_eq!(v["permissions"]["defaultMode"], "auto");
    }

    #[test]
    fn permission_mode_is_saved_in_snake_case() {
        assert_eq!(serde_json::to_value(PermissionMode::Auto).unwrap(), "auto");
        let m: PermissionMode = serde_json::from_value(json!("ask")).unwrap();
        assert_eq!(m, PermissionMode::Ask);
    }

    #[test]
    fn model_and_effort_names_are_checked() {
        assert!(model_ok("sonnet") && model_ok("haiku") && model_ok("claude-opus-5-5"));
        assert!(!model_ok("gpt-5") && !model_ok("") && !model_ok("sonnet --print"));
        assert!(effort_ok("low") && effort_ok("xhigh") && effort_ok("max"));
        assert!(!effort_ok("turbo") && !effort_ok(""));
    }
}
