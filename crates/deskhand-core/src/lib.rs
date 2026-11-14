//! Deskhand core: the OS traits, the local MCP server that gives Claude Code
//! eyes and hands, and the "brain" that drives the user's own interactive
//! `claude` CLI inside a pseudo-terminal.

pub mod brain;
pub mod coords;
pub mod paths;
pub mod platform;
pub mod server;
pub mod voice;

/// Default loopback port for the MCP server and hook receiver.
/// Override with the `DESKHAND_PORT` environment variable.
pub const DEFAULT_PORT: u16 = 47823;

pub fn port_from_env() -> u16 {
    std::env::var("DESKHAND_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Install a tracing subscriber that respects `RUST_LOG` (default `info`,
/// with the speech engines' chatty start-up logs at `warn`).
pub fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,transcribe_rs=warn,ort=warn")),
        )
        .with_target(false)
        .try_init();
}
