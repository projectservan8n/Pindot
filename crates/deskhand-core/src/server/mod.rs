//! The local server Claude Code talks to.
//!
//! One loopback HTTP listener serves:
//! - `/mcp`: the MCP server (rmcp, streamable HTTP) with the desktop tools.
//! - `/hooks/stop`: Claude Code's `Stop` HTTP hook (a turn finished).
//! - `/hooks/prompt`: the `UserPromptSubmit` HTTP hook (a new user turn
//!   started), which clears an Esc halt.
//!
//! Every request must carry `Authorization: Bearer <token>` and must NOT
//! carry an `Origin` header (browsers always send one, so this blocks web
//! pages from reaching the server). The Host must be loopback.

pub mod activity;
pub mod approval;
pub mod control;
pub mod session;
pub mod shot;
pub mod tools;
pub mod tree;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::paths::ct_eq;
use crate::platform::{Accessibility, Capture, Input, Overlay, StopKey};
use activity::ActivityLog;
use approval::Approver;
use control::Control;

/// What Claude Code POSTs to the Stop hook (fields we use; others ignored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookEvent {
    pub session_id: String,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
    #[serde(default)]
    pub last_assistant_message: Option<String>,
}

/// Everything the server needs from the outside.
pub struct ServerDeps {
    pub capture: Arc<dyn Capture>,
    pub input: Arc<dyn Input>,
    pub accessibility: Arc<dyn Accessibility>,
    pub stop_key: Arc<dyn StopKey>,
    pub overlay: Arc<dyn Overlay>,
    /// Who answers `request_access` (the app's approval window).
    pub approver: Arc<dyn Approver>,
    pub activity: Arc<ActivityLog>,
    /// TEST-ONLY apps approved without asking (`DESKHAND_TEST_AUTO_APPROVE`).
    /// Empty in normal use.
    pub test_auto_approve: Vec<String>,
    /// Deskhand's own process: never controllable, never in screenshots.
    pub own_pid: i32,
    /// How long after the last action the "controlling input" state lasts.
    pub idle: Duration,
}

impl ServerDeps {
    /// The real platform for this OS. Reads `DESKHAND_TEST_AUTO_APPROVE`
    /// (and logs loudly if it is set).
    pub fn native(overlay: Arc<dyn Overlay>, approver: Arc<dyn Approver>, activity: Arc<ActivityLog>) -> Self {
        let p = crate::platform::native();
        Self {
            capture: p.capture,
            input: p.input,
            accessibility: p.accessibility,
            stop_key: p.stop_key,
            overlay,
            approver,
            activity,
            test_auto_approve: approval::auto_approve_from_env(),
            own_pid: std::process::id() as i32,
            idle: control::DEFAULT_IDLE,
        }
    }
}

pub struct ServerState {
    pub capture: Arc<dyn Capture>,
    pub input: Arc<dyn Input>,
    pub accessibility: Arc<dyn Accessibility>,
    pub overlay: Arc<dyn Overlay>,
    pub approver: Arc<dyn Approver>,
    pub activity: Arc<ActivityLog>,
    pub control: Arc<Control>,
    pub auto_approve: Vec<String>,
    pub own_pid: i32,
    pub stop_events: broadcast::Sender<StopHookEvent>,
}

impl ServerState {
    pub fn new(deps: ServerDeps) -> Arc<Self> {
        let control = Control::new(deps.stop_key, deps.overlay.clone(), deps.activity.clone(), deps.idle);
        Arc::new(Self {
            capture: deps.capture,
            input: deps.input,
            accessibility: deps.accessibility,
            overlay: deps.overlay,
            approver: deps.approver,
            activity: deps.activity,
            control,
            auto_approve: deps.test_auto_approve,
            own_pid: deps.own_pid,
            stop_events: broadcast::channel(64).0,
        })
    }
}

async fn guard(State(token): State<Arc<str>>, req: Request, next: Next) -> Response {
    let h = req.headers();
    if h.contains_key(header::ORIGIN) {
        return (StatusCode::FORBIDDEN, "requests with an Origin header are not allowed").into_response();
    }
    let host_ok = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| {
            let name = host.rsplit_once(':').map(|(n, _)| n).unwrap_or(host);
            matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        })
        .unwrap_or(false);
    if !host_ok {
        return (StatusCode::FORBIDDEN, "bad Host").into_response();
    }
    let authed = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| ct_eq(t.trim(), &token))
        .unwrap_or(false);
    if !authed {
        return (StatusCode::UNAUTHORIZED, "missing or wrong bearer token").into_response();
    }
    next.run(req).await
}

async fn stop_hook(State(state): State<Arc<ServerState>>, Json(ev): Json<StopHookEvent>) -> StatusCode {
    tracing::info!(session = %ev.session_id, "Stop hook received");
    state.control.turn_ended();
    let _ = state.stop_events.send(ev);
    // 2xx with an empty body = "success, no decision" for Claude Code.
    StatusCode::OK
}

async fn prompt_hook(State(state): State<Arc<ServerState>>, Json(_ev): Json<serde_json::Value>) -> StatusCode {
    // A new user turn: an Esc halt from the previous turn no longer applies.
    tracing::info!("UserPromptSubmit hook received");
    state.control.turn_ended();
    StatusCode::OK
}

pub fn router(state: Arc<ServerState>, token: &str) -> Router {
    let mcp_state = state.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(tools::DesktopServer::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        // Loopback-only Host check is rmcp's default; also reject any Origin.
        StreamableHttpServerConfig::default().enforce_origin_validation(),
    );
    Router::new()
        .nest_service("/mcp", mcp)
        .route("/hooks/stop", post(stop_hook))
        .route("/hooks/prompt", post(prompt_hook))
        .route("/health", get(|| async { "ok" }))
        .with_state(state)
        .layer(middleware::from_fn_with_state(Arc::<str>::from(token), guard))
}

pub struct RunningServer {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    pub fn mcp_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.addr.port())
    }
    pub fn stop_hook_url(&self) -> String {
        format!("http://127.0.0.1:{}/hooks/stop", self.addr.port())
    }
    pub fn prompt_hook_url(&self) -> String {
        format!("http://127.0.0.1:{}/hooks/prompt", self.addr.port())
    }
    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// Bind 127.0.0.1:`port` (0 = any free port) and serve in the background.
pub async fn start(state: Arc<ServerState>, token: &str, port: u16) -> Result<RunningServer> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .with_context(|| format!("binding 127.0.0.1:{port}"))?;
    let addr = listener.local_addr()?;
    let app = router(state, token);
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("server stopped: {e}");
        }
    });
    tracing::info!("Deskhand server on http://{addr} (MCP at /mcp)");
    Ok(RunningServer { addr, task })
}
