//! Deskhand app: one click-through AI-cursor overlay per monitor (with the
//! "Claude is controlling input" banner), the embedded MCP server with the
//! desktop tools, the per-app approval window and the activity view.
//!
//!   deskhand [--activity] [--capturable] [--demo] [--exit-after SECONDS] [--no-server]
//!            [--test-turn PROMPT [--model M] [--permission-mode MODE] [--turn-timeout S] [--accept-trust]]
//!            [--test-esc-after MS]
//!
//! `--activity` opens the activity view. `--capturable` lets screenshots see
//! Deskhand's own windows (tests and evidence only). `--test-turn` runs one
//! real turn of interactive `claude` against this app and quits.
//! `--test-esc-after MS` (with `DESKHAND_TEST_AUTO_APPROVE`) posts one
//! synthetic Esc once an action has run for MS with a test app in front, and
//! checks that no app received it.

mod approval;
mod overlay;
mod test_turn;

use std::sync::Arc;
use std::time::Duration;

use deskhand_core::brain::config::{write_hook_settings, write_mcp_config};
use deskhand_core::paths::{activity_log_path, load_or_create_token};
use deskhand_core::platform::{Overlay, Point};
use deskhand_core::server::activity::{ActivityEntry, ActivityLog};
use deskhand_core::server::control::ControlEvent;
use deskhand_core::server::{RunningServer, ServerDeps, ServerState, start};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub use approval::TauriApprover;
pub use overlay::{MonitorRect, TauriOverlay};

struct Args {
    demo: bool,
    exit_after: Option<u64>,
    server: bool,
    capturable: bool,
    activity: bool,
    test_turn: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    turn_timeout: u64,
    accept_trust: bool,
    test_esc_after: Option<u64>,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned();
    let flag = |name: &str| a.iter().any(|x| x == name);
    Args {
        demo: flag("--demo"),
        exit_after: opt("--exit-after").and_then(|s| s.parse().ok()),
        server: !flag("--no-server"),
        capturable: flag("--capturable"),
        activity: flag("--activity"),
        test_turn: opt("--test-turn"),
        model: opt("--model"),
        permission_mode: opt("--permission-mode"),
        turn_timeout: opt("--turn-timeout").and_then(|s| s.parse().ok()).unwrap_or(240),
        accept_trust: flag("--accept-trust"),
        test_esc_after: opt("--test-esc-after").and_then(|s| s.parse().ok()),
    }
}

#[derive(Serialize)]
struct Recent {
    path: Option<String>,
    entries: Vec<ActivityEntry>,
}

/// The activity view's initial rows.
#[tauri::command]
fn activity_recent(app: AppHandle) -> Recent {
    match app.try_state::<Arc<ServerState>>() {
        Some(s) => Recent {
            path: s.activity.path().map(|p| p.display().to_string()),
            entries: s.activity.recent(300),
        },
        None => Recent { path: None, entries: vec![] },
    }
}

fn open_activity_window(app: &AppHandle, capturable: bool) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, "activity", WebviewUrl::App("activity.html".into()))
        .title("Deskhand activity")
        .inner_size(820.0, 460.0)
        .focused(false)
        // Deskhand's own windows stay out of Claude's screenshots.
        .content_protected(!capturable)
        .build()?;
    Ok(())
}

