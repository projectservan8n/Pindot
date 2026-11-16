//! In-process tests of the MCP server and hook receiver with mock platform
//! implementations: auth and Origin checks, the approval gate, the one-
//! session input lock, the Esc stop, refs from the accessibility tree,
//! pagination, masking, and the pixel -> point mapping.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use pindot_core::platform::mock::{
    InputAction, MockAccessibility, MockCapture, MockInput, MockNode, MockOverlay, MockStopKey, MockTarget,
};
use pindot_core::platform::{AppInfo, CursorAction, KeyCombo, MouseButton, Point, Rect, WindowInfo};
use pindot_core::server::activity::ActivityLog;
use pindot_core::server::approval::{ApprovalRequest, Approver};
use pindot_core::server::control::ControlEvent;
use pindot_core::server::{ServerDeps, ServerState, router};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";
const OWN_PID: i32 = 99;
const TEXTEDIT: i32 = 10;
const CHROME: i32 = 20;

/// Answers approval prompts with a scripted value and records them.
#[derive(Default)]
struct ScriptedApprover {
    answer: Mutex<bool>,
    asked: Mutex<Vec<ApprovalRequest>>,
    /// Press Esc while the prompt is open (tests the stop during a fill).
    esc: Mutex<Option<Arc<MockStopKey>>>,
}

impl Approver for ScriptedApprover {
    fn ask(&self, req: ApprovalRequest, reply: oneshot::Sender<bool>) {
        self.asked.lock().unwrap().push(req);
        if let Some(k) = self.esc.lock().unwrap().as_ref() {
            k.press();
        }
        let _ = reply.send(*self.answer.lock().unwrap());
    }
    fn cancel(&self, _id: &str) {}
}

struct Harness {
    app: axum::Router,
    input: Arc<MockInput>,
    overlay: Arc<MockOverlay>,
    ax: Arc<MockAccessibility>,
    stop: Arc<MockStopKey>,
    approver: Arc<ScriptedApprover>,
    activity: Arc<ActivityLog>,
    state: Arc<ServerState>,
}

fn win(id: u64, pid: i32, app: &str, b: Rect) -> WindowInfo {
    WindowInfo { id, pid, app: app.into(), title: format!("{app} window"), bounds: b, layer: 0, alpha: 1.0 }
}

/// Display 1440x900 pt at 2x (screenshots 1280x800). TextEdit window 100 at
/// (100,100 800x600) with Chrome window 200 in front of its right edge at
/// (800,100 500x400).
fn fixture() -> MockAccessibility {
    let ax = MockAccessibility::default();
    *ax.apps.lock().unwrap() = vec![
        AppInfo { pid: TEXTEDIT, name: "TextEdit".into(), bundle_id: Some("com.apple.TextEdit".into()) },
        AppInfo { pid: CHROME, name: "Google Chrome".into(), bundle_id: Some("com.google.Chrome".into()) },
        AppInfo { pid: OWN_PID, name: "Pindot".into(), bundle_id: Some("dev.pindot.app".into()) },
    ];
    *ax.windows.lock().unwrap() = vec![
        win(200, CHROME, "Google Chrome", Rect::new(800.0, 100.0, 500.0, 400.0)),
        win(100, TEXTEDIT, "TextEdit", Rect::new(100.0, 100.0, 800.0, 600.0)),
    ];
    *ax.frontmost.lock().unwrap() = Some(TEXTEDIT);
    let mut item = MockNode::new(5, 1, "AXMenuItem", "New", None);
    item.menu_path = Some("File > New".into());
    item.menu_root = Some(2); // the "File" menu bar item below
    ax.trees.lock().unwrap().insert(
        100,
        vec![
            MockNode::new(1, 0, "AXWindow", "Untitled", Some(Rect::new(100.0, 100.0, 800.0, 600.0))),
            MockNode::new(2, 1, "AXButton", "New Document", Some(Rect::new(200.0, 200.0, 90.0, 20.0))),
            MockNode::new(3, 0, "AXMenuBarItem", "File", Some(Rect::new(50.0, 0.0, 40.0, 24.0))),
            MockNode::new(4, 1, "AXTextArea", "", Some(Rect::new(120.0, 250.0, 600.0, 400.0))),
            item,
        ],
    );
    ax
}

