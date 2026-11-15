//! The tray icon, the main window (Setup, Settings, Activity) and the
//! commands behind it, plus when the voice loop runs.
//!
//! Every command that changes something outside Pindot (a permission
//! request, opening System Settings, `claude mcp add/remove`) only runs from
//! a button the user pressed; nothing here calls them on its own.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pindot_core::brain::config::{write_hook_settings, write_mcp_config};
use pindot_core::brain::{self, AuthState};
use pindot_core::connect::{self, ConnectStatus};
use pindot_core::platform::{self, HotkeyBinding, Os, Permission, PermissionState, PermissionStatus};
use pindot_core::server::activity::{self, ActivityEntry, ActivityFilter};
use pindot_core::server::{RunningServer, ServerState};
use pindot_core::settings::Settings;
use pindot_core::voice::audio::{AudioSink, CpalSpeaker};
use pindot_core::voice::engines::{PhononTts, TextToSpeech};
use pindot_core::voice::models::{self, DownloadOptions, ModelKind, Progress};
use pindot_core::voice::phonon_voices::{self, PhononVoice};
use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::voice::{self, VoiceApp, VoiceState};

/// The label of the Setup / Settings / Activity window.
pub const MAIN: &str = "main";
/// Emitted to the main window when something it shows changed.
pub const STATE_EVENT: &str = "pindot:state";
pub const NAVIGATE_EVENT: &str = "pindot:navigate";
pub const DOWNLOAD_EVENT: &str = "pindot:download";

/// App-wide UI state (managed by Tauri).
pub struct Ui {
    pub capturable: bool,
    /// `--voice`: start hold-to-talk even before Setup is finished.
    pub force_voice: bool,
    /// `--model` for voice turns.
    pub model: Option<String>,
    /// The settings as saved (environment overrides not applied).
    settings: Mutex<Settings>,
    /// How the settings file loaded (first start, migrated, unreadable);
    /// cleared once a save succeeds.
    load_note: Mutex<String>,
    server_error: Mutex<Option<String>>,
    voice: Mutex<Option<Arc<VoiceApp>>>,
    /// Why hold-to-talk is off, when it is.
    voice_note: Mutex<Option<String>>,
    /// One voice (re)start at a time.
    voice_lock: Mutex<()>,
    download: Mutex<Option<Arc<AtomicBool>>>,
    tray_status: Mutex<Option<MenuItem<Wry>>>,
    tray_voice: Mutex<Option<CheckMenuItem<Wry>>>,
    /// Windows, while hold-to-talk is off for a reason: a key hook that
    /// only says why when the key is held (see `start_off_notice`).
    off_hotkey: Mutex<Option<platform::ListenerHandle>>,
}

impl Ui {
    pub fn new(capturable: bool, force_voice: bool, model: Option<String>, settings: Settings, load_note: String) -> Self {
        Self {
            capturable,
            force_voice,
            model,
            settings: Mutex::new(settings),
            load_note: Mutex::new(load_note),
            server_error: Mutex::new(None),
            voice: Mutex::new(None),
            voice_note: Mutex::new(None),
            voice_lock: Mutex::new(()),
            download: Mutex::new(None),
            tray_status: Mutex::new(None),
            tray_voice: Mutex::new(None),
            off_hotkey: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_server_error(&self, e: String) {
        *self.server_error.lock().unwrap() = Some(e);
    }

    /// Keep a voice loop started elsewhere (the test path) so quitting
    /// closes it.
    pub fn adopt_voice(&self, v: Arc<VoiceApp>) {
        *self.voice.lock().unwrap() = Some(v);
    }

    /// Stop the voice loop (hotkey tap, audio, the `claude` child).
    pub fn shutdown_voice(&self) {
        drop(self.off_hotkey.lock().unwrap().take());
        if let Some(v) = self.voice.lock().unwrap().take() {
            v.shutdown();
        }
    }

    fn voice_state(&self) -> Option<VoiceState> {
        self.voice.lock().unwrap().as_ref().map(|v| v.state())
    }
}

// ------------------------------------------------------------- window, tray

/// Show the main window on `view` (`setup`, `settings` or `activity`).
/// `focus` is false when Pindot opens it by itself at launch, so it never
/// takes the keyboard from the app the user is in.
pub fn open_main(app: &AppHandle, view: &str, focus: bool) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = app.emit_to(MAIN, NAVIGATE_EVENT, view);
        w.show()?;
        if focus {
            front_with_focus(app);
        } else {
            front_without_focus(app);
        }
        return Ok(());
    }
    let capturable = app.state::<Ui>().capturable;
    let w = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App(format!("index.html#{view}").into()))
        .title("Pindot")
        .inner_size(820.0, 640.0)
        .min_inner_size(640.0, 460.0)
        .center()
        .focused(focus)
        // Pindot's own windows stay out of Claude's screenshots.
        .content_protected(!capturable)
        .build()?;
    if let (Ok(p), Ok(z), Ok(scale)) = (w.outer_position(), w.outer_size(), w.scale_factor()) {
        let (p, z) = (p.to_logical::<f64>(scale), z.to_logical::<f64>(scale));
        println!("window {MAIN} ({view}) at ({:.0}, {:.0}) size {:.0}x{:.0} pt", p.x, p.y, z.width, z.height);
    }
    if focus {
        front_with_focus(app);
    } else {
        front_without_focus(app);
    }
    Ok(())
}