pub fn run() {
    deskhand_core::init_logging();
    let args = parse_args();

    let builder = tauri::Builder::default();
    // Tauri activates the app at launch by default; an overlay must never
    // take focus from the app the user is working in.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init()).activate_ignoring_other_apps(false);

    builder
        .invoke_handler(tauri::generate_handler![approval::approval_answer, activity_recent])
        .on_window_event(|window, event| {
            // Closing an approval window is a "Deny".
            if let tauri::WindowEvent::Destroyed = event
                && window.label().starts_with(approval::LABEL_PREFIX)
                && let Some(a) = window.app_handle().try_state::<Arc<TauriApprover>>()
            {
                a.answer_from_window(window.label(), false);
            }
        })
        .setup(move |app| {
            // No Dock icon, no menu bar, never steals focus.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let monitors: Vec<MonitorRect> = app
                .available_monitors()?
                .iter()
                .map(|m| {
                    let s = m.scale_factor();
                    let p = m.position().to_logical::<f64>(s);
                    let z = m.size().to_logical::<f64>(s);
                    MonitorRect { x: p.x, y: p.y, width: z.width, height: z.height, scale: s }
                })
                .collect();
            for (i, m) in monitors.iter().enumerate() {
                println!("monitor {i}: origin ({}, {}) size {}x{} pt, scale {}", m.x, m.y, m.width, m.height, m.scale);
                overlay::create_panel(app.handle(), i, m, args.capturable)?;
            }
            let ov = Arc::new(TauriOverlay::new(app.handle().clone()));
            app.manage(ov.clone());

            if args.server {
                let ax = deskhand_core::platform::native().accessibility;
                let approver = Arc::new(TauriApprover::new(app.handle().clone(), ax, args.capturable));
                app.manage(approver.clone());
                let log_path = activity_log_path()?;
                let activity = Arc::new(ActivityLog::open(log_path.clone()));
                let state = ServerState::new(ServerDeps::native(ov.clone(), approver, activity));
                let token = load_or_create_token()?;
                let port = deskhand_core::port_from_env();
                let server = tauri::async_runtime::block_on(start(state.clone(), &token, port))?;
                write_mcp_config(&server.mcp_url(), &token)?;
                write_hook_settings(&server.stop_hook_url(), &token)?;
                println!("MCP server: {} (token and configs in the Deskhand data dir)", server.mcp_url());
                println!("activity log: {}", log_path.display());
                forward_events(app.handle(), &state);
                if let Some(ms) = args.test_esc_after {
                    spawn_esc_self_test(state.clone(), ms);
                }
                if let Some(prompt) = args.test_turn.clone() {
                    let urls = test_turn::ServerUrls {
                        mcp: server.mcp_url(),
                        stop_hook: server.stop_hook_url(),
                        token: token.clone(),
                    };
                    let targs = test_turn::TurnArgs {
                        prompt,
                        model: args.model.clone(),
                        permission_mode: args.permission_mode.clone(),
                        timeout: Duration::from_secs(args.turn_timeout),
                        accept_trust: args.accept_trust,
                        cwd: None,
                    };
                    let st = state.clone();
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        let ok = match test_turn::run(st, urls, targs) {
                            Ok(ok) => ok,
                            Err(e) => {
                                println!("test-turn: error: {e:#}");
                                false
                            }
                        };
                        println!("test-turn: RESULT {}", if ok { "TURN COMPLETED" } else { "CHECK OUTPUT" });
                        std::thread::sleep(Duration::from_millis(500));
                        handle.exit(0);
                    });
                }
                app.manage(state);
                app.manage(server);
            }

            if args.activity {
                open_activity_window(app.handle(), args.capturable)?;
            }
            if args.demo {
                let ov = ov.clone();
                let monitors = monitors.clone();
                std::thread::spawn(move || run_demo(&*ov, &monitors));
            }
            if let Some(secs) = args.exit_after {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(secs));
                    println!("exit-after {secs}s reached; quitting");
                    handle.exit(0);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Deskhand")
        .run(|app, event| match event {
            tauri::RunEvent::Ready => {
                #[cfg(target_os = "macos")]
                overlay::log_activation_policy();
            }
            // Accessory app: closing the last normal window must not quit.
            tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            tauri::RunEvent::Exit => {
                // Never leave the Esc tap armed.
                if let Some(s) = app.try_state::<Arc<ServerState>>() {
                    s.control.shutdown();
                }
                if let Some(s) = app.try_state::<RunningServer>() {
                    println!("server stopping on {}", s.addr);
                }
            }
            _ => {}
        });
}