fn harness_with(ax: MockAccessibility, auto: &[&str]) -> Harness {
    let input = Arc::new(MockInput::default());
    let overlay = Arc::new(MockOverlay::default());
    let ax = Arc::new(ax);
    let stop = Arc::new(MockStopKey::default());
    let approver = Arc::new(ScriptedApprover::default());
    let activity = Arc::new(ActivityLog::memory());
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let vault_dir = std::env::temp_dir()
        .join(format!("pindot-server-test-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let vault = Arc::new(pindot_core::vault::Vault::new(vault_dir, Arc::new(pindot_core::vault::keys::MemoryKeys::default())));
    let state = ServerState::new(ServerDeps {
        capture: Arc::new(MockCapture::default()),
        input: input.clone(),
        accessibility: ax.clone(),
        stop_key: stop.clone(),
        overlay: overlay.clone(),
        approver: approver.clone(),
        activity: activity.clone(),
        vault: vault.clone(),
        test_auto_approve: auto.iter().map(|s| s.to_string()).collect(),
        own_pid: OWN_PID,
        idle: Duration::from_millis(50),
    });
    Harness { app: router(state.clone(), TOKEN), input, overlay, ax, stop, approver, activity, state }
}

fn harness() -> Harness {
    harness_with(fixture(), &[])
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

/// Initialize an MCP session; returns its id and the initialize result.
async fn open(app: &axum::Router) -> (String, Value) {
    let (st, sid, init) = mcp(
        app,
        None,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"test","version":"0"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "initialize: {init}");
    let sid = sid.expect("session id");
    let (st, _, _) = mcp(app, Some(&sid), json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    assert!(st.is_success());
    (sid, init)
}

/// Call a tool. Returns (is_error, all text content joined, raw result).
async fn call(app: &axum::Router, sid: &str, name: &str, args: Value) -> (bool, String, Value) {
    let (_, _, v) = mcp(
        app,
        Some(sid),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":name,"arguments":args}}),
    )
    .await;
    let r = v.get("result").cloned().unwrap_or_else(|| panic!("{name}: no result: {v}"));
    let err = r.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let text = r
        .get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    (err, text, r)
}

async fn ok(app: &axum::Router, sid: &str, name: &str, args: Value) -> String {
    let (err, text, _) = call(app, sid, name, args.clone()).await;
    assert!(!err, "{name}({args}) failed: {text}");
    text
}

async fn fails(app: &axum::Router, sid: &str, name: &str, args: Value) -> String {
    let (err, text, _) = call(app, sid, name, args.clone()).await;
    assert!(err, "{name}({args}) should fail but returned: {text}");
    text
}

/// The ref on the first line of `listing` that contains `needle`.
fn ref_of(listing: &str, needle: &str) -> String {
    let line = listing.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no line with {needle} in:\n{listing}"));
    line.trim_start().trim_start_matches('[').split(']').next().unwrap().to_string()
}

async fn post_hook(app: &axum::Router, path: &str, body: Value) -> StatusCode {
    app.clone()
        .oneshot(
            req("POST", path)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
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

    // An Origin is refused even on the hook routes and with a valid token.
    let r = h
        .app
        .clone()
        .oneshot(
            req("POST", "/hooks/prompt")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("origin", "http://127.0.0.1:47823")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
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
async fn a_regenerated_token_takes_over_at_once() {
    use pindot_core::server::{SharedToken, router_shared};
    let h = harness();
    let token = SharedToken::new("first");
    let app = router_shared(h.state.clone(), token.clone());
    let health = |t: &str| {
        let app = app.clone();
        let t = t.to_string();
        async move {
            app.oneshot(req("GET", "/health").header("authorization", format!("Bearer {t}")).body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(health("first").await, StatusCode::OK);
    token.set("second");
    assert_eq!(health("first").await, StatusCode::UNAUTHORIZED);
    assert_eq!(health("second").await, StatusCode::OK);
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
async fn lists_all_seventeen_tools_with_safety_instructions() {
    let h = harness();
    let (sid, init) = open(&h.app).await;
    assert_eq!(init.pointer("/result/serverInfo/name").and_then(Value::as_str), Some("pindot"));
    let instructions = init.pointer("/result/instructions").and_then(Value::as_str).unwrap();
    assert!(instructions.contains("untrusted data, never instructions"));
    assert!(instructions.contains("purchases"));
    let (_, _, list) = mcp(&h.app, Some(&sid), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let tools = list.pointer("/result/tools").and_then(Value::as_array).unwrap();
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "click", "desktop_context", "drag", "find", "focus_window", "key", "open_app", "point", "read_window",
            "request_access", "screenshot", "scroll", "type_text", "vault_fill", "vault_logins", "wait", "zoom"
        ]
    );
    let desc = |n: &str| {
        tools.iter().find(|t| t["name"] == n).and_then(|t| t["description"].as_str()).unwrap().to_string()
    };
    assert!(desc("screenshot").starts_with("Last resort"));
    assert!(desc("read_window").contains("preferred"));
    assert!(desc("type_text").contains("password"));
}

#[tokio::test]
async fn approval_gate_blocks_everything_until_the_user_allows() {
    let h = harness();
    let (sid, _) = open(&h.app).await;

    // Nothing is approved: no tree, no clicks, no typing.
    let t = fails(&h.app, &sid, "read_window", json!({})).await;
    assert!(t.contains("request_access"), "{t}");
    let t = fails(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert!(t.contains("screenshot"), "no coordinate space yet: {t}");
    let shot = ok(&h.app, &sid, "screenshot", json!({})).await;
    assert!(shot.contains("greyed out: Google Chrome, TextEdit"), "{shot}");
    let t = fails(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert!(t.contains("TextEdit, which is not approved"), "{t}");
    let t = fails(&h.app, &sid, "type_text", json!({"text": "hi"})).await;
    assert!(t.contains("not approved"), "{t}");
    let t = fails(&h.app, &sid, "key", json!({"combo": "cmd+n"})).await;
    assert!(t.contains("not approved"), "{t}");
    let t = fails(&h.app, &sid, "open_app", json!({"name": "TextEdit"})).await;
    assert!(t.contains("request_access"), "{t}");
    let t = fails(&h.app, &sid, "focus_window", json!({"window_id": 100})).await;
    assert!(t.contains("not approved"), "{t}");

    // The user denies.
    let t = fails(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "write a note"})).await;
    assert!(t.contains("denied"), "{t}");
    {
        let asked = h.approver.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].apps[0].name, "TextEdit");
        assert_eq!(asked[0].reason, "write a note");
    }
    fails(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;

    // The user allows: TextEdit works, Chrome (in front of part of it) does not.
    *h.approver.answer.lock().unwrap() = true;
    let t = ok(&h.app, &sid, "request_access", json!({"apps": ["textedit"], "reason": "write a note"})).await;
    assert!(t.contains("approved for this session: TextEdit"), "{t}");
    ok(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    let t = fails(&h.app, &sid, "click", json!({"x": 756, "y": 267})).await;
    assert!(t.contains("Google Chrome, which is not approved"), "{t}");
    assert_eq!(h.input.clicks(), vec![(Point::new(400.5, 400.5), MouseButton::Left, 1)]);
    let shot = ok(&h.app, &sid, "screenshot", json!({})).await;
    assert!(shot.contains("greyed out: Google Chrome.") && !shot.contains("TextEdit."), "{shot}");

    // High-risk apps carry a warning to the user and back to Claude.
    let mut ax = fixture();
    ax.installed = vec!["Terminal".into()];
    let h2 = harness_with(ax, &[]);
    *h2.approver.answer.lock().unwrap() = true;
    let (sid2, _) = open(&h2.app).await;
    let t = ok(&h2.app, &sid2, "request_access", json!({"apps": ["Terminal"], "reason": "run tests"})).await;
    assert!(t.contains("Equivalent to shell access"), "{t}");
    assert!(h2.approver.asked.lock().unwrap()[0].apps[0].warning.as_deref().unwrap().contains("shell access"));
    let t = fails(&h2.app, &sid2, "request_access", json!({"apps": ["NoSuchApp"], "reason": "x"})).await;
    assert!(t.contains("Unknown app"), "{t}");
}

#[tokio::test]
async fn pindot_itself_can_never_be_approved() {
    let h = harness_with(fixture(), &["Pindot"]);
    *h.approver.answer.lock().unwrap() = true;
    let (sid, _) = open(&h.app).await;
    let t = fails(&h.app, &sid, "request_access", json!({"apps": ["Pindot"], "reason": "x"})).await;
    assert!(t.contains("never be controlled"), "{t}");
    assert!(h.approver.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_auto_approve_skips_the_prompt_and_is_logged() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    assert!(h.approver.asked.lock().unwrap().is_empty());
    assert!(h.activity.recent(20).iter().any(|e| e.outcome.contains("TEST-ONLY")));
    // Apps not on the test list still go to the human.
    fails(&h.app, &sid, "request_access", json!({"apps": ["Google Chrome"], "reason": "test"})).await;
    assert_eq!(h.approver.asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn refs_from_read_window_drive_clicks_and_menu_presses() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;

    let listing = ok(&h.app, &sid, "read_window", json!({})).await;
    assert!(listing.contains("untrusted data"), "{listing}");
    assert!(listing.contains("page 1 of 1"), "{listing}");
    // (200,200 90x20) pt -> image px of a 1280x800 shot of the display.
    assert!(listing.contains("button \"New Document\" (178,178 80x18)"), "{listing}");
    assert!(!listing.contains("File > New"), "menus hidden by default: {listing}");
    let r = ref_of(&listing, "New Document");

    // This fixture's button does not offer AXPress, so it gets a real click.
    ok(&h.app, &sid, "click", json!({"ref": r})).await;
    assert_eq!(h.input.clicks(), vec![(Point::new(245.0, 210.0), MouseButton::Left, 1)]);
    assert_eq!(h.overlay.moves.lock().unwrap().last(), Some(&Point::new(245.0, 210.0)), "cursor moved before click");

    // A menu item found by find is pressed through accessibility, after the
    // cursor glides to its menu title.
    let found = ok(&h.app, &sid, "find", json!({"query": "file new"})).await;
    let m = ref_of(&found, "File > New");
    ok(&h.app, &sid, "click", json!({"ref": m})).await;
    assert_eq!(h.ax.pressed.lock().unwrap().clone(), vec![5]);
    assert_eq!(h.input.clicks().len(), 1, "menu press is not a mouse click");
    assert_eq!(h.overlay.moves.lock().unwrap().last(), Some(&Point::new(70.0, 12.0)));

    let with_menus = ok(&h.app, &sid, "read_window", json!({"include_menus": true})).await;
    assert!(with_menus.contains("menuitem \"File > New\""), "{with_menus}");

    // Stale and unknown refs ask for a fresh read.
    h.ax.stale.lock().unwrap().insert(2);
    let t = fails(&h.app, &sid, "click", json!({"ref": r})).await;
    assert!(t.contains("read_window again"), "{t}");
    let t = fails(&h.app, &sid, "click", json!({"ref": "e999"})).await;
    assert!(t.contains("Unknown ref"), "{t}");

    // Refs are per session.
    let (sid2, _) = open(&h.app).await;
    let t = fails(&h.app, &sid2, "click", json!({"ref": m})).await;
    assert!(t.contains("Unknown ref"), "{t}");
}

#[tokio::test]
async fn a_pressable_ref_is_pressed_without_moving_the_users_pointer() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.pressable.lock().unwrap().insert(2);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    let listing = ok(&h.app, &sid, "read_window", json!({})).await;
    let r = ref_of(&listing, "New Document");

    let t = ok(&h.app, &sid, "click", json!({"ref": r})).await;
    assert!(t.contains("via accessibility"), "{t}");
    assert_eq!(h.ax.pressed.lock().unwrap().clone(), vec![2]);
    assert!(h.input.clicks().is_empty(), "the user's pointer must not move: {:?}", h.input.clicks());
    assert_eq!(h.overlay.moves.lock().unwrap().last(), Some(&Point::new(245.0, 210.0)), "AI cursor still glides there");
}

/// TextEdit approved and a screenshot taken, so x,y clicks are allowed.
async fn ready_for_points(h: &Harness) -> String {
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    ok(&h.app, &sid, "screenshot", json!({})).await;
    sid
}

fn target(id: u64, pid: i32, role: &str, name: &str, action: &str) -> MockTarget {
    MockTarget { id, pid, role: role.into(), name: name.into(), action: action.into() }
}

#[tokio::test]
async fn a_point_click_presses_the_control_under_it_without_moving_the_pointer() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.targets.lock().unwrap().push(target(2, TEXTEDIT, "AXButton", "New Document", "AXPress"));
    let sid = ready_for_points(&h).await;
    let t = ok(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert!(t.contains("via accessibility") && t.contains("New Document"), "{t}");
    assert_eq!(h.ax.pressed.lock().unwrap().clone(), vec![2]);
    assert!(h.input.clicks().is_empty(), "the user's pointer must not move: {:?}", h.input.clicks());
    assert_eq!(h.overlay.actions.lock().unwrap().last(), Some(&CursorAction::Press));
}

#[tokio::test]
async fn a_control_of_another_app_under_the_point_is_not_pressed() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.targets.lock().unwrap().push(target(2, CHROME, "AXButton", "Buy now", "AXPress"));
    let sid = ready_for_points(&h).await;
    ok(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert!(h.ax.pressed.lock().unwrap().is_empty());
    assert_eq!(h.input.clicks().len(), 1, "falls back to the (gated) real click");
}

#[tokio::test]
async fn a_right_click_opens_the_context_menu_through_accessibility() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.targets.lock().unwrap().push(target(2, TEXTEDIT, "AXTextArea", "Body", "AXShowMenu"));
    let sid = ready_for_points(&h).await;
    let t = ok(&h.app, &sid, "click", json!({"x": 356, "y": 356, "button": "right"})).await;
    assert!(t.contains("context menu") && t.contains("via accessibility"), "{t}");
    assert_eq!(h.ax.performed.lock().unwrap().clone(), vec![(2, "AXShowMenu".to_string())]);
    assert!(h.input.clicks().is_empty(), "{:?}", h.input.clicks());
}

#[tokio::test]
async fn a_real_click_puts_the_users_pointer_back() {
    let h = harness_with(fixture(), &["TextEdit"]);
    *h.input.pointer.lock().unwrap() = Some(Point::new(900.0, 700.0));
    let sid = ready_for_points(&h).await;
    let t = ok(&h.app, &sid, "click", json!({"x": 356, "y": 356, "count": 2})).await;
    assert!(t.contains("pointer was put back"), "{t}");
    let acts = h.input.actions.lock().unwrap().clone();
    assert_eq!(acts.last(), Some(&InputAction::Move(Point::new(900.0, 700.0))), "{acts:?}");
}

#[tokio::test]
async fn the_pointer_is_left_alone_if_the_user_moved_it_meanwhile() {
    let h = harness_with(fixture(), &["TextEdit"]);
    *h.input.pointer.lock().unwrap() = Some(Point::new(900.0, 700.0));
    *h.input.user_moves_to.lock().unwrap() = Some(Point::new(50.0, 60.0));
    let sid = ready_for_points(&h).await;
    ok(&h.app, &sid, "click", json!({"x": 356, "y": 356, "count": 2})).await;
    let acts = h.input.actions.lock().unwrap().clone();
    assert!(matches!(acts.last(), Some(InputAction::Click(..))), "no move back: {acts:?}");
}

#[tokio::test]
async fn right_and_double_clicks_on_a_pressable_ref_stay_real_clicks() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.pressable.lock().unwrap().insert(2);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    let listing = ok(&h.app, &sid, "read_window", json!({})).await;
    let r = ref_of(&listing, "New Document");

    ok(&h.app, &sid, "click", json!({"ref": r, "button": "right"})).await;
    ok(&h.app, &sid, "click", json!({"ref": r, "count": 2})).await;
    assert!(h.ax.pressed.lock().unwrap().is_empty());
    assert_eq!(
        h.input.clicks(),
        vec![(Point::new(245.0, 210.0), MouseButton::Right, 1), (Point::new(245.0, 210.0), MouseButton::Left, 2)]
    );
}

#[tokio::test]
async fn a_failed_press_is_reported_and_never_retried_as_a_real_click() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.pressable.lock().unwrap().insert(2);
    h.ax.press_fails.lock().unwrap().insert(2);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    let listing = ok(&h.app, &sid, "read_window", json!({})).await;
    let r = ref_of(&listing, "New Document");

    let t = fails(&h.app, &sid, "click", json!({"ref": r})).await;
    assert!(t.contains("read_window"), "says how to check what happened: {t}");
    assert!(h.input.clicks().is_empty(), "no real click after a failed press: {:?}", h.input.clicks());
}

#[tokio::test]
async fn the_overlay_is_told_what_each_action_did() {
    let h = harness_with(fixture(), &["TextEdit"]);
    h.ax.pressable.lock().unwrap().insert(2);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    let listing = ok(&h.app, &sid, "read_window", json!({})).await;
    let button = ref_of(&listing, "New Document");
    let area = ref_of(&listing, "textarea");

    ok(&h.app, &sid, "click", json!({"ref": button})).await;
    ok(&h.app, &sid, "click", json!({"ref": button, "button": "right"})).await;
    ok(&h.app, &sid, "scroll", json!({"ref": area, "direction": "down", "amount": 3})).await;
    ok(&h.app, &sid, "drag", json!({"from_ref": button, "to_ref": area})).await;
    ok(&h.app, &sid, "key", json!({"combo": "cmd+shift+s"})).await;
    ok(&h.app, &sid, "type_text", json!({"text": "hello"})).await;
    ok(&h.app, &sid, "point", json!({"ref": area, "label": "here"})).await;

    let actions = h.overlay.actions.lock().unwrap().clone();
    assert_eq!(actions.len(), 7, "{actions:?}");
    assert_eq!(actions[6], CursorAction::Point, "pointing gets a spotlight, not a click mark");
    assert_eq!(actions[0], CursorAction::Press);
    assert_eq!(actions[1], CursorAction::Click { button: MouseButton::Right, count: 1 });
    assert_eq!(actions[2], CursorAction::Scroll { dx: 0, dy: 3 });
    // The drag goes from the button's centre to the text area's centre.
    match &actions[3] {
        CursorAction::Drag { to, ms } => {
            assert_eq!(*to, Point::new(420.0, 450.0));
            assert!(*ms >= 200, "the overlay follows the real drag's length: {ms}");
        }
        other => panic!("expected a drag, got {other:?}"),
    }
    assert_eq!(actions[4], CursorAction::Key { keys: KeyCombo::parse("cmd+shift+s").unwrap().caps(), repeat: 1 });
    assert_eq!(actions[5], CursorAction::Type { chars: 5 }, "the overlay gets a count, never the text");
}

#[tokio::test]
async fn read_window_paginates_long_trees() {
    let ax = fixture();
    let nodes: Vec<MockNode> = (0..400)
        .map(|i| {
            let y = 110.0 + (i % 50) as f64 * 10.0;
            MockNode::new(1000 + i, 1, "AXButton", &format!("Button {i}"), Some(Rect::new(150.0, y, 50.0, 8.0)))
        })
        .collect();
    ax.trees.lock().unwrap().insert(100, nodes);
    let h = harness_with(ax, &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;

    let p1 = ok(&h.app, &sid, "read_window", json!({"window_id": 100})).await;
    assert!(p1.contains("400 elements (interactive), page 1 of 3"), "{p1}");
    assert!(p1.contains("page=2"), "{p1}");
    assert_eq!(p1.lines().filter(|l| l.starts_with('[')).count(), 150);
    let p3 = ok(&h.app, &sid, "read_window", json!({"window_id": 100, "page": 3})).await;
    assert_eq!(p3.lines().filter(|l| l.starts_with('[')).count(), 100);
    // Later pages reuse the snapshot, so refs continue rather than restart.
    assert!(p3.contains("[e301] button \"Button 300\""), "{p3}");
    assert!(p3.contains("[e400] button \"Button 399\""), "{p3}");
    assert!(!p3.contains("page=4"));
}

#[tokio::test]
async fn one_session_holds_the_input_lock() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (a, _) = open(&h.app).await;
    let (b, _) = open(&h.app).await;
    for s in [&a, &b] {
        ok(&h.app, s, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
        ok(&h.app, s, "screenshot", json!({})).await;
    }
    ok(&h.app, &a, "click", json!({"x": 356, "y": 356})).await;
    let holder = h.state.control.lock_holder().expect("A holds the lock");
    let t = fails(&h.app, &b, "click", json!({"x": 356, "y": 356})).await;
    assert!(t.contains("Another Claude session") && t.contains(&holder), "{t}");
    fails(&h.app, &b, "type_text", json!({"text": "x"})).await;
    let ctx = ok(&h.app, &b, "desktop_context", json!({})).await;
    assert!(ctx.contains("held by another session"), "{ctx}");
    // Reading is not input: B may still look.
    ok(&h.app, &b, "read_window", json!({})).await;
    assert_eq!(h.input.clicks().len(), 1);

    // When A's MCP session ends, the lock is free for B.
    let r = h
        .app
        .clone()
        .oneshot(
            req("DELETE", "/mcp")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("mcp-session-id", &a)
                .header("mcp-protocol-version", "2025-06-18")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(r.status().is_success(), "DELETE session: {}", r.status());
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.state.control.lock_holder().is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ok(&h.app, &b, "click", json!({"x": 356, "y": 356})).await;
    assert_ne!(h.state.control.lock_holder(), Some(holder));
}

/// A tool call the way Claude Code 2.1.3xx sends it: MCP 2026-07-28, no
/// initialize and no `Mcp-Session-Id`, protocol data in `_meta`.
async fn stateless_call(app: &axum::Router, key: Option<&str>, name: &str, args: Value) -> (bool, String) {
    let mut b = req("POST", "/mcp")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", name);
    if let Some(k) = key {
        b = b.header("x-pindot-session", k);
    }
    let body = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name": name, "arguments": args,
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }}});
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let v = rpc_body(&to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap());
    let r = v.get("result").cloned().unwrap_or_else(|| panic!("{name}: HTTP {status}, no result: {v}"));
    let err = r.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let text = r
        .get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    (err, text)
}

#[tokio::test]
async fn stateless_calls_share_the_claude_session_named_by_the_header() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let a = Some("616b631e-9490-4585-86e2-b1f10e68a1c8");
    let b = Some("7b2c0000-0000-0000-0000-000000000000");

    // Each call is a separate HTTP request with no MCP session, yet the
    // approval, the screenshot geometry and the refs carry over.
    let (err, t) = stateless_call(&h.app, a, "request_access", json!({"apps": ["TextEdit"], "reason": "t"})).await;
    assert!(!err, "{t}");
    let (err, listing) = stateless_call(&h.app, a, "read_window", json!({})).await;
    assert!(!err, "approval must survive between stateless calls: {listing}");
    let (err, t) = stateless_call(&h.app, a, "screenshot", json!({})).await;
    assert!(!err, "{t}");
    let (err, t) = stateless_call(&h.app, a, "click", json!({"x": 356, "y": 356})).await;
    assert!(!err, "{t}");
    assert_eq!(h.state.control.lock_holder().as_deref(), Some("616b631e"));

    // Another Claude session: its own approvals, and the lock is A's.
    let (err, t) = stateless_call(&h.app, b, "read_window", json!({})).await;
    assert!(err && t.contains("request_access"), "{t}");
    stateless_call(&h.app, b, "request_access", json!({"apps": ["TextEdit"], "reason": "t"})).await;
    stateless_call(&h.app, b, "screenshot", json!({})).await;
    let (err, t) = stateless_call(&h.app, b, "click", json!({"x": 356, "y": 356})).await;
    assert!(err && t.contains("Another Claude session (616b631e)"), "{t}");

    // A's claude exits: its session (approvals, lock) ends.
    h.state.end_session(a.unwrap());
    assert_eq!(h.state.control.lock_holder(), None);
    let (err, t) = stateless_call(&h.app, b, "click", json!({"x": 356, "y": 356})).await;
    assert!(!err, "{t}");
    let (err, t) = stateless_call(&h.app, a, "read_window", json!({})).await;
    assert!(err && t.contains("request_access"), "a new session starts with no approvals: {t}");

    // Stateless calls without the header share one session.
    stateless_call(&h.app, None, "request_access", json!({"apps": ["TextEdit"], "reason": "t"})).await;
    let (err, t) = stateless_call(&h.app, None, "read_window", json!({})).await;
    assert!(!err, "{t}");
}

#[test]
fn session_key_prefers_our_header_then_the_mcp_connection() {
    use axum::http::HeaderMap;
    use pindot_core::server::tools::{SessionKey, session_key};
    assert_eq!(session_key(None), SessionKey::Connection);
    let mut h = HeaderMap::new();
    assert_eq!(session_key(Some(&h)), SessionKey::Keyed("shared".into()));
    h.insert("mcp-session-id", "abc".parse().unwrap());
    assert_eq!(session_key(Some(&h)), SessionKey::Connection);
    h.insert("x-pindot-session", " k1 ".parse().unwrap());
    assert_eq!(session_key(Some(&h)), SessionKey::Keyed("k1".into()));
}

#[tokio::test]
async fn esc_cancels_the_action_and_halts_until_the_next_prompt() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;
    ok(&h.app, &sid, "screenshot", json!({})).await;
    assert!(!h.stop.is_armed(), "the Esc tap is off until Claude acts");
    assert!(!h.stop.press(), "Esc passes through when Claude is not acting");

    let mut events = h.state.control.subscribe();
    let app = h.app.clone();
    let s = sid.clone();
    let started = Instant::now();
    let waiting = tokio::spawn(async move { call(&app, &s, "wait", json!({"ms": 5000})).await });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !h.stop.is_armed() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.stop.is_armed(), "the Esc tap is armed while Claude acts");
    assert_eq!(h.overlay.acting.lock().unwrap().first(), Some(&true), "indicator on");
    assert!(h.stop.press(), "Esc is consumed by the tap");

    let (err, text, _) = waiting.await.unwrap();
    assert!(err && text.contains("pressed Esc"), "{text}");
    assert!(started.elapsed() < Duration::from_secs(2), "wait was cancelled, not run out");
    let mut saw_stop = false;
    while let Ok(ev) = events.try_recv() {
        saw_stop |= matches!(ev, ControlEvent::EmergencyStop);
    }
    assert!(saw_stop, "voice loop hook got EmergencyStop");
    assert_eq!(h.overlay.acting.lock().unwrap().last(), Some(&false), "indicator off");

    // Halted: nothing acts until the user's next message.
    let t = fails(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert!(t.contains("pressed Esc"), "{t}");
    assert!(h.input.clicks().is_empty());
    assert_eq!(post_hook(&h.app, "/hooks/prompt", json!({"session_id": "x", "prompt": "go on"})).await, StatusCode::OK);
    ok(&h.app, &sid, "click", json!({"x": 356, "y": 356})).await;
    assert_eq!(h.input.clicks().len(), 1);

    // After the idle period the tap disarms and the indicator goes off.
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.stop.is_armed() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!h.stop.is_armed());
    assert_eq!(h.overlay.acting.lock().unwrap().last(), Some(&false));
}

#[tokio::test]
async fn typing_refuses_password_fields_and_unapproved_focus() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;

    h.ax.secure_focus.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = fails(&h.app, &sid, "type_text", json!({"text": "hunter2"})).await;
    assert!(t.contains("password"), "{t}");
    h.ax.secure_focus.store(false, std::sync::atomic::Ordering::SeqCst);

    let long = "The quick brown fox jumps over the lazy dog.";
    ok(&h.app, &sid, "type_text", json!({"text": long})).await;
    ok(&h.app, &sid, "key", json!({"combo": "cmd+n"})).await;
    let typed: String = h
        .input
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter_map(|a| if let InputAction::Type(s) = a { Some(s.clone()) } else { None })
        .collect();
    assert_eq!(typed, long, "typed in chunks, nothing lost");
    assert!(h.input.actions.lock().unwrap().iter().any(|a| matches!(a, InputAction::Key(_))));
    fails(&h.app, &sid, "key", json!({"combo": "cmd+banana"})).await;

    // Focus moves to an unapproved app: typing stops.
    *h.ax.frontmost.lock().unwrap() = Some(CHROME);
    let t = fails(&h.app, &sid, "type_text", json!({"text": "secret"})).await;
    assert!(t.contains("Google Chrome"), "{t}");
    fails(&h.app, &sid, "key", json!({"combo": "return"})).await;

    // focus_window brings the approved window back.
    ok(&h.app, &sid, "focus_window", json!({"window_id": 100})).await;
    assert_eq!(h.ax.raised.lock().unwrap().clone(), vec![100]);
    ok(&h.app, &sid, "type_text", json!({"text": "ok"})).await;
}

#[tokio::test]
async fn screenshot_then_click_maps_pixels_to_points() {
    let h = harness_with(fixture(), &["TextEdit"]);
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "test"})).await;

    let (err, text, shot) = call(&h.app, &sid, "screenshot", json!({})).await;
    assert!(!err, "{text}");
    let content = shot.get("content").and_then(Value::as_array).expect("content");
    assert_eq!(content[0].get("type").and_then(Value::as_str), Some("image"));
    assert_eq!(content[0].get("mimeType").and_then(Value::as_str), Some("image/jpeg"));
    // Mock display: 1440x900 pt at 2x -> 2880x1800 px -> sent at 1280x800.
    assert!(text.contains("1280x800"), "{text}");
    assert_eq!(h.state.activity.recent(50).iter().filter(|e| e.tool == "screenshot").count(), 1);

    ok(&h.app, &sid, "click", json!({"x": 640, "y": 400})).await;
    assert_eq!(h.input.clicks(), vec![(Point::new(720.0, 450.0), MouseButton::Left, 1)]);
    assert_eq!(h.overlay.moves.lock().unwrap().clone(), vec![Point::new(720.0, 450.0)]);

    // A window screenshot changes the coordinate space to that window.
    let t = ok(&h.app, &sid, "screenshot", json!({"window_id": 100})).await;
    assert!(t.contains("1280x960"), "{t}");
    // Window (100,100 800x600) at 1.6 px/pt: pixel (160,160) = point (200,200).
    ok(&h.app, &sid, "click", json!({"x": 160, "y": 160, "count": 2})).await;
    assert_eq!(h.input.clicks()[1], (Point::new(200.0, 200.0), MouseButton::Left, 2));
    fails(&h.app, &sid, "screenshot", json!({"window_id": 200})).await;
    let z = ok(&h.app, &sid, "zoom", json!({"x": 0, "y": 0, "width": 320, "height": 240})).await;
    assert!(z.contains("400x300"), "{z}");
}

