//! Pindot app: one click-through AI-cursor overlay per monitor (with the
//! "Claude is controlling input" banner and the voice pill), the embedded
//! MCP server with the desktop tools, the per-app approval window, the tray
//! icon, the Setup / Settings / Activity window and the voice loop.
//!
//!   pindot [--voice] [--activity] [--setup] [--capturable] [--model M] [--no-server]
//!            [--demo] [--exit-after SECONDS]
//!
//! Normal start: the tray icon, the MCP server on the port from Settings, and
//! hold-to-talk once Setup is done (models downloaded, permissions granted,
//! `claude` signed in). The Setup window opens by itself (without taking
//! focus) when something is missing. `--voice` starts hold-to-talk even
//! before Setup is finished. `--activity` / `--setup` open that view.
//! `--capturable` lets screenshots see Pindot's own windows (evidence
//! only). `--demo` shows every AI cursor action on each monitor. Ctrl+C or SIGTERM
//! quit cleanly.
//!
//! TEST BUILDS ONLY (`--features test-switches`, never in a release build):
//!   [--test-turn PROMPT [--permission-mode MODE] [--turn-timeout S] [--accept-trust] [--cwd DIR]]
//!   [--test-esc-after MS]
//!   [--test-voice WAV [--test-voice-esc] [--turn-timeout S] [--keep-running]]
//! `--test-turn` runs one real turn of interactive `claude` against this app
//! and quits. `--test-esc-after MS` (with `PINDOT_TEST_AUTO_APPROVE`) posts
//! one synthetic Esc once an action has run for MS with a test app in front,
//! and checks that no app received it. `--test-voice WAV` runs the voice loop
//! with the WAV in place of the microphone and a synthetic hotkey press,
//! prints the latency of each stage and quits (stays up with
//! `--keep-running`); `--test-voice-esc` also presses Esc during that first
//! turn and then speaks the request again. Without the feature these flags
//! are ignored with a warning.

mod approval;
mod chat;
mod dictation;
mod overlay;
mod test_turn;
mod ui;
mod vault;
mod voice;

use std::sync::Arc;
use std::time::Duration;

use pindot_core::brain::config::{write_hook_settings, write_mcp_config};
use pindot_core::paths::{activity_log_path, load_or_create_token};
use pindot_core::platform::{CursorAction, KeyCombo, MouseButton, Overlay, Point, drag_travel_ms};
use pindot_core::server::activity::ActivityLog;
use pindot_core::server::control::ControlEvent;
use pindot_core::server::{RunningServer, ServerDeps, ServerState, start};
use pindot_core::settings::{LoadNote, Settings};
use tauri::{AppHandle, Emitter, Listener, Manager};

pub use approval::TauriApprover;
pub use overlay::{MonitorRect, TauriOverlay};

struct Args {
    demo: bool,
    exit_after: Option<u64>,
    server: bool,
    capturable: bool,
    activity: bool,
    setup: bool,
    voice: bool,
    model: Option<String>,
    test: TestArgs,
}

/// The test-only switches. Empty unless the app was built with
/// `--features test-switches`.
#[derive(Default)]
struct TestArgs {
    test_turn: Option<String>,
    permission_mode: Option<String>,
    turn_timeout: u64,
    accept_trust: bool,
    /// The folder the test turn's `claude` runs in (default `~/pindot/.spike-cwd`).
    cwd: Option<String>,
    test_esc_after: Option<u64>,
    test_voice: Option<String>,
    keep_running: bool,
    test_voice_esc: bool,
}

impl TestArgs {
    fn any(&self) -> bool {
        self.test_turn.is_some() || self.test_esc_after.is_some() || self.test_voice.is_some()
    }
}