/// The user asked for the window (tray menu): activate Pindot and give the
/// window the keyboard. Queued on the main thread, so it runs after the
/// tray menu has closed.
fn front_with_focus(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window(MAIN)
                && let Ok(ns) = w.ns_window()
            {
                // SAFETY: Tauri's live NSWindow, on the main thread.
                unsafe { pindot_core::platform::macos::window::activate_and_focus(ns) };
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.set_focus();
    }
}

/// Show the main window above other apps' windows without taking the
/// keyboard from the app the user is in.
fn front_without_focus(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window(MAIN)
                && let Ok(ns) = w.ns_window()
            {
                use pindot_core::platform::macos::window::{order_front_without_focus, window_number};
                // SAFETY: Tauri's live NSWindow, on the main thread.
                unsafe { order_front_without_focus(ns) };
                println!("window {MAIN}: in front without focus (window id {})", unsafe { window_number(ns) });
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let ui = app.state::<Ui>();
    let status = MenuItem::with_id(app, "status", "Pindot: starting…", false, None::<&str>)?;
    let setup = MenuItem::with_id(app, "setup", "Setup…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let activity = MenuItem::with_id(app, "activity", "Activity…", true, None::<&str>)?;
    let voice =
        CheckMenuItem::with_id(app, "voice", "Hold-to-talk", true, ui.settings().voice_enabled, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Pindot", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &setup,
            &settings,
            &activity,
            &PredefinedMenuItem::separator(app)?,
            &voice,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    TrayIconBuilder::with_id("pindot")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Pindot")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            id @ ("setup" | "settings" | "activity") => {
                if let Err(e) = open_main(app, id, true) {
                    tracing::warn!("opening the {id} window: {e}");
                }
            }
            "voice" => {
                let on = !app.state::<Ui>().settings().voice_enabled;
                if let Err(e) = set_voice_enabled(app, on) {
                    tracing::warn!("hold-to-talk on/off: {e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    *ui.tray_status.lock().unwrap() = Some(status);
    *ui.tray_voice.lock().unwrap() = Some(voice);
    Ok(())
}

fn refresh_tray(app: &AppHandle) {
    let ui = app.state::<Ui>();
    let s = ui.settings();
    let label = s.effective().hotkey.label_for(Os::current());
    let text = match (ui.voice_state(), ui.voice_note.lock().unwrap().clone()) {
        (Some(VoiceState::Running), _) => format!("Pindot: hold {label} to talk"),
        (Some(VoiceState::Starting), _) => "Pindot: hold-to-talk starting…".into(),
        (Some(VoiceState::Failed(e)), _) => format!("Pindot: hold-to-talk failed ({})", short(&e, 60)),
        (_, Some(note)) => format!("Pindot: hold-to-talk off ({})", short(&note, 60)),
        _ => "Pindot: hold-to-talk off".into(),
    };
    if let Some(i) = ui.tray_status.lock().unwrap().as_ref() {
        let _ = i.set_text(text);
    }
    if let Some(i) = ui.tray_voice.lock().unwrap().as_ref() {
        let _ = i.set_checked(s.voice_enabled);
        let _ = i.set_text(format!("Hold-to-talk ({label})"));
    }
}

fn short(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or_default();
    activity::truncate(first, max)
}

fn changed(app: &AppHandle) {
    refresh_tray(app);
    let _ = app.emit_to(MAIN, STATE_EVENT, ());
}

// ------------------------------------------------------------- voice

/// What keeps hold-to-talk from starting, in words for the user. Empty =
/// ready. Read-only: the permission checks never prompt.
pub fn voice_blockers(app: &AppHandle) -> Vec<String> {
    let ui = app.state::<Ui>();
    let s = ui.settings().effective();
    let mut out = Vec::new();
    if !ui.force_voice {
        if !s.voice_enabled {
            out.push("hold-to-talk is turned off".to_string());
            return out;
        }
        if !s.onboarding_done {
            out.push("finish Setup first".into());
        }
    }
    if app.try_state::<RunningServer>().is_none() {
        out.push("the MCP server is not running".into());
    }
    let root = models::models_dir().ok();
    for (id, kind) in [(&s.voice.stt_model, ModelKind::SpeechToText), (&s.voice.tts_model, ModelKind::TextToSpeech)] {
        if !model_ready(id, kind, root.as_deref()) {
            out.push(format!("the {} model is not downloaded", kind_word(kind)));
        }
    }
    let claude = claude_info();
    if claude.path.is_none() {
        out.push("Claude Code (`claude`) is not installed".into());
    } else if !claude.signed_in && claude.problem.is_none() {
        out.push("Claude Code is not signed in (run `claude login`)".into());
    }
    for p in platform::permission_report() {
        let needed = matches!(p.permission, Permission::Microphone | Permission::Accessibility | Permission::InputMonitoring);
        if needed && platform::windows_logic::blocks_voice(p.state, cfg!(windows)) {
            out.push(p.note.clone().unwrap_or_else(|| format!("{} permission is missing", p.name)));
        }
    }
    out
}

fn kind_word(k: ModelKind) -> &'static str {
    match k {
        ModelKind::SpeechToText => "speech-to-text",
        ModelKind::TextToSpeech => "text-to-speech",
    }
}

fn model_ready(setting: &str, kind: ModelKind, root: Option<&Path>) -> bool {
    let p = Path::new(setting);
    if p.is_absolute() {
        return p.is_dir();
    }
    match (models::find(setting), root) {
        (Some(spec), Some(root)) => spec.kind == kind && models::is_present(spec, root),
        _ => false,
    }
}

/// Stop the voice loop if it runs, then start it again if everything it
/// needs is there. On a background thread; returns at once.
pub fn restart_voice(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("pindot-voice-restart".into()).spawn(move || {
        restart_voice_now(&app);
    });
    if let Err(e) = spawned {
        tracing::warn!("could not restart the voice loop: {e}");
    }
}

pub fn restart_voice_now(app: &AppHandle) {
    let ui = app.state::<Ui>();
    let _one_at_a_time = ui.voice_lock.lock().unwrap();
    ui.shutdown_voice();
    let blockers = voice_blockers(app);
    if blockers.is_empty() {
        let (Some(server), Some(state)) = (app.try_state::<RunningServer>(), app.try_state::<Arc<ServerState>>())
        else {
            return;
        };
        let link = voice::ServerLink {
            mcp_url: server.mcp_url(),
            stop_hook_url: server.stop_hook_url(),
            token: server.token.get().to_string(),
        };
        let opts = voice::VoiceOptions {
            test_wav: None,
            model: ui.model.clone(),
            test_timeout: Duration::from_secs(240),
            exit_when_done: false,
            test_esc: false,
            hotkey: ui.settings().effective().hotkey,
        };
        *ui.voice_note.lock().unwrap() = None;
        let v = voice::start(app, state.inner().clone(), link, opts);
        *ui.voice.lock().unwrap() = Some(v.clone());
        // Let the tray show "running" (or the error) once models are loaded.
        let app2 = app.clone();
        std::thread::spawn(move || {
            for _ in 0..120 {
                std::thread::sleep(Duration::from_millis(500));
                if v.state() != VoiceState::Starting {
                    break;
                }
            }
            changed(&app2);
        });
    } else {
        let note = blockers.join("; ");
        println!("voice: hold-to-talk is off ({note})");
        // Not when the user switched hold-to-talk off themselves.
        #[cfg(windows)]
        if ui.force_voice || ui.settings().effective().voice_enabled {
            start_off_notice(app, &note);
        }
        *ui.voice_note.lock().unwrap() = Some(note);
    }
    changed(app);
}

/// Hold-to-talk is off for a reason (a model is missing, Setup is not
/// finished): when the key is held anyway, say why on the overlay instead
/// of doing nothing. Windows only: a key hook needs no permission there,
/// while on macOS creating a tap without one can itself show a prompt.
#[cfg(windows)]
fn start_off_notice(app: &AppHandle, note: &str) {
    use pindot_core::platform::HotkeyEvent;
    let ui = app.state::<Ui>();
    let (tx, rx) = std::sync::mpsc::channel();
    let Ok(listener) = platform::native().hotkey.start(ui.settings().effective().hotkey, tx) else { return };
    *ui.off_hotkey.lock().unwrap() = Some(listener);
    let (app, text) = (app.clone(), format!("Hold-to-talk is off: {note}"));
    // Ends when the listener is dropped (the channel closes).
    let spawned = std::thread::Builder::new().name("pindot-off-notice".into()).spawn(move || {
        for event in rx {
            if event == HotkeyEvent::Down {
                println!("voice: the hold-to-talk key was held while it is off; the overlay says why");
                let _ = app.emit("pindot:voice", serde_json::json!({ "state": "idle", "notice": text }));
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("no off notice: {e}");
    }
}

fn set_voice_enabled(app: &AppHandle, on: bool) -> Result<(), String> {
    let ui = app.state::<Ui>();
    {
        let mut s = ui.settings.lock().unwrap();
        s.voice_enabled = on;
        s.save().map_err(|e| format!("{e:#}"))?;
    }
    restart_voice(app);
    changed(app);
    Ok(())
}

// ------------------------------------------------------------- commands

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    os: &'static str,
    data_dir: Option<String>,
    settings_path: Option<String>,
    load_note: String,
    server_url: Option<String>,
    server_error: Option<String>,
    /// "running", "starting", "failed", "off".
    voice: &'static str,
    voice_note: Option<String>,
    hotkey_label: String,
    capturable: bool,
    test_switches: bool,
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let ui = app.state::<Ui>();
    let (voice, voice_note) = match ui.voice_state() {
        Some(VoiceState::Running) => ("running", None),
        Some(VoiceState::Starting) => ("starting", None),
        Some(VoiceState::Failed(e)) => ("failed", Some(e)),
        _ => ("off", ui.voice_note.lock().unwrap().clone()),
    };
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: if cfg!(target_os = "macos") { "macos" } else { "windows" },
        data_dir: pindot_core::paths::data_dir().ok().map(|p| p.display().to_string()),
        settings_path: Settings::path().ok().map(|p| p.display().to_string()),
        load_note: ui.load_note.lock().unwrap().clone(),
        server_url: app.try_state::<RunningServer>().map(|s| s.mcp_url()),
        server_error: ui.server_error.lock().unwrap().clone(),
        voice,
        voice_note,
        hotkey_label: ui.settings().effective().hotkey.label_for(Os::current()),
        capturable: ui.capturable,
        test_switches: cfg!(feature = "test-switches"),
    }
}

#[derive(Serialize)]
pub struct ModelChoice {
    id: &'static str,
    display: &'static str,
    license: &'static str,
    size: u64,
}

#[derive(Serialize)]
pub struct SettingsView {
    settings: Settings,
    /// Environment variables that override saved settings right now.
    overrides: Vec<String>,
    stt_models: Vec<ModelChoice>,
    tts_models: Vec<ModelChoice>,
    /// Extra Phonon voices (downloaded when picked).
    tts_voices: &'static [PhononVoice],
    default_port: u16,
    running_port: Option<u16>,
    os: &'static str,
}

const OVERRIDE_VARS: &[&str] = &[
    "PINDOT_PORT",
    "PINDOT_HOTKEY",
    "PINDOT_STT_MODEL",
    "PINDOT_TTS_MODEL",
    "PINDOT_TTS_VOICE",
    "PINDOT_VOICE_MODEL",
    "PINDOT_VOLUME",
    "PINDOT_SPEAK_MODE",
];

#[tauri::command]
pub fn settings_get(app: AppHandle) -> SettingsView {
    let choices = |kind: ModelKind| {
        models::CATALOG
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| ModelChoice { id: m.id, display: m.display, license: m.license, size: m.total_size() })
            .collect()
    };
    SettingsView {
        settings: app.state::<Ui>().settings(),
        overrides: OVERRIDE_VARS.iter().filter(|k| std::env::var_os(k).is_some()).map(|k| k.to_string()).collect(),
        stt_models: choices(ModelKind::SpeechToText),
        tts_models: choices(ModelKind::TextToSpeech),
        tts_voices: phonon_voices::VOICES,
        default_port: pindot_core::DEFAULT_PORT,
        running_port: app.try_state::<RunningServer>().map(|s| s.addr.port()),
        os: if cfg!(target_os = "macos") { "macos" } else { "windows" },
    }
}

#[derive(Serialize)]
pub struct SaveResult {
    notes: Vec<String>,
}

/// Validate, save and apply. Errors are the problems, one per field.
#[tauri::command]
pub fn settings_save(app: AppHandle, settings: Settings) -> Result<SaveResult, Vec<String>> {
    settings.validate()?;
    let ui = app.state::<Ui>();
    let old = ui.settings();
    // The settings screen never changes this one; keep what is saved.
    let mut new = settings;
    new.onboarding_done = old.onboarding_done;
    new.save().map_err(|e| vec![format!("Saving: {e:#}")])?;
    *ui.settings.lock().unwrap() = new.clone();
    ui.load_note.lock().unwrap().clear();
    let mut notes = Vec::new();
    if new.port != old.port {
        notes.push(format!(
            "The MCP server moves to port {} the next time Pindot starts. Press \"Connect to Claude Code\" again after that.",
            new.port
        ));
    }
    if new.hotkey != old.hotkey || new.voice != old.voice || new.voice_enabled != old.voice_enabled {
        restart_voice(&app);
        notes.push("Hold-to-talk restarts with the new settings.".into());
    }
    if new.voice.assistant_name != old.voice.assistant_name {
        send_assistant_name(&app);
    }
    changed(&app);
    Ok(SaveResult { notes })
}

/// Tell the overlays what the assistant is called (the banner says
/// "Rusty is controlling input").
pub fn send_assistant_name(app: &AppHandle) {
    if let Some(ui) = app.try_state::<Ui>() {
        let _ = app.emit("pindot:assistant", ui.settings().voice.assistant_name);
    }
}

#[derive(Serialize)]
pub struct HotkeyCheck {
    ok: bool,
    /// The canonical text to save (`"Right Option"`).
    normalized: Option<String>,
    label: Option<String>,
    caution: Option<&'static str>,
    error: Option<String>,
}

pub fn check_hotkey(text: &str, os: Os) -> HotkeyCheck {
    match text.parse::<HotkeyBinding>().and_then(|b| b.check_for(os).map(|_| b)) {
        Ok(b) => HotkeyCheck {
            ok: true,
            normalized: Some(b.to_string()),
            label: Some(b.label_for(os)),
            caution: b.caution(),
            error: None,
        },
        Err(e) => HotkeyCheck { ok: false, normalized: None, label: None, caution: None, error: Some(e.to_string()) },
    }
}

#[tauri::command]
pub fn hotkey_check(text: String) -> HotkeyCheck {
    check_hotkey(&text, Os::current())
}

#[derive(Serialize, Clone)]
pub struct ClaudeInfo {
    pub path: Option<String>,
    pub version: Option<String>,
    pub signed_in: bool,
    pub method: Option<String>,
    /// The sign-in check itself failed (old CLI, odd output).
    pub problem: Option<String>,
    pub hint: &'static str,
}

/// `claude` on this machine and whether it is signed in. Runs
/// `claude --version` and `claude auth status --json` (both read-only).
pub fn claude_info() -> ClaudeInfo {
    let Some(bin) = brain::find_claude() else {
        return ClaudeInfo {
            path: None,
            version: None,
            signed_in: false,
            method: None,
            // Installed, but only as npm's script (Windows).
            problem: brain::npm_claude_script().map(|_| brain::NPM_INSTALL_HINT.to_string()),
            hint: brain::SIGN_IN_HINT,
        };
    };
    let version = brain::claude_version(&bin);
    let (signed_in, method, problem) = match brain::auth_status(&bin) {
        AuthState::SignedIn { method } => (true, Some(method), None),
        AuthState::SignedOut => (false, None, None),
        AuthState::Unknown(e) => (false, None, Some(e)),
    };
    ClaudeInfo { path: Some(bin.display().to_string()), version, signed_in, method, problem, hint: brain::SIGN_IN_HINT }
}

#[tauri::command]
pub async fn claude_status() -> ClaudeInfo {
    tauri::async_runtime::spawn_blocking(claude_info).await.unwrap_or_else(|e| ClaudeInfo {
        path: None,
        version: None,
        signed_in: false,
        method: None,
        problem: Some(e.to_string()),
        hint: brain::SIGN_IN_HINT,
    })
}

/// Read-only checks; never shows a prompt.
#[tauri::command]
pub fn permissions() -> Vec<PermissionStatus> {
    platform::permission_report()
}

/// FOR THE USER'S BUTTON: may show the system prompt.
#[tauri::command]
pub fn permission_request(permission: Permission) -> PermissionState {
    tracing::info!(?permission, "user asked for a permission");
    platform::request_permission(permission)
}

/// FOR THE USER'S BUTTON: opens the System Settings pane.
#[tauri::command]
pub fn permission_open_settings(permission: Permission) -> Result<(), String> {
    open_url(settings_url(permission))
}

pub fn settings_url(p: Permission) -> &'static str {
    if cfg!(target_os = "windows") { "ms-settings:privacy-microphone" } else { p.settings_url() }
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("/usr/bin/open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer.exe").arg(url).spawn();
    r.map(|_| ()).map_err(|e| format!("opening {url}: {e}"))
}

#[derive(Serialize)]
pub struct ModelInfo {
    id: &'static str,
    display: &'static str,
    kind: ModelKind,
    license: &'static str,
    license_url: &'static str,
    license_text: &'static str,
    source: &'static str,
    size: u64,
    present: bool,
    in_use: bool,
    fallback: Option<&'static str>,
    dir: Option<String>,
}

#[tauri::command]
pub fn models_status(app: AppHandle) -> Vec<ModelInfo> {
    let s = app.state::<Ui>().settings().effective();
    let root = models::models_dir().ok();
    models::CATALOG
        .iter()
        .map(|m| ModelInfo {
            id: m.id,
            display: m.display,
            kind: m.kind,
            license: m.license,
            license_url: m.license_url,
            license_text: m.license_text,
            source: m.source,
            size: m.total_size(),
            present: root.as_deref().is_some_and(|r| models::is_present(m, r)),
            in_use: s.voice.stt_model == m.id || s.voice.tts_model == m.id,
            fallback: m.fallback.map(|f| f.note),
            dir: root.as_ref().map(|r| r.join(m.id).display().to_string()),
        })
        .collect()
}

#[derive(Serialize, Clone)]
pub struct DownloadEvent {
    id: String,
    progress: Option<Progress>,
    /// Set once per model when it ends: Ok(folder) or the error.
    done: Option<Result<String, String>>,
    /// The whole batch ended.
    finished: bool,
}

/// Download (or check and repair) the given catalog models one after the
/// other on a background thread, reporting `pindot:download` events.
/// The user started it after seeing each license.
#[tauri::command]
pub fn models_download(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    let ui = app.state::<Ui>();
    let mut slot = ui.download.lock().unwrap();
    if slot.is_some() {
        return Err("a download is already running".into());
    }
    let specs = ids
        .iter()
        .map(|id| models::find(id).ok_or_else(|| format!("unknown model {id}")))
        .collect::<Result<Vec<_>, _>>()?;
    let root = models::models_dir().map_err(|e| format!("{e:#}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    *slot = Some(cancel.clone());
    drop(slot);
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("pindot-model-download".into())
        .spawn(move || {
            for spec in specs {
                let opts = DownloadOptions { cancel: cancel.clone(), ..DownloadOptions::default() };
                let id = spec.id.to_string();
                let mut report = |p: Progress| {
                    let _ = app2.emit_to(
                        MAIN,
                        DOWNLOAD_EVENT,
                        DownloadEvent { id: id.clone(), progress: Some(p), done: None, finished: false },
                    );
                };
                let r = models::ensure_with(spec, &root, &opts, &mut report)
                    .map(|p| p.display().to_string())
                    .map_err(|e| format!("{e:#}"));
                match &r {
                    Ok(p) => println!("models: {id} ready in {p}"),
                    Err(e) => println!("models: {id} failed: {e}"),
                }
                let stop = r.is_err() && cancel.load(Ordering::SeqCst);
                let _ = app2.emit_to(
                    MAIN,
                    DOWNLOAD_EVENT,
                    DownloadEvent { id: id.clone(), progress: None, done: Some(r), finished: false },
                );
                if stop {
                    break;
                }
            }
            *app2.state::<Ui>().download.lock().unwrap() = None;
            let _ =
                app2.emit_to(MAIN, DOWNLOAD_EVENT, DownloadEvent { id: String::new(), progress: None, done: None, finished: true });
            restart_voice(&app2);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stop the running download; the partial file is kept for a resume.
#[tauri::command]
pub fn models_cancel(app: AppHandle) {
    if let Some(c) = app.state::<Ui>().download.lock().unwrap().as_ref() {
        c.store(true, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
pub struct ConnectView {
    status: ConnectStatus,
    /// The command with the token hidden, for display.
    command: Option<String>,
    remove_command: String,
    claude_found: bool,
    port: Option<u16>,
}

fn server_port_token(app: &AppHandle) -> Option<(u16, String)> {
    app.try_state::<RunningServer>().map(|s| (s.addr.port(), s.token.get().to_string()))
}

/// Reads `~/.claude.json` without changing it.
#[tauri::command]
pub fn connect_status(app: AppHandle) -> ConnectView {
    let pt = server_port_token(&app);
    ConnectView {
        status: match &pt {
            Some((port, token)) => connect::status(*port, token),
            None => ConnectStatus::Unknown { error: "the MCP server is not running".into() },
        },
        command: pt.as_ref().map(|(port, token)| connect::shell_line(&connect::add_args(*port, token), true)),
        remove_command: connect::shell_line(&connect::remove_args(), true),
        claude_found: brain::find_claude().is_some(),
        port: pt.map(|(p, _)| p),
    }
}

/// The full command with the token, for "Copy command" (to run it by hand).
#[tauri::command]
pub fn connect_command(app: AppHandle) -> Result<String, String> {
    let (port, token) = server_port_token(&app).ok_or("the MCP server is not running")?;
    Ok(connect::shell_line(&connect::add_args(port, &token), false))
}

/// FOR THE USER'S BUTTON: `claude mcp add --scope user desktop ...`.
/// Changes the user's own Claude Code config.
#[tauri::command]
pub async fn connect_claude(app: AppHandle) -> Result<String, String> {
    let (port, token) = server_port_token(&app).ok_or("the MCP server is not running")?;
    let bin = brain::find_claude().ok_or("Claude Code (`claude`) is not installed")?;
    let args = connect::add_args(port, &token);
    tracing::info!("user pressed Connect: {}", connect::shell_line(&args, true));
    let out = tauri::async_runtime::spawn_blocking(move || connect::run(&bin, &args))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"));
    changed(&app);
    out
}

/// FOR THE USER'S BUTTON: `claude mcp remove --scope user desktop`.
#[tauri::command]
pub async fn disconnect_claude(app: AppHandle) -> Result<String, String> {
    let bin = brain::find_claude().ok_or("Claude Code (`claude`) is not installed")?;
    let args = connect::remove_args();
    tracing::info!("user pressed Disconnect: {}", connect::shell_line(&args, true));
    let out = tauri::async_runtime::spawn_blocking(move || connect::run(&bin, &args))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"));
    changed(&app);
    out
}

/// New bearer token (file mode 0600): the server switches to it at once,
/// Pindot's own `claude` configs are rewritten and hold-to-talk restarts.
/// A terminal `claude` registered with the old token must Connect again.
#[tauri::command]
pub fn token_regenerate(app: AppHandle) -> Result<String, String> {
    let server = app.try_state::<RunningServer>().ok_or("the MCP server is not running")?;
    let token = pindot_core::paths::regenerate_token().map_err(|e| format!("{e:#}"))?;
    server.token.set(&token);
    write_mcp_config(&server.mcp_url(), &token).map_err(|e| format!("{e:#}"))?;
    write_hook_settings(&server.stop_hook_url(), &token).map_err(|e| format!("{e:#}"))?;
    println!("token: regenerated by the user; the old token stopped working");
    restart_voice(&app);
    changed(&app);
    Ok("New token in use; the old one no longer works.".into())
}

#[derive(Serialize)]
pub struct ActivityView {
    path: Option<String>,
    entries: Vec<ActivityEntry>,
    sessions: Vec<String>,
    tools: Vec<String>,
    total: usize,
}

/// Every logged call (current and rotated file), newest last, filtered.
#[tauri::command]
pub fn activity_all(app: AppHandle, session: Option<String>, tool: Option<String>) -> ActivityView {
    let Some(state) = app.try_state::<Arc<ServerState>>() else {
        return ActivityView { path: None, entries: vec![], sessions: vec![], tools: vec![], total: 0 };
    };
    let all = state.activity.all(20_000);
    let (sessions, tools) = activity::facets(&all);
    let total = all.len();
    let entries = ActivityFilter { session, tool }.apply(all);
    ActivityView {
        path: state.activity.path().map(|p| p.display().to_string()),
        entries,
        sessions,
        tools,
        total,
    }
}

/// The user confirmed "Clear": empties the log files.
#[tauri::command]
pub fn activity_clear(app: AppHandle) -> Result<(), String> {
    let state = app.try_state::<Arc<ServerState>>().ok_or("the MCP server is not running")?;
    state.activity.clear().map_err(|e| e.to_string())?;
    println!("activity: log cleared by the user");
    Ok(())
}

#[tauri::command]
pub fn onboarding_finish(app: AppHandle) -> Result<(), String> {
    let ui = app.state::<Ui>();
    {
        let mut s = ui.settings.lock().unwrap();
        s.onboarding_done = true;
        s.save().map_err(|e| format!("{e:#}"))?;
    }
    restart_voice(&app);
    Ok(())
}

#[tauri::command]
pub fn voice_set_enabled(app: AppHandle, on: bool) -> Result<(), String> {
    set_voice_enabled(&app, on)
}

/// Why hold-to-talk is off (empty = it can run). Read-only.
#[tauri::command]
pub async fn voice_check(app: AppHandle) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || voice_blockers(&app)).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_text_is_checked_for_each_os() {
        let mac = check_hotkey("right option", Os::MacOs);
        assert!(mac.ok);
        assert_eq!(mac.normalized.as_deref(), Some("Right Option"));
        assert_eq!(mac.label.as_deref(), Some("Right Option"));
        assert!(mac.caution.is_none());

        let win = check_hotkey("right option", Os::Windows);
        assert_eq!(win.label.as_deref(), Some("Right Alt"));

        let fn_win = check_hotkey("Fn", Os::Windows);
        assert!(!fn_win.ok);
        assert!(fn_win.error.unwrap().contains("Windows"));

        let lone = check_hotkey("ctrl", Os::MacOs);
        assert!(lone.ok && lone.caution.is_some());

        let bad = check_hotkey("ctrl+q", Os::MacOs);
        assert!(!bad.ok && bad.error.unwrap().contains("not a modifier"));

        assert_eq!(check_hotkey("ctrl + cmd", Os::MacOs).label.as_deref(), Some("Ctrl+Cmd"));
        assert_eq!(check_hotkey("ctrl+win", Os::Windows).label.as_deref(), Some("Ctrl+Win"));
    }

    #[test]
    fn model_setting_can_be_a_folder() {
        let dir = std::env::temp_dir();
        assert!(model_ready(&dir.display().to_string(), ModelKind::SpeechToText, None));
        assert!(!model_ready("/no/such/pindot/model", ModelKind::SpeechToText, None));
        assert!(!model_ready("no-such-id", ModelKind::SpeechToText, Some(&dir)));
    }

    #[test]
    fn settings_panes_are_right_for_the_os() {
        let url = settings_url(Permission::Microphone);
        if cfg!(target_os = "macos") {
            assert_eq!(url, "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
        } else {
            assert_eq!(url, "ms-settings:privacy-microphone");
        }
    }
}

/// Download a Phonon voice (256 KB, Apache-2.0, pinned and checked) when the
/// user picks it in Settings.
#[tauri::command]
pub async fn voice_download(name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = models::models_dir().map_err(|e| format!("{e:#}"))?;
        phonon_voices::ensure(Some(&name), &root).map(|_| ()).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

static PREVIEWING: AtomicBool = AtomicBool::new(false);

/// Speak a short sample with a voice (downloading it first if needed), at
/// the volume and with the assistant name on screen, with the saved speech
/// model.
#[tauri::command]
pub async fn voice_preview(app: AppHandle, name: Option<String>, volume: f32, assistant: Option<String>) -> Result<(), String> {
    if PREVIEWING.swap(true, Ordering::SeqCst) {
        return Err("A preview is already playing.".into());
    }
    let settings = app.state::<Ui>().settings().voice;
    let res = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        let root = models::models_dir()?;
        let voice = phonon_voices::ensure(name.as_deref(), &root)?;
        let tts_dir = models::resolve(&settings.tts_model, ModelKind::TextToSpeech, &root)?;
        let tts = PhononTts::load(&tts_dir, voice.as_ref().map(|(n, p)| (*n, p.as_path())))?;
        let who = assistant
            .as_deref()
            .map(str::trim)
            .filter(|a| pindot_core::voice::assistant_problems(a, "").is_empty())
            .unwrap_or(pindot_core::voice::DEFAULT_ASSISTANT_NAME);
        let text = format!("Hi, I'm {who}. This is how I'll sound when I answer you.");
        let mut pcm = Vec::new();
        tts.speak(&text, &mut |c| {
            pcm.extend_from_slice(c);
            true
        })?;
        let mut once = Some(pcm);
        CpalSpeaker { volume: volume.clamp(0.0, 1.0) }.play(tts.sample_rate(), &mut || once.take(), &|| false, &mut || {})
    })
    .await;
    PREVIEWING.store(false, Ordering::SeqCst);
    res.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))
}
