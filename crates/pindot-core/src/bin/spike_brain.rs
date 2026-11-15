//! Spikes (c) and (d): drive the user's own interactive `claude` in a PTY.
//!
//!   spike_brain --prompt "Reply with exactly: pong" [--desktop] [--accept-trust]
//!               [--model haiku] [--timeout 180] [--cwd DIR] [--permission-mode MODE]
//!
//! Starts the Pindot server (MCP tools + Stop hook receiver), writes the
//! `--mcp-config` / `--settings` files, spawns `claude` (never `-p`) with
//! ANTHROPIC_API_KEY blanked, waits for the TUI, types the prompt, waits for
//! the Stop hook for *this* session id, confirms the answer in the
//! transcript, then sends `/exit`. With `--desktop` the session gets the
//! screenshot/click tools through `--allowedTools mcp__desktop`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use pindot_core::brain::config::{ALLOWED_TOOLS_RULE, write_hook_settings, write_mcp_config};
use pindot_core::brain::transcript::{find_transcript, wait_for_last_turn};
use pindot_core::brain::turn::{TurnEnd, drive_turn};
use pindot_core::brain::{ClaudeSession, SpawnOptions, claude_version, find_claude};
use pindot_core::paths::load_or_create_token;
use pindot_core::platform::{Overlay, Point};
use pindot_core::server::activity::ActivityLog;
use pindot_core::server::approval::NoApprover;
use pindot_core::server::{ServerDeps, ServerState, StopHookEvent, start};

struct LogOverlay;

impl Overlay for LogOverlay {
    fn move_cursor(&self, to: Point, _label: Option<String>) -> Duration {
        println!("overlay: AI cursor to ({:.0}, {:.0}) pt", to.x, to.y);
        Duration::ZERO
    }
    fn set_acting(&self, _acting: bool) {}
}

fn main() -> anyhow::Result<()> {
    pindot_core::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let prompt = opt("--prompt").unwrap_or_else(|| "Reply with exactly the word: pong".into());
    let desktop = flag("--desktop");
    let timeout = Duration::from_secs(opt("--timeout").and_then(|s| s.parse().ok()).unwrap_or(180));
    let cwd = match opt("--cwd") {
        Some(c) => c.into(),
        None => dirs::home_dir().context("home")?.join("pindot").join(".spike-cwd"),
    };
    std::fs::create_dir_all(&cwd)?;

    let bin = find_claude().context("claude CLI not found (set PINDOT_CLAUDE_BIN)")?;
    println!("claude: {} ({})", bin.display(), claude_version(&bin).unwrap_or_default());
    println!(
        "parent env: ANTHROPIC_API_KEY {}",
        if std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|v| !v.is_empty()) { "set (must be blanked in child)" } else { "unset" }
    );

    let rt = tokio::runtime::Runtime::new()?;
    let state = ServerState::new(ServerDeps::native(
        Arc::new(LogOverlay),
        Arc::new(NoApprover),
        Arc::new(ActivityLog::memory()),
    ));
    let token = load_or_create_token()?;
    let server = rt.block_on(start(state.clone(), &token, 0))?;
    println!("server: {}", server.mcp_url());

    let mut opts = SpawnOptions::new(bin, cwd.clone());
    opts.settings = Some(write_hook_settings(&server.stop_hook_url(), &token)?);
    if desktop {
        opts.mcp_config = Some(write_mcp_config(&server.mcp_url(), &token)?);
        opts.allowed_tools = vec![ALLOWED_TOOLS_RULE.into()];
    }
    opts.model = opt("--model");
    opts.permission_mode = opt("--permission-mode");
    println!("cwd: {}", cwd.display());
    println!("args: {}", opts.args().join(" "));

    let t0 = Instant::now();
    let mut session = ClaudeSession::spawn(&opts)?;
    let pid = session.pid();
    println!("spawned interactive claude, pid {pid:?}, session {}", session.session_id);
    if let Some(pid) = pid {
        report_child_env(pid);
    }

    session.wait_ready(Duration::from_secs(60), flag("--accept-trust"))?;
    println!("TUI ready after {:.1}s; typing prompt: {prompt}", t0.elapsed().as_secs_f64());
    let t1 = Instant::now();
    let want = session.session_id.to_string();
    let end = drive_turn(&mut session, &state, &prompt, timeout, &mut |t| {
        println!("tool call: {} {} -> {}", t.tool, t.args, t.outcome)
    })?;
    let ev: StopHookEvent = match end {
        TurnEnd::Stopped(ev) => ev,
        other => {
            let _ = session.interrupt();
            let _ = session.exit(Duration::from_secs(5));
            state.control.shutdown();
            server.shutdown();
            bail!("turn did not finish normally: {other:?}");
        }
    };
    println!("Stop hook after {:.1}s: last_assistant_message = {:?}", t1.elapsed().as_secs_f64(), ev.last_assistant_message);

    let path = ev
        .transcript_path
        .clone()
        .map(Into::into)
        .or_else(|| find_transcript(&want))
        .context("no transcript path")?;
    println!("transcript: {}", path_display(&path));
    let (turn, matched) = wait_for_last_turn(&path, ev.last_assistant_message.as_deref(), Duration::from_secs(10))?;
    println!("transcript prompt: {:?}", turn.prompt);
    println!("transcript final text: {:?}", turn.final_text);
    println!("transcript tool calls: {:?}", turn.tool_uses);
    println!("hook and transcript agree: {matched}");

    session.exit(Duration::from_secs(8))?;
    std::thread::sleep(Duration::from_millis(300));
    let alive = pid.map(|p| std::process::Command::new("kill").args(["-0", &p.to_string()]).status().map(|s| s.success()).unwrap_or(false));
    println!("child claude still running after /exit: {alive:?}");
    state.control.shutdown();
    server.shutdown();
    drop(rt);

    let ok = matched && turn.prompt.as_deref().map(str::trim) == Some(prompt.trim()) && alive == Some(false);
    println!("RESULT {}", if ok { "PASS" } else { "CHECK OUTPUT" });
    Ok(())
}

fn path_display(p: &std::path::Path) -> String {
    p.display().to_string()
}

/// Show (without printing other values) that the child got a blank
/// ANTHROPIC_API_KEY and none of the parent-session variables.
fn report_child_env(pid: u32) {
    let Ok(out) = std::process::Command::new("ps").args(["-E", "-ww", "-o", "command=", "-p", &pid.to_string()]).output() else {
        return;
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let words: Vec<&str> = s.split_whitespace().collect();
    let key = words.iter().find(|w| w.starts_with("ANTHROPIC_API_KEY="));
    let leaked: Vec<&str> = words
        .iter()
        .filter(|w| w.starts_with("CLAUDECODE=") || w.starts_with("CLAUDE_CODE_SESSION_ID=") || w.starts_with("CLAUDE_CODE_ENTRYPOINT="))
        .map(|w| w.split('=').next().unwrap_or(""))
        .collect();
    println!(
        "child env: ANTHROPIC_API_KEY {}; parent-session vars present: {:?}; TERM {}",
        match key {
            Some(k) if *k == "ANTHROPIC_API_KEY=" => "blank",
            Some(_) => "NOT BLANK",
            None => "absent from ps output",
        },
        leaked,
        words.iter().find(|w| w.starts_with("TERM=")).copied().unwrap_or("?")
    );
}