// ---- Vault: saved logins, typed by Pindot, never seen by the model ----

const PASSWORD: &str = "Zq9-correct-horse-battery-STAPLE-77!";
const USERNAME: &str = "tony@example.com";
const TOTP_KEY: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

struct V {
    h: Harness,
    sid: String,
    id: String,
}

fn chrome_page(urls: &[&str], secure: bool) -> pindot_core::platform::FocusedPage {
    pindot_core::platform::FocusedPage {
        pid: CHROME,
        secure,
        text_input: true,
        page_urls: urls.iter().map(|u| u.to_string()).collect(),
    }
}

async fn vault_ready(approve: bool) -> V {
    let ax = fixture();
    ax.apps.lock().unwrap().iter_mut().find(|a| a.pid == CHROME).unwrap().bundle_id = Some("com.google.Chrome".into());
    let mut field = MockNode::new(7, 1, "AXSecureTextField", "Password", Some(Rect::new(820.0, 200.0, 200.0, 24.0)));
    field.value = Some(PASSWORD.into());
    ax.trees.lock().unwrap().insert(200, vec![MockNode::new(6, 0, "AXWindow", "Sign in", Some(Rect::new(800.0, 100.0, 500.0, 400.0))), field]);
    let h = harness_with(ax, &["Google Chrome"]);
    *h.approver.answer.lock().unwrap() = approve;
    *h.ax.frontmost.lock().unwrap() = Some(CHROME);
    *h.ax.page.lock().unwrap() = Some(chrome_page(&["https://example.com"], true));
    let id = h
        .state
        .vault
        .add(pindot_core::vault::NewLogin {
            name: "Work".into(),
            site: "https://example.com/login".into(),
            username: USERNAME.into(),
            password: PASSWORD.into(),
            authenticator: Some(TOTP_KEY.into()),
        })
        .unwrap()
        .id;
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["Google Chrome"], "reason": "sign in"})).await;
    V { h, sid, id }
}