/// Activity entries to the activity view; Esc to the overlays (and the
/// place the voice loop will stop speech).
fn forward_events(app: &AppHandle, state: &Arc<ServerState>) {
    let mut act = state.activity.subscribe();
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match act.recv().await {
                Ok(e) => {
                    let _ = h.emit_to("activity", "deskhand:activity", e);
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
    let mut ctl = state.control.subscribe();
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match ctl.recv().await {
                Ok(ControlEvent::EmergencyStop) => {
                    println!("EMERGENCY STOP: user pressed Esc (action cancelled, input released)");
                    let _ = h.emit("deskhand:stopped", ());
                }
                Ok(ControlEvent::Acting { on }) => tracing::info!(on, "controlling-input indicator"),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Self-test of the stop key. Once an action has been in flight for `ms`
/// while a test-approved app (one the test opened) is frontmost, post one
/// unmarked Esc and check that no app received it. Never posts otherwise, so
/// a missed Esc could only reach the test's own window.
fn spawn_esc_self_test(state: Arc<ServerState>, ms: u64) {
    std::thread::spawn(move || {
        let ax = deskhand_core::platform::native().accessibility;
        let front_is_test_app = || {
            let Some(pid) = ax.frontmost_pid().ok().flatten() else { return false };
            let Some(app) = ax.app_info(pid) else { return false };
            state.auto_approve.iter().any(|n| n.eq_ignore_ascii_case(&app.name))
        };
        let t0 = std::time::Instant::now();
        let mut since: Option<std::time::Instant> = None;
        loop {
            if t0.elapsed() > Duration::from_secs(600) {
                println!("esc-test: condition never met within 600 s; not posting");
                return;
            }
            let ready = state.control.in_flight() && state.control.is_armed() && front_is_test_app();
            match (ready, since) {
                (true, None) => since = Some(std::time::Instant::now()),
                (true, Some(s)) if s.elapsed() >= Duration::from_millis(ms) => break,
                (false, _) => since = None,
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        #[cfg(target_os = "macos")]
        {
            use deskhand_core::platform::macos::stopkey::{post_test_escape, watch_escape};
            let (tx, rx) = std::sync::mpsc::channel();
            let watcher = watch_escape(tx);
            if let Err(e) = &watcher {
                println!("esc-test: watcher tap failed ({e}); posting anyway");
            }
            std::thread::sleep(Duration::from_millis(100));
            println!("esc-test: posting one synthetic (unmarked) Esc while an action is in flight");
            if let Err(e) = post_test_escape() {
                println!("esc-test: post failed: {e}");
            }
            std::thread::sleep(Duration::from_millis(400));
            drop(watcher);
            let seen: Vec<bool> = rx.try_iter().collect();
            println!(
                "esc-test: Esc events that reached apps: {} ({})",
                seen.len(),
                if seen.is_empty() { "consumed: PASS" } else { "NOT consumed: FAIL" }
            );
            println!(
                "esc-test: halted {}, stop key armed {}, action in flight {}",
                state.control.is_halted(),
                state.control.is_armed(),
                state.control.in_flight()
            );
        }
    });
}

/// Centre, then each corner (inset 60 pt), then centre, on every monitor.
fn run_demo(ov: &dyn Overlay, monitors: &[MonitorRect]) {
    std::thread::sleep(Duration::from_millis(1500)); // let the webviews load
    for (i, m) in monitors.iter().enumerate() {
        let inset = 60.0;
        let points = [
            ("centre", m.x + m.width / 2.0, m.y + m.height / 2.0),
            ("top-left", m.x + inset, m.y + inset),
            ("top-right", m.x + m.width - inset, m.y + inset),
            ("bottom-right", m.x + m.width - inset, m.y + m.height - inset),
            ("bottom-left", m.x + inset, m.y + m.height - inset),
            ("centre", m.x + m.width / 2.0, m.y + m.height / 2.0),
        ];
        for (name, x, y) in points {
            let d = ov.move_cursor(Point::new(x, y), Some(format!("demo: monitor {i} {name}")));
            let t = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs_f64()).unwrap_or(0.0);
            println!("{t:.3} demo: monitor {i} {name} -> ({x:.0}, {y:.0}) over {} ms", d.as_millis());
            std::thread::sleep(d + Duration::from_millis(1200));
        }
    }
    println!("demo done");
}
