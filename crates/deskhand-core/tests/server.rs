//! In-process tests of the MCP server and hook receiver with mock platform
//! implementations: auth, Origin rejection, the MCP handshake, and the
//! screenshot -> click flow including the pixel -> point mapping.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use deskhand_core::platform::mock::{MockCapture, MockInput, MockOverlay};
use deskhand_core::platform::{MouseButton, Point};
use deskhand_core::server::{ServerState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

struct Harness {
    app: axum::Router,
    input: Arc<MockInput>,
    overlay: Arc<MockOverlay>,
    state: Arc<ServerState>,
}

fn harness() -> Harness {
    let input = Arc::new(MockInput::default());
    let overlay = Arc::new(MockOverlay::default());
    let state = ServerState::new(Arc::new(MockCapture::default()), input.clone(), overlay.clone());
    Harness { app: router(state.clone(), TOKEN), input, overlay, state }
}

fn req(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(path).header("host", "127.0.0.1:47823")
}

/// Pull the JSON-RPC message out of a JSON or SSE response body.
fn rpc_body(bytes: &[u8]) -> Value {
    let s = String::from_utf8_lossy(bytes);
    if let Ok(v) = serde_json::from_str::<Value>(&s) {
        return v;
    }
    s.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v.get("id").is_some())
        .unwrap_or(Value::Null)
}

async fn mcp(app: &axum::Router, session: Option<&str>, body: Value) -> (StatusCode, Option<String>, Value) {
    let mut b = req("POST", "/mcp")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(s) = session {
        b = b.header("mcp-session-id", s).header("mcp-protocol-version", "2025-06-18");
    }
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, sid, rpc_body(&bytes))
}

#[tokio::test]
async fn rejects_missing_token_wrong_token_and_origin() {
    let h = harness();
    let r = h.app.clone().oneshot(req("GET", "/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = h
        .app
        .clone()
        .oneshot(req("GET", "/health").header("authorization", "Bearer nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = h
        .app
        .clone()
        .oneshot(
            req("POST", "/mcp")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let r = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("host", "evil.example")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let r = h
        .app
        .clone()
        .oneshot(req("GET", "/health").header("authorization", format!("Bearer {TOKEN}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn stop_hook_is_broadcast() {
    let h = harness();
    let mut rx = h.state.stop_events.subscribe();
    let body = json!({
        "session_id": "abc", "transcript_path": "/tmp/x.jsonl", "cwd": "/tmp",
        "hook_event_name": "Stop", "stop_hook_active": false,
        "last_assistant_message": "hello", "background_tasks": []
    });
    let r = h
        .app
        .clone()
        .oneshot(
            req("POST", "/hooks/stop")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(to_bytes(r.into_body(), 1024).await.unwrap().is_empty(), "hook reply must be an empty 2xx");
    let ev = rx.try_recv().unwrap();
    assert_eq!(ev.session_id, "abc");
    assert_eq!(ev.last_assistant_message.as_deref(), Some("hello"));
}

#[tokio::test]
async fn mcp_screenshot_then_click_maps_pixels_to_points() {
    let h = harness();
    let (st, sid, init) = mcp(
        &h.app,
        None,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"test","version":"0"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "initialize: {init}");
    assert_eq!(init.pointer("/result/serverInfo/name").and_then(Value::as_str), Some("deskhand"));
    let sid = sid.expect("session id");

    let (st, _, _) = mcp(&h.app, Some(&sid), json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    assert!(st.is_success());

    let (_, _, list) = mcp(&h.app, Some(&sid), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let mut names: Vec<&str> = list
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    names.sort();
    assert_eq!(names, vec!["click", "screenshot"]);

    // Click before any screenshot is refused.
    let (_, _, early) = mcp(
        &h.app,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"click","arguments":{"x":1,"y":1}}}),
    )
    .await;
    assert_eq!(early.pointer("/result/isError"), Some(&Value::Bool(true)));

    let (_, _, shot) = mcp(
        &h.app,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"screenshot","arguments":{}}}),
    )
    .await;
    let content = shot.pointer("/result/content").and_then(Value::as_array).expect("content");
    assert_eq!(content[0].get("type").and_then(Value::as_str), Some("image"));
    assert_eq!(content[0].get("mimeType").and_then(Value::as_str), Some("image/jpeg"));
    // Mock display: 1440x900 pt at 2x -> 2880x1800 px -> sent at 1280x800.
    assert!(content[1].get("text").and_then(Value::as_str).unwrap().contains("1280x800"));

    let (_, _, click) = mcp(
        &h.app,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"click","arguments":{"x":640,"y":400}}}),
    )
    .await;
    assert_ne!(click.pointer("/result/isError"), Some(&Value::Bool(true)), "{click}");
    let clicks = h.input.clicks.lock().unwrap().clone();
    assert_eq!(clicks, vec![(Point::new(720.0, 450.0), MouseButton::Left, 1)]);
    // The overlay cursor moved to the same point before the click.
    assert_eq!(h.overlay.moves.lock().unwrap().clone(), vec![Point::new(720.0, 450.0)]);
}