fn typed(h: &Harness) -> Vec<String> {
    h.input
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter_map(|a| if let InputAction::Type(s) = a { Some(s.clone()) } else { None })
        .collect()
}

/// End an MCP session so the input lock is free for the next one.
async fn close(h: &Harness, sid: &str) {
    let r = h
        .app
        .clone()
        .oneshot(
            req("DELETE", "/mcp")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("mcp-session-id", sid)
                .header("mcp-protocol-version", "2025-06-18")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(r.status().is_success());
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.state.control.lock_holder().is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn set_page(h: &Harness, urls: &[&str], secure: bool) {
    *h.ax.page.lock().unwrap() = Some(chrome_page(urls, secure));
}

/// Everything the model, the activity view and the use log could show.
fn everything_visible(h: &Harness, results: &[String]) -> String {
    let mut all = results.join("\n");
    all.push_str(&serde_json::to_string(&h.activity.all(10_000)).unwrap());
    all.push_str(&serde_json::to_string(&h.state.vault.all_uses(10_000)).unwrap());
    all.push_str(&serde_json::to_string(&h.state.vault.list().unwrap()).unwrap());
    all.push_str(&format!("{:?}", h.approver.asked.lock().unwrap()));
    all
}

#[tokio::test]
async fn vault_logins_lists_names_for_the_page_in_front_and_no_secrets() {
    let v = vault_ready(true).await;
    let t = ok(&v.h.app, &v.sid, "vault_logins", json!({})).await;
    assert!(t.contains("Work") && t.contains(USERNAME) && t.contains("authenticator"), "{t}");
    assert!(!t.contains(PASSWORD) && !t.contains(TOTP_KEY));
    // A different site: nothing listed.
    set_page(&v.h, &["https://example.com.evil.test"], false);
    let t = ok(&v.h.app, &v.sid, "vault_logins", json!({})).await;
    assert!(t.contains("No saved login") && !t.contains("Work"), "{t}");
    // Unreadable page: nothing listed.
    *v.h.ax.page.lock().unwrap() = None;
    let t = ok(&v.h.app, &v.sid, "vault_logins", json!({})).await;
    assert!(!t.contains("Work"), "{t}");
}

#[tokio::test]
async fn a_fill_needs_the_user_to_allow_the_login_and_types_it_once_allowed() {
    let v = vault_ready(true).await;
    let mut results = vec![];
    results.push(ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await);
    assert_eq!(results[0], "typed");
    assert_eq!(typed(&v.h).concat(), PASSWORD);
    let asked = v.h.approver.asked.lock().unwrap().clone();
    let l = asked.last().unwrap().login.clone().expect("a login card");
    assert_eq!((l.name.as_str(), l.site.as_str(), l.field.as_str()), ("Work", "https://example.com", "password"));
    // The login stays allowed for the session: no second prompt.
    let n = v.h.approver.asked.lock().unwrap().len();
    set_page(&v.h, &["https://example.com"], false);
    results.push(ok(&v.h.app, &v.sid, "vault_fill", json!({"login": v.id, "field": "username"})).await);
    assert_eq!(v.h.approver.asked.lock().unwrap().len(), n);
    assert_eq!(typed(&v.h).last().unwrap(), USERNAME);
    // And an authenticator code (6 digits, never the key).
    results.push(ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "code"})).await);
    let code = typed(&v.h).last().unwrap().clone();
    assert!(code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()), "{code}");
    results.push(code.clone());
    // Nothing the model or the user-facing logs can see holds a secret.
    let seen = everything_visible(&v.h, &results[..2]);
    assert!(!seen.contains(PASSWORD) && !seen.contains(TOTP_KEY));
    assert!(!seen.contains(&format!("\"{code}\"")) || code == "typed");
    let uses = v.h.state.vault.uses_for(&v.id, 10);
    assert_eq!(uses.iter().filter(|u| u.ok && u.result == "typed").count(), 3, "{uses:?}");
}

