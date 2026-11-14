//! `--test-turn PROMPT`: one real turn of the user's interactive `claude`
//! (PTY, `claude login`, never `-p`) against this app's own server, overlay
//! and approval window. Used for the Phase 1 done-when check; prints every
//! tool call, the screenshot count and the transcript summary, then quits.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use deskhand_core::brain::config::{ALLOWED_TOOLS_RULE, write_hook_settings, write_mcp_config};
use deskhand_core::brain::transcript::{find_transcript, wait_for_last_turn};
use deskhand_core::brain::turn::{TurnEnd, drive_turn};
use deskhand_core::brain::{ClaudeSession, SpawnOptions, claude_version, find_claude};
use deskhand_core::server::ServerState;

pub struct TurnArgs {
    pub prompt: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub timeout: Duration,
    pub accept_trust: bool,
    pub cwd: Option<PathBuf>,
}

pub struct ServerUrls {
    pub mcp: String,
    pub stop_hook: String,
    pub token: String,
}

pub fn run(state: Arc<ServerState>, urls: ServerUrls, args: TurnArgs) -> Result<bool> {
    let cwd = match args.cwd {
        Some(c) => c,
        None => dirs_home()?.join("deskhand").join(".spike-cwd"),
    };
    std::fs::create_dir_all(&cwd)?;
    let bin = find_claude().context("claude CLI not found (set DESKHAND_CLAUDE_BIN)")?;
    println!("test-turn: claude {} ({})", bin.display(), claude_version(&bin).unwrap_or_default());

    let mut opts = SpawnOptions::new(bin, cwd);
    opts.settings = Some(write_hook_settings(&urls.stop_hook, &urls.token)?);
    opts.mcp_config = Some(write_mcp_config(&urls.mcp, &urls.token)?);
    opts.allowed_tools = vec![ALLOWED_TOOLS_RULE.into()];
    opts.model = args.model;
    opts.permission_mode = args.permission_mode;
    println!("test-turn: args {}", opts.args().join(" "));

    let t0 = Instant::now();
    let mut session = ClaudeSession::spawn(&opts)?;
    println!("test-turn: spawned interactive claude pid {:?}, session {}", session.pid(), session.session_id);
    session.wait_ready(Duration::from_secs(60), args.accept_trust)?;
    println!("test-turn: TUI ready after {:.1}s; prompt: {}", t0.elapsed().as_secs_f64(), args.prompt);

    let t1 = Instant::now();
    let mut tools: Vec<String> = vec![];
    let mut shots = 0;
    let end = drive_turn(&mut session, &state, &args.prompt, args.timeout, &mut |e| {
        println!("tool: {} {} -> {} [{}]", e.tool, e.args, e.outcome.replace('\n', " "), if e.ok { "ok" } else { "error" });
        tools.push(e.tool.clone());
        if e.tool == "screenshot" {
            shots += 1;
        }
    })?;
    let secs = t1.elapsed().as_secs_f64();
    let mut ok = false;
    match end {
        TurnEnd::Stopped(ev) => {
            println!("test-turn: Stop hook after {secs:.1}s");
            println!("test-turn: final message: {:?}", ev.last_assistant_message);
            let path = ev.transcript_path.clone().map(PathBuf::from).or_else(|| find_transcript(&ev.session_id));
            if let Some(path) = path {
                if let Ok((turn, matched)) =
                    wait_for_last_turn(&path, ev.last_assistant_message.as_deref(), Duration::from_secs(10))
                {
                    println!("test-turn: transcript tool calls: {:?}", turn.tool_uses);
                    println!("test-turn: hook and transcript agree: {matched}");
                }
            }
            ok = true;
        }
        TurnEnd::Interrupted => println!("test-turn: interrupted by Esc after {secs:.1}s (claude turn cancelled)"),
        TurnEnd::PermissionPrompt(tail) => {
            println!("test-turn: claude showed a PERMISSION PROMPT; allow rule or mode did not behave. Screen:\n{tail}")
        }
        TurnEnd::Exited(tail) => println!("test-turn: claude exited early:\n{tail}"),
        TurnEnd::Timeout(tail) => println!("test-turn: TIMEOUT after {secs:.1}s:\n{tail}"),
    }
    println!("test-turn: activity sequence: {}", tools.join(", "));
    println!("test-turn: screenshots taken: {shots}");
    let _ = session.interrupt();
    std::thread::sleep(Duration::from_millis(300));
    let pid = session.pid();
    session.exit(Duration::from_secs(8))?;
    let alive = pid.map(|p| {
        std::process::Command::new("kill").args(["-0", &p.to_string()]).status().map(|s| s.success()).unwrap_or(false)
    });
    println!("test-turn: child claude still running after /exit: {alive:?}");
    Ok(ok)
}

fn dirs_home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME not set")
}