const TEST_FLAGS: &[&str] = &[
    "--test-turn",
    "--permission-mode",
    "--turn-timeout",
    "--accept-trust",
    "--cwd",
    "--test-esc-after",
    "--test-voice",
    "--keep-running",
    "--test-voice-esc",
];

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned();
    let flag = |name: &str| a.iter().any(|x| x == name);
    let test = if cfg!(feature = "test-switches") {
        TestArgs {
            test_turn: opt("--test-turn"),
            permission_mode: opt("--permission-mode"),
            turn_timeout: opt("--turn-timeout").and_then(|s| s.parse().ok()).unwrap_or(240),
            accept_trust: flag("--accept-trust"),
            cwd: opt("--cwd"),
            test_esc_after: opt("--test-esc-after").and_then(|s| s.parse().ok()),
            test_voice: opt("--test-voice"),
            keep_running: flag("--keep-running"),
            test_voice_esc: flag("--test-voice-esc"),
        }
    } else {
        let used: Vec<&str> = TEST_FLAGS.iter().copied().filter(|f| flag(f)).collect();
        if !used.is_empty() {
            println!(
                "WARNING: {} ignored: this build has no test switches (build with --features test-switches)",
                used.join(", ")
            );
        }
        TestArgs { turn_timeout: 240, ..TestArgs::default() }
    };
    Args {
        demo: flag("--demo"),
        exit_after: opt("--exit-after").and_then(|s| s.parse().ok()),
        server: !flag("--no-server"),
        capturable: flag("--capturable"),
        activity: flag("--activity"),
        setup: flag("--setup"),
        voice: flag("--voice"),
        model: opt("--model"),
        test,
    }
}

fn load_settings() -> (Settings, String) {
    match Settings::load() {
        Ok((s, note)) => {
            let text = match note {
                LoadNote::Loaded => String::new(),
                LoadNote::Fresh => "First start: default settings.".into(),
                LoadNote::MigratedVoiceSettings => "Voice settings from the earlier voice-settings.json were moved into settings.json.".into(),
                LoadNote::Invalid(e) => format!("settings.json could not be used ({e}); it was kept as a backup and the defaults are in use."),
                LoadNote::NewerVersion(v) => format!("settings.json is from a newer Pindot (version {v}); unknown fields are kept."),
            };
            (s, text)
        }
        Err(e) => (Settings::default(), format!("Settings could not be read ({e:#}); using the defaults.")),
    }
}