#[tokio::test]
async fn a_denied_login_types_nothing_and_is_recorded() {
    let v = vault_ready(false).await;
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("did not allow"), "{t}");
    assert!(typed(&v.h).is_empty());
    let u = v.h.state.vault.uses_for(&v.id, 10);
    assert!(u.iter().any(|u| u.result == "refused" && !u.ok), "{u:?}");
}

#[tokio::test]
async fn lookalike_sites_never_get_a_secret() {
    let v = vault_ready(true).await;
    let bad = [
        "https://example.com.evil.test/login",
        "https://evil-example.com",
        "http://example.com",
        "https://example.com:8443",
        "https://example.com@evil.test",
        "https://www.example.com",
        "https://sub.example.com",
        "https://example.com.",
        "about:blank",
        "",
    ];
    for u in bad {
        // A fresh session each time, so the lock cannot be what refuses.
        let (sid, _) = open(&v.h.app).await;
        ok(&v.h.app, &sid, "request_access", json!({"apps": ["Google Chrome"], "reason": "t"})).await;
        set_page(&v.h, &[u], true);
        let t = fails(&v.h.app, &sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
        assert!(t.contains("not https://example.com"), "{u}: {t}");
        close(&v.h, &sid).await;
    }
    assert!(typed(&v.h).is_empty(), "nothing typed on any lookalike: {:?}", typed(&v.h));
}

#[tokio::test]
async fn a_foreign_frame_inside_the_right_page_is_refused() {
    let v = vault_ready(true).await;
    set_page(&v.h, &["https://example.com", "https://evil.test/frame"], true);
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("not https://example.com"), "{t}");
    assert!(typed(&v.h).is_empty());
    // Address not readable at all.
    set_page(&v.h, &[], true);
    fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(typed(&v.h).is_empty());
}

