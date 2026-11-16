//! Pindot's own loop against Pindot's real MCP server (mock platform) and
//! a fake Chat Completions server: tools listed and called with the
//! session header, the approval gate, whole turns, the model test.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pindot_core::brain::native::mcp_client::McpClient;
use pindot_core::platform::mock::{MockAccessibility, MockCapture, MockInput, MockNode, MockOverlay, MockStopKey};
use pindot_core::platform::{AppInfo, Rect, WindowInfo};
use pindot_core::server::activity::ActivityLog;
use pindot_core::server::approval::{ApprovalRequest, Approver};
use pindot_core::server::{RunningServer, ServerDeps, ServerState, start};
use tokio::sync::oneshot;

const TOKEN: &str = "test-token-0123456789abcdef";

struct Deny;
impl Approver for Deny {
    fn ask(&self, _req: ApprovalRequest, reply: oneshot::Sender<bool>) {
        let _ = reply.send(false);
    }
    fn cancel(&self, _id: &str) {}
}

struct Pindot {
    rt: tokio::runtime::Runtime,
    server: RunningServer,
    state: Arc<ServerState>,
}

/// TextEdit (pid 10) with one window and a "New Document" button; only
/// TextEdit is auto-approved, any other app is denied.
fn pindot() -> Pindot {
    let ax = MockAccessibility::default();
    *ax.apps.lock().unwrap() = vec![AppInfo { pid: 10, name: "TextEdit".into(), bundle_id: Some("com.apple.TextEdit".into()) }];
    *ax.windows.lock().unwrap() = vec![WindowInfo {
        id: 100,
        pid: 10,
        app: "TextEdit".into(),
        title: "TextEdit window".into(),
        bounds: Rect::new(100.0, 100.0, 800.0, 600.0),
        layer: 0,
        alpha: 1.0,
    }];
    *ax.frontmost.lock().unwrap() = Some(10);
    ax.trees.lock().unwrap().insert(
        100,
        vec![
            MockNode::new(1, 0, "AXWindow", "Untitled", Some(Rect::new(100.0, 100.0, 800.0, 600.0))),
            MockNode::new(2, 1, "AXButton", "New Document", Some(Rect::new(200.0, 200.0, 90.0, 20.0))),
        ],
    );
    let vault_dir = std::env::temp_dir().join(format!("pindot-loop-test-{}", uuid::Uuid::new_v4()));
    let state = ServerState::new(ServerDeps {
        capture: Arc::new(MockCapture::default()),
        input: Arc::new(MockInput::default()),
        accessibility: Arc::new(ax),
        stop_key: Arc::new(MockStopKey::default()),
        overlay: Arc::new(MockOverlay::default()),
        approver: Arc::new(Deny),
        activity: Arc::new(ActivityLog::memory()),
        vault: Arc::new(pindot_core::vault::Vault::new(vault_dir, Arc::new(pindot_core::vault::keys::MemoryKeys::default()))),
        test_auto_approve: vec!["TextEdit".into()],
        own_pid: 99,
        idle: Duration::from_millis(50),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(start(state.clone(), TOKEN, 0)).unwrap();
    Pindot { rt, server, state }
}

#[test]
fn tools_are_listed_and_called_in_the_loops_own_session() {
    let p = pindot();
    let mcp = McpClient::new(&p.server.mcp_url(), TOKEN, "loop-session-1");
    let tools = mcp.list_tools().unwrap();
    assert!(tools.iter().any(|t| t.name == "desktop_context"), "{:?}", tools.iter().map(|t| &t.name).collect::<Vec<_>>());
    assert!(tools.iter().all(|t| t.parameters.get("$schema").is_none()));
    let out = mcp.call("request_access", r#"{"apps":["TextEdit"],"reason":"test"}"#);
    assert!(!out.is_error, "{}", out.text);
    let ctx = mcp.call("desktop_context", "{}");
    assert!(ctx.text.contains("Approved for this session: TextEdit"), "{}", ctx.text);
    // Approvals belong to this session only.
    let other = McpClient::new(&p.server.mcp_url(), TOKEN, "loop-session-2");
    assert!(!other.call("desktop_context", "{}").text.contains("Approved for this session: TextEdit"));
    let bad = mcp.call("desktop_context", "not json");
    assert!(bad.is_error && bad.text.contains("not valid JSON"), "{}", bad.text);
    let denied = mcp.call("request_access", r#"{"apps":["Finder"],"reason":"test"}"#);
    assert!(denied.is_error || !denied.text.contains("approved"), "{}", denied.text);
    mcp.close();
    other.close();
    p.state.end_session("loop-session-1");
    p.state.end_session("loop-session-2");
}
