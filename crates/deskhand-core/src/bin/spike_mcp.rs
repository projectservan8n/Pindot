//! Spike (c), server half: run the desktop MCP server (screenshot + click)
//! on 127.0.0.1 with the real macOS capture and input, and write the config
//! files a normal `claude` session needs to use it.
//!
//!   spike_mcp [--port N] [--seconds N]
//!
//! Then, in another terminal, from a scratch folder:
//!   claude --mcp-config "$HOME/Library/Application Support/Deskhand/claude-mcp.json" \
//!          --strict-mcp-config --allowedTools mcp__desktop
//! and ask it to take a screenshot and click something.

use std::sync::Arc;
use std::time::Duration;

use deskhand_core::brain::config::{write_hook_settings, write_mcp_config};
use deskhand_core::paths::{load_or_create_token, token_path};
use deskhand_core::platform::{Overlay, Point, native, permission_report};
use deskhand_core::server::{ServerState, start};

/// Prints where the AI cursor would go (the Tauri app draws it for real).
struct LogOverlay;

impl Overlay for LogOverlay {
    fn move_cursor(&self, to: Point, _label: Option<String>) -> Duration {
        println!("overlay: AI cursor to ({:.0}, {:.0}) pt", to.x, to.y);
        Duration::ZERO
    }
    fn set_acting(&self, acting: bool) {
        println!("overlay: acting = {acting}");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    deskhand_core::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let port: u16 = opt("--port").and_then(|s| s.parse().ok()).unwrap_or_else(deskhand_core::port_from_env);
    let seconds: u64 = opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(600);

    for p in permission_report() {
        println!("permission {:?}: {}", p.permission, if p.granted { "granted" } else { "MISSING" });
    }
    let platform = native();
    let state = ServerState::new(platform.capture.clone(), platform.input.clone(), Arc::new(LogOverlay));
    let token = load_or_create_token()?;
    let server = start(state.clone(), &token, port).await?;
    let mcp_cfg = write_mcp_config(&server.mcp_url(), &token)?;
    let hook_cfg = write_hook_settings(&server.stop_hook_url(), &token)?;

    println!("MCP server: {}", server.mcp_url());
    println!("token file (0600): {}", token_path()?.display());
    println!("--mcp-config file: {}", mcp_cfg.display());
    println!("--settings file (Stop hook): {}", hook_cfg.display());
    println!("running for {seconds}s; Ctrl-C to stop");

    let mut tools = state.tool_events.subscribe();
    let mut stops = state.stop_events.subscribe();
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            Ok(ev) = tools.recv() => println!("tool call: {} {}", ev.tool, ev.detail),
            Ok(ev) = stops.recv() => println!(
                "Stop hook: session {} last message {:?}",
                ev.session_id,
                ev.last_assistant_message.unwrap_or_default()
            ),
        }
    }
    server.shutdown();
    println!("server stopped");
    Ok(())
}