#[tokio::test]
async fn a_password_goes_only_into_a_secure_field_and_a_username_not_into_one() {
    let v = vault_ready(true).await;
    set_page(&v.h, &["https://example.com"], false);
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("not a password field"), "{t}");
    set_page(&v.h, &["https://example.com"], true);
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "username"})).await;
    assert!(t.contains("password field"), "{t}");
    assert!(typed(&v.h).is_empty());
    // The right boxes work.
    ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    set_page(&v.h, &["https://example.com"], false);
    ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "username"})).await;
}

#[tokio::test]
async fn only_a_browser_the_user_approved_gets_a_fill() {
    // A non-browser app that is approved and shows a page-like focus.
    let ax = fixture();
    let h = harness_with(ax, &["TextEdit"]);
    *h.approver.answer.lock().unwrap() = true;
    h.state
        .vault
        .add(pindot_core::vault::NewLogin { name: "Work".into(), site: "example.com".into(), username: "u".into(), password: PASSWORD.into(), authenticator: None })
        .unwrap();
    let (sid, _) = open(&h.app).await;
    ok(&h.app, &sid, "request_access", json!({"apps": ["TextEdit"], "reason": "t"})).await;
    *h.ax.page.lock().unwrap() =
        Some(pindot_core::platform::FocusedPage { pid: TEXTEDIT, secure: true, text_input: true, page_urls: vec!["https://example.com".into()] });
    let t = fails(&h.app, &sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("not a browser"), "{t}");
    // The browser itself is not approved: refused before anything else.
    *h.ax.frontmost.lock().unwrap() = Some(CHROME);
    set_page(&h, &["https://example.com"], true);
    let t = fails(&h.app, &sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("not approved"), "{t}");
    assert!(typed(&h).is_empty());
}

