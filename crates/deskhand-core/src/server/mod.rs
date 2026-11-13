//! The local server Claude Code talks to.
//!
//! One loopback HTTP listener serves two things:
//! - `/mcp`: the MCP server (rmcp, streamable HTTP) with the desktop tools.
//! - `/hooks/stop`: receiver for Claude Code's `Stop` HTTP hook, which is how
//!   the brain learns that an interactive turn finished.
//!
//! Every request must carry `Authorization: Bearer <token>` and must NOT
//! carry an `Origin` header (browsers always send one, so this blocks web
//! pages from reaching the server). rmcp additionally validates `Host`.

pub mod tools;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

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

use crate::coords::ShotGeometry;
use crate::paths::ct_eq;
use crate::platform::{Capture, Input, Overlay};

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

/// One desktop tool call, for logs and the app's activity view.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallEvent {
    pub tool: String,
    pub detail: String,
}

pub struct ServerState {
    pub capture: Arc<dyn Capture>,
    pub input: Arc<dyn Input>,
    pub overlay: Arc<dyn Overlay>,
    pub last_shot: Mutex<Option<ShotGeometry>>,
    pub stop_events: broadcast::Sender<StopHookEvent>,
    pub tool_events: broadcast::Sender<ToolCallEvent>,
}

impl ServerState {
    pub fn new(capture: Arc<dyn Capture>, input: Arc<dyn Input>, overlay: Arc<dyn Overlay>) -> Arc<Self> {
        Arc::new(Self {
            capture,
            input,
            overlay,
            last_shot: Mutex::new(None),
            stop_events: broadcast::channel(64).0,
            tool_events: broadcast::channel(256).0,
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
    let _ = state.stop_events.send(ev);
    // 2xx with an empty body = "success, no decision" for Claude Code.
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
