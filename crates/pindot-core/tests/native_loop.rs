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

// ------------------------------------------------------ a fake model server

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

/// A fake OpenAI-compatible server: `/v1/models`, Ollama's `/api/show`, and
/// `/v1/chat/completions` playing scripted SSE bodies in order (the last
/// one repeats). Records every chat request body.
struct FakeModel {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

fn fake_model(rt: &tokio::runtime::Runtime, replies: Vec<String>) -> FakeModel {
    #[derive(Clone)]
    struct S(Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<Value>>>);
    let requests = Arc::new(Mutex::new(vec![]));
    let s = S(Arc::new(Mutex::new(replies)), requests.clone());
    let app = Router::new()
        .route("/v1/models", get(|| async { Json(json!({"data": [{"id": "tiny"}, {"id": "nomic-embed"}]})) }))
        .route(
            "/api/show",
            post(|Json(b): Json<Value>| async move {
                let caps = if b["model"] == "nomic-embed" { json!(["embedding"]) } else { json!(["completion", "tools", "vision"]) };
                Json(json!({"capabilities": caps, "model_info": {"qwen3.context_length": 40960}}))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|State(s): State<S>, Json(b): Json<Value>| async move {
                s.1.lock().unwrap().push(b);
                let mut q = s.0.lock().unwrap();
                let body = if q.len() > 1 { q.remove(0) } else { q[0].clone() };
                ([("content-type", "text/event-stream")], body)
            }),
        )
        .with_state(s);
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    FakeModel { base_url: format!("http://{addr}/v1"), requests }
}

/// An SSE body: text, then tool calls (id, name, JSON arguments), then [DONE].
fn sse(text: &str, calls: &[(&str, &str, &str)]) -> String {
    let mut out = String::new();
    if !text.is_empty() {
        out += &format!("data: {}\n\n", json!({"choices": [{"delta": {"content": text}}]}));
    }
    for (i, (id, name, args)) in calls.iter().enumerate() {
        let delta = json!({"tool_calls": [{"index": i, "id": id, "function": {"name": name, "arguments": args}}]});
        out += &format!("data: {}\n\n", json!({"choices": [{"delta": delta}]}));
    }
    out + "data: [DONE]\n\n"
}

#[test]
fn models_and_what_they_can_do_are_read_from_ollama() {
    use pindot_core::brain::native::probe;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fake = fake_model(&rt, vec![sse("hi", &[])]);
    let models = probe::list_models("ollama", &fake.base_url, None).unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["tiny"], "embedding models are left out");
    assert_eq!((models[0].vision, models[0].tools, models[0].context), (Some(true), Some(true), Some(40960)));
    let unknown = probe::list_models("custom", &fake.base_url, None).unwrap();
    assert_eq!(unknown.len(), 2);
    assert_eq!(unknown[0].vision, None);
    assert!(probe::list_models("ollama", "http://127.0.0.1:9/v1", None).is_err());
    assert!(fake.requests.lock().unwrap().is_empty());
}