#[tokio::test]
async fn fills_are_rate_limited_and_repeated_refusals_lock_the_vault() {
    let v = vault_ready(true).await;
    for _ in 0..5 {
        ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    }
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("too many"), "{t}");
    assert_eq!(typed(&v.h).concat().len(), 5 * PASSWORD.len());
    close(&v.h, &v.sid).await;

    // A fresh session: three wrong-site attempts lock it.
    let (sid2, _) = open(&v.h.app).await;
    ok(&v.h.app, &sid2, "request_access", json!({"apps": ["Google Chrome"], "reason": "t"})).await;
    set_page(&v.h, &["https://evil.test"], true);
    for _ in 0..3 {
        fails(&v.h.app, &sid2, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    }
    set_page(&v.h, &["https://example.com"], true);
    let t = fails(&v.h.app, &sid2, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("locked"), "{t}");
    assert_eq!(typed(&v.h).concat().len(), 5 * PASSWORD.len());
}

#[tokio::test]
async fn esc_during_a_fill_stops_it() {
    let v = vault_ready(true).await;
    *v.h.approver.esc.lock().unwrap() = Some(v.h.stop.clone());
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    assert!(t.contains("pressed Esc"), "{t}");
    assert!(typed(&v.h).is_empty(), "Esc before the first character types nothing");
}

#[tokio::test]
async fn read_window_never_shows_a_password_boxs_value() {
    let v = vault_ready(true).await;
    ok(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "password"})).await;
    let listing = ok(&v.h.app, &v.sid, "read_window", json!({"window_id": 200})).await;
    assert!(listing.contains("securetextfield") && !listing.contains(PASSWORD), "{listing}");
    let found = ok(&v.h.app, &v.sid, "find", json!({"query": "Zq9-correct"})).await;
    assert!(!found.contains(PASSWORD), "{found}");
}

#[tokio::test]
async fn unknown_logins_and_missing_fields_are_refused_in_plain_words() {
    let v = vault_ready(true).await;
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Nope", "field": "password"})).await;
    assert!(t.contains("no saved login"), "{t}");
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Work", "field": "cvv"})).await;
    assert!(t.contains("username"), "{t}");
    v.h.state.vault.add(pindot_core::vault::NewLogin { name: "Plain".into(), site: "example.com".into(), username: "u".into(), password: "pw-pw-pw-pw".into(), authenticator: None }).unwrap();
    let t = fails(&v.h.app, &v.sid, "vault_fill", json!({"login": "Plain", "field": "code"})).await;
    assert!(t.contains("no saved code"), "{t}");
}
