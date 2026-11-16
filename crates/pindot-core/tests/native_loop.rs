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

// ------------------------------------------------------------ LoopBrain

#[test]
fn a_spoken_request_runs_through_the_loop_and_pindots_tools() {
    use pindot_core::brain::Brain;
    use pindot_core::brain::native::settings::{BrainKind, BrainSettings, Vision};
    use pindot_core::brain::native::{LoopBrain, LoopConfig};
    use pindot_core::voice::machine::Input;

    let p = pindot();
    let fake = fake_model(
        &p.rt,
        vec![
            sse("Opening TextEdit.", &[("c1", "request_access", r#"{"apps":["TextEdit"],"reason":"asked"}"#)]),
            sse("", &[("c2", "desktop_context", "{}")]),
            sse("TextEdit is ready.", &[]),
        ],
    );
    let data = std::env::temp_dir().join(format!("pindot-loopbrain-{}", uuid::Uuid::new_v4()));
    let (tx, rx) = std::sync::mpsc::channel::<Input>();
    let tx = Mutex::new(tx);
    let brain = LoopBrain::start(
        LoopConfig {
            brain: BrainSettings {
                kind: BrainKind::Loop,
                provider: "custom".into(),
                base_url: fake.base_url.clone(),
                model: "tiny".into(),
                vision: Vision::Off,
                ..BrainSettings::default()
            },
            key: None,
            mcp_url: p.server.mcp_url(),
            token: TOKEN.into(),
            system_prompt: "Be brief.".into(),
            data_dir: data.clone(),
        },
        p.state.clone(),
        Arc::new(move |i| {
            let _ = tx.lock().unwrap().send(i);
        }),
    )
    .unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(10)).expect("an input from the brain");
    let Input::SessionReady { session_id } = next() else { panic!("not ready") };
    brain.send("open text edit");
    let mut got = vec![];
    loop {
        let i = next();
        let end = matches!(i, Input::Stop { .. } | Input::TurnFailed { .. });
        got.push(i);
        if end {
            break;
        }
    }
    let sid = session_id.clone();
    assert_eq!(got[0], Input::TranscriptPrompt { session_id: sid.clone() }, "{got:?}");
    assert!(got.contains(&Input::TranscriptText { session_id: sid.clone(), text: "Opening TextEdit.".into() }));
    assert!(got.contains(&Input::TranscriptToolUse { session_id: sid.clone(), name: "mcp__desktop__request_access".into() }));
    assert_eq!(got.last(), Some(&Input::Stop { session_id: sid.clone(), last_message: Some("TextEdit is ready.".into()) }));
    let reqs = fake.requests.lock().unwrap().clone();
    let names: Vec<&str> = reqs[0]["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(names.contains(&"click") && !names.contains(&"screenshot"), "no screenshot tool without vision: {names:?}");
    assert!(reqs[2]["messages"].to_string().contains("Approved for this session: TextEdit"), "the tool result reached the model");
    assert_eq!(reqs[0]["messages"][0]["role"], "system");
    let file = pindot_core::brain::transcript::conversations_dir(&data).join(format!("{sid}.jsonl"));
    let items = pindot_core::chat::transcript::read_all(&file);
    assert!(items.len() >= 4, "{items:?}");
    drop(brain);
    std::fs::remove_dir_all(&data).ok();
}

#[test]
fn a_server_that_is_not_running_is_reported_once_and_a_request_fails_plainly() {
    use pindot_core::brain::Brain;
    use pindot_core::brain::native::settings::{BrainKind, BrainSettings};
    use pindot_core::brain::native::{LoopBrain, LoopConfig};
    use pindot_core::voice::machine::Input;

    let p = pindot();
    let (tx, rx) = std::sync::mpsc::channel::<Input>();
    let tx = Mutex::new(tx);
    let brain = LoopBrain::start(
        LoopConfig {
            brain: BrainSettings { kind: BrainKind::Loop, provider: "ollama".into(), base_url: "http://127.0.0.1:9/v1".into(), model: "x".into(), ..BrainSettings::default() },
            key: None,
            mcp_url: p.server.mcp_url(),
            token: TOKEN.into(),
            system_prompt: String::new(),
            data_dir: std::env::temp_dir(),
        },
        p.state.clone(),
        Arc::new(move |i| {
            let _ = tx.lock().unwrap().send(i);
        }),
    )
    .unwrap();
    let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(first, Input::BrainUnavailable { message: "Ollama isn't running. Start it, or pick another brain in Settings.".into() });
    brain.send("hello");
    let second = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(second, Input::TurnFailed { message } if message.contains("isn't running")));
}

// ------------------------------------------------------------ model test

#[test]
fn the_model_test_passes_a_model_that_uses_tools_and_explains_one_that_does_not() {
    use pindot_core::brain::native::settings::{BrainKind, BrainSettings};
    use pindot_core::brain::native::test_model::test_model;
    let p = pindot();
    let brain = |url: &str| BrainSettings {
        kind: BrainKind::Loop,
        provider: "custom".into(),
        base_url: url.into(),
        model: "tiny".into(),
        ..BrainSettings::default()
    };
    let good = fake_model(&p.rt, vec![sse("ready", &[]), sse("", &[("c1", "desktop_context", "{}")]), sse("There is 1 display.", &[])]);
    let r = test_model(&brain(&good.base_url), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(r.ok, "{}", r.note);
    let no_tools = fake_model(&p.rt, vec![sse("Ready.", &[]), sse("I can't do that.", &[])]);
    let r = test_model(&brain(&no_tools.base_url), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(!r.ok);
    assert_eq!(r.note, "It answered but didn't call the tool: this model may not support tools.");
    let wrong = fake_model(&p.rt, vec![sse("Hello there!", &[])]);
    let r = test_model(&brain(&wrong.base_url), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(!r.ok && r.note.starts_with("It didn't answer as asked"), "{}", r.note);
    let down = test_model(&brain("http://127.0.0.1:9/v1"), None, &p.server.mcp_url(), TOKEN, &p.state);
    assert!(!down.ok && down.note.contains("Can't reach"), "{}", down.note);
}