pub fn run() {
    pindot_core::init_logging();
    let args = parse_args();
    if cfg!(feature = "test-switches") {
        println!("!!! TEST BUILD: test switches are compiled in (PINDOT_TEST_AUTO_APPROVE, --test-*). Not for normal use. !!!");
    }
    if args.capturable {
        println!("NOTE: --capturable: Pindot's own windows show up in screenshots (evidence runs only)");
    }

    // Windows: one Pindot per data folder. A second launch asks the first
    // to show its window and ends here. (macOS does this by itself; the
    // demo, a serverless start and the test paths are meant to run beside.)
    #[cfg(windows)]
    let instance = {
        use pindot_core::platform::windows::instance::{Claim, ask_first_to_show, claim};
        let guarded = args.server && !args.demo && !args.test.any();
        match pindot_core::paths::data_dir().ok().filter(|_| guarded).map(|dir| (claim(&dir), dir)) {
            Some((Claim::Second, dir)) => {
                let asked = ask_first_to_show(&dir);
                println!(
                    "Pindot is already running{}; this second start ends here",
                    if asked { " and was asked to show its window" } else { "" }
                );
                return;
            }
            Some((Claim::First(guard), _)) => Some(guard),
            None => None,
        }
    };

    let builder = tauri::Builder::default();
    // Tauri activates the app at launch by default; an overlay must never
    // take focus from the app the user is working in.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init()).activate_ignoring_other_apps(false);

    builder
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            approval::approval_answer,
            approval::approval_answer_inline,
            approval::approvals_pending,
            chat::chat_state,
            chat::chat_send,
            chat::chat_attach_paths,
            chat::chat_attach_bytes,
            chat::chat_choose_files,
            chat::chat_stop,
            chat::chat_history,
            chat::chat_image,
            chat::chat_open,
            dictation::dictation_start,
            dictation::dictation_stop,
            dictation::dictation_cancel,
            dictation::dictation_focus,
            ui::app_info,
            ui::settings_get,
            ui::settings_save,
            ui::choose_brain_folder,
            ui::list_input_devices,
            ui::hotkey_check,
            ui::claude_status,
            ui::permissions,
            ui::permission_request,
            ui::permission_open_settings,
            ui::models_status,
            ui::voice_download,
            ui::voice_preview,
            ui::models_download,
            ui::models_cancel,
            ui::connect_status,
            ui::connect_command,
            ui::connect_claude,
            ui::disconnect_claude,
            ui::token_regenerate,
            vault::vault_state,
            vault::vault_add,
            vault::vault_update,
            vault::vault_delete,
            vault::vault_uses,
            vault::vault_import_choose,
            vault::vault_import_apply,
            vault::vault_import_cancel,
            ui::activity_all,
            ui::activity_clear,
            ui::onboarding_finish,
            ui::voice_set_enabled,
            ui::voice_check,
            ui::setup_check,
        ])
        .on_window_event(|window, event| {
            // The main window closed: the keyboard goes back to the user's app.
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::Destroyed = event
                && window.label() == ui::MAIN
            {
                // SAFETY: window events arrive on the main thread.
                unsafe { pindot_core::platform::macos::window::deactivate_app() };
            }
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

            // A second launch (Windows) shows this instance's window.
            #[cfg(windows)]
            if let Some(guard) = instance {
                let h = app.handle().clone();
                guard.on_show(move || {
                    println!("a second start asked for the window");
                    if let Err(e) = ui::open_main(&h, "setup", true) {
                        println!("the window could not open: {e}");
                    }
                });
                app.manage(guard);
            }

            let (settings, load_note) = load_settings();
            if !load_note.is_empty() {
                println!("settings: {load_note}");
            }
            let port = settings.effective().port;
            app.manage(ui::Ui::new(args.capturable, args.voice, args.model.clone(), settings, load_note));
            chat::start(app.handle());
            // Each overlay page asks for the assistant's name and the cursor
            // colour once it loads (events sent before that are lost);
            // Settings sends them on save.
            let h = app.handle().clone();
            app.listen_any("pindot:overlay-ready", move |_| {
                ui::send_assistant_name(&h);
                ui::send_cursor_color(&h);
            });

            let monitors: Vec<MonitorRect> = app
                .available_monitors()?
                .iter()
                .map(|m| {
                    let (p, z) = (m.position(), m.size());
                    MonitorRect::from_physical((p.x, p.y), (z.width, z.height), m.scale_factor(), cfg!(windows))
                })
                .collect();
            for (i, m) in monitors.iter().enumerate() {
                println!(
                    "monitor {i}: origin ({}, {}) size {}x{} pt, scale {}, {} pt per page pixel",
                    m.x, m.y, m.width, m.height, m.scale, m.unit
                );
                overlay::create_panel(app.handle(), i, m, args.capturable)?;
            }
            let ov = Arc::new(TauriOverlay::new(app.handle().clone(), monitors.first().map_or(1.0, |m| m.unit)));
            app.manage(ov.clone());
            app.manage(Arc::new(pindot_core::vault::Vault::open_system()));
            app.manage(vault::ImportHold::default());
            match dictation::DictationHub::new(app.handle()) {
                Ok(hub) => {
                    app.manage(hub);
                }
                Err(e) => tracing::warn!("dictation is off: {e}"),
            }

            if args.server {
                match start_server(app.handle(), &ov, port, args.capturable) {
                    Ok((state, server)) => {
                        forward_events(app.handle(), &state);
                        run_test_paths(app.handle(), &args, &state, &server)?;
                        app.manage(state);
                        app.manage(server);
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        println!("MCP server could not start: {msg}");
                        app.state::<ui::Ui>().set_server_error(msg);
                    }
                }
            } else {
                app.state::<ui::Ui>().set_server_error("started with --no-server".into());
            }
            quit_on_signals(app.handle());
            if let Err(e) = ui::build_tray(app.handle()) {
                println!("tray icon could not be created: {e}");
            }

            if args.activity {
                ui::open_main(app.handle(), "activity", false)?;
            } else if args.setup {
                ui::open_main(app.handle(), "setup", false)?;
            }
            // The test paths drive their own voice loop / turn.
            if !args.test.any() {
                let handle = app.handle().clone();
                let explicit_view = args.activity || args.setup;
                std::thread::Builder::new()
                    .name("pindot-startup".into())
                    .spawn(move || startup_checks(&handle, explicit_view))?;
            }
            if args.demo {
                println!("NOTE: --demo: showing every AI cursor action on each monitor");
                let ov = ov.clone();
                let monitors = monitors.clone();
                let handle = app.handle().clone();
                std::thread::spawn(move || run_demo(&handle, &*ov, &monitors));
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
        .expect("error while building Pindot")
        .run(|app, event| match event {
            tauri::RunEvent::Ready => {
                #[cfg(target_os = "macos")]
                overlay::log_activation_policy();
            }
            // Accessory app: closing the last normal window must not quit.
            tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            tauri::RunEvent::Exit => {
                // The voice loop first: hotkey tap, audio, the claude child.
                if let Some(ui) = app.try_state::<ui::Ui>() {
                    ui.shutdown_voice();
                }
                // Never leave the Esc tap armed.
                if let Some(s) = app.try_state::<Arc<ServerState>>() {
                    s.control.shutdown();
                }
                if let Some(s) = app.try_state::<RunningServer>() {
                    println!("server stopping on {}", s.addr);
                }
                println!("Pindot exited cleanly");
            }
            _ => {}
        });
}

fn start_server(
    app: &AppHandle,
    ov: &Arc<TauriOverlay>,
    port: u16,
    capturable: bool,
) -> anyhow::Result<(Arc<ServerState>, RunningServer)> {
    let ax = pindot_core::platform::native().accessibility;
    let approver = Arc::new(TauriApprover::new(app.clone(), ax, capturable));
    let log_path = activity_log_path()?;
    let activity = Arc::new(ActivityLog::open(log_path.clone()));
    let vault = app.state::<Arc<pindot_core::vault::Vault>>().inner().clone();
    let state = ServerState::new(ServerDeps::native(ov.clone(), approver.clone(), activity, vault));
    let token = load_or_create_token()?;
    let server = match tauri::async_runtime::block_on(start(state.clone(), &token, port)) {
        Ok(s) => s,
        Err(e) => {
            state.control.shutdown();
            return Err(e.context(format!("port {port} (change it in Settings, or quit what uses it)")));
        }
    };
    app.manage(approver);
    write_mcp_config(&server.mcp_url(), &token)?;
    write_hook_settings(&server.stop_hook_url(), &token)?;
    println!("MCP server: {} (token and configs in the Pindot data dir)", server.mcp_url());
    println!("activity log: {}", log_path.display());
    Ok((state, server))
}

/// Opens Setup (without focus) when something is missing, then starts
/// hold-to-talk if it can run.
fn startup_checks(app: &AppHandle, explicit_view: bool) {
    let ui = app.state::<ui::Ui>();
    let claude = ui::claude_info();
    let signed_in = claude.signed_in;
    match &claude.path {
        None => println!("claude: not found; Setup shows how to install it"),
        Some(p) => println!(
            "claude: {p} ({}), {}",
            claude.version.as_deref().unwrap_or("version unknown"),
            if signed_in { "signed in" } else { "NOT signed in (run `claude login`)" }
        ),
    }
    let s = ui.settings();
    let server_down = app.try_state::<RunningServer>().is_none();
    let needs_setup = !s.onboarding_done || claude.path.is_none() || !signed_in || server_down;
    if needs_setup && !explicit_view {
        println!("setup: opening the Setup window");
        if let Err(e) = ui::open_main(app, "setup", false) {
            println!("setup window could not open: {e}");
        }
    }
    ui::restart_voice_now(app);
}

/// The automated test paths (test builds only; `args.test` is empty
/// otherwise).
fn run_test_paths(
    app: &AppHandle,
    args: &Args,
    state: &Arc<ServerState>,
    server: &RunningServer,
) -> Result<(), Box<dyn std::error::Error>> {
    let t = &args.test;
    let token = server.token.get().to_string();
    if let Some(ms) = t.test_esc_after {
        println!("TEST SWITCH: --test-esc-after {ms}");
        spawn_esc_self_test(state.clone(), ms);
    }
    if let Some(prompt) = t.test_turn.clone() {
        println!("TEST SWITCH: --test-turn");
        let urls = test_turn::ServerUrls { mcp: server.mcp_url(), stop_hook: server.stop_hook_url(), token: token.clone() };
        let targs = test_turn::TurnArgs {
            prompt,
            model: args.model.clone(),
            permission_mode: t.permission_mode.clone(),
            timeout: Duration::from_secs(t.turn_timeout),
            accept_trust: t.accept_trust,
            cwd: t.cwd.clone().map(Into::into),
        };
        let st = state.clone();
        let handle = app.clone();
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
    if let Some(p) = &t.test_voice {
        println!("TEST SWITCH: --test-voice (WAV in place of the microphone, synthetic hotkey)");
        let link = voice::ServerLink { mcp_url: server.mcp_url(), stop_hook_url: server.stop_hook_url(), token };
        let test_wav = Some(std::fs::canonicalize(p).map_err(|e| format!("--test-voice {p}: {e}"))?);
        let opts = voice::VoiceOptions {
            test_wav,
            model: args.model.clone(),
            test_timeout: Duration::from_secs(t.turn_timeout),
            exit_when_done: !t.keep_running,
            test_esc: t.test_voice_esc,
            with_voice: true,
            hotkey: app.state::<ui::Ui>().settings().effective().hotkey,
        };
        app.state::<ui::Ui>().adopt_voice(voice::start(app, state.clone(), link, opts));
    }
    Ok(())
}

/// Ctrl+C and SIGTERM quit through the normal exit path, so the voice loop,
/// the `claude` child and the key taps are closed. If that hangs, a second
/// timer ends the process.
fn quit_on_signals(app: &AppHandle) {
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut int), Ok(mut term)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else {
                return;
            };
            tokio::select! {
                _ = int.recv() => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        println!("signal received; quitting");
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(15));
            eprintln!("clean exit took too long; ending the process");
            std::process::exit(1);
        });
        h.exit(0);
    });
}

/// Activity entries to the activity view; Esc to the overlays (the voice
/// loop has its own subscriber that stops speech and the turn).
fn forward_events(app: &AppHandle, state: &Arc<ServerState>) {
    let mut act = state.activity.subscribe();
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match act.recv().await {
                Ok(e) => {
                    let _ = h.emit_to(ui::MAIN, "pindot:activity", e);
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
                    let _ = h.emit("pindot:stopped", ());
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
        let ax = pindot_core::platform::native().accessibility;
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
            use pindot_core::platform::macos::stopkey::{post_test_escape, watch_escape};
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
fn run_demo(app: &tauri::AppHandle, ov: &dyn Overlay, monitors: &[MonitorRect]) {
    std::thread::sleep(Duration::from_millis(1500)); // let the webviews load
    let stamp = || std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs_f64()).unwrap_or(0.0);
    if let Some(m) = monitors.first() {
        demo_companion(app, ov, m, &stamp);
    }
    // One showcase of every kind of action per monitor; the glide between
    // monitors crosses the edge. Nothing is clicked or typed for real.
    for (i, m) in monitors.iter().enumerate() {
        let at = |fx: f64, fy: f64| Point::new(m.x + m.width * fx, m.y + m.height * fy);
        let go = |p: Point, label: &str| {
            let d = ov.move_cursor(p, Some(label.to_string()));
            std::thread::sleep(d);
        };
        let act = |name: &str, a: CursorAction, hold: u64| {
            println!("{:.3} demo: monitor {i} {name}", stamp());
            ov.set_acting(true);
            ov.action(&a);
            std::thread::sleep(Duration::from_millis(hold));
            ov.set_acting(false);
            std::thread::sleep(Duration::from_millis(500));
        };
        go(at(0.3, 0.35), "New Document");
        act("click", CursorAction::Click { button: MouseButton::Left, count: 1 }, 900);
        go(at(0.62, 0.3), "Save");
        act("press", CursorAction::Press, 900);
        go(at(0.7, 0.55), "(812, 344)");
        act("right-click", CursorAction::Click { button: MouseButton::Right, count: 1 }, 900);
        go(at(0.45, 0.62), "Inbox");
        act("double-click", CursorAction::Click { button: MouseButton::Left, count: 2 }, 900);
        go(at(0.25, 0.6), "scroll");
        act("scroll", CursorAction::Scroll { dx: 0, dy: 5 }, 900);
        let (from, to) = (at(0.25, 0.6), at(0.55, 0.75));
        go(from, "drag");
        let ms = drag_travel_ms(from, to);
        act("drag", CursorAction::Drag { to, ms }, 140 + ms + 600);
        let keys = KeyCombo::parse("cmd+shift+s").map(|k| k.caps()).unwrap_or_default();
        act("key", CursorAction::Key { keys, repeat: 1 }, 1700);
        act("type", CursorAction::Type { chars: 42 }, 1800);
        go(at(0.5, 0.45), "Look here");
        act("point", CursorAction::Point, 1400);
    }
    println!("demo done");
}

/// The companion scene of `--demo`: a pretend pointer and voice (nothing
/// real moves): listening with a waveform, thinking, flying off to press,
/// coming back, then speaking with a caption.
fn demo_companion(app: &tauri::AppHandle, ov: &dyn Overlay, m: &MonitorRect, stamp: &dyn Fn() -> f64) {
    use tauri::Emitter;
    let at = |fx: f64, fy: f64| Point::new(m.x + m.width * fx, m.y + m.height * fy);
    let voice = |state: &str| {
        let _ = app.emit("pindot:voice", serde_json::json!({ "state": state, "notice": null }));
    };
    let mut t = 0.0f64;
    // Feed the pretend pointer (drifting a little) and level for `secs`.
    let mut mate = |secs: f64, talking: bool| {
        let end = t + secs;
        while t < end {
            let p = at(0.36 + 0.05 * (t * 0.7).sin(), 0.48 + 0.03 * (t * 1.1).sin());
            let level = if talking { 0.01 + 0.09 * (t * 9.0).sin().abs() * (0.5 + 0.5 * (t * 2.3).sin()) } else { 0.0 };
            let _ = app.emit("pindot:companion", serde_json::json!({ "x": p.x, "y": p.y, "level": level }));
            std::thread::sleep(Duration::from_millis(16));
            t += 0.016;
        }
    };
    println!("{:.3} demo: companion listening", stamp());
    voice("listening");
    mate(2.6, true);
    println!("{:.3} demo: companion thinking", stamp());
    voice("thinking");
    mate(1.4, false);
    println!("{:.3} demo: companion acting", stamp());
    voice("acting");
    let d = ov.move_cursor(at(0.62, 0.3), Some("Send".into()));
    mate(d.as_secs_f64(), false);
    ov.set_acting(true);
    ov.action(&CursorAction::Press);
    mate(0.6, false);
    ov.set_acting(false);
    println!("{:.3} demo: companion returning", stamp());
    mate(1.8, false);
    println!("{:.3} demo: companion speaking", stamp());
    voice("speaking");
    let _ = app.emit(
        "pindot:caption",
        serde_json::json!({ "seq": 1, "text": "Done. I pressed Send, so your message to Maya is on its way." }),
    );
    mate(4.2, false);
    let _ = app.emit("pindot:caption-end", serde_json::json!({ "seq": 1, "interrupted": false }));
    voice("idle");
    println!("{:.3} demo: companion idle", stamp());
    mate(2.5, false);
    std::thread::sleep(Duration::from_millis(1800));
}
