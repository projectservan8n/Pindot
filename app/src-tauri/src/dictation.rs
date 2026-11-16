//! Dictation into the chat box: the app side.
//!
//! The mic button (and the hold-to-talk key while the chat box is focused)
//! records, the words are recognised locally by `pindot_core::voice::dictation`
//! and sent to the chat box as an event. Nothing is sent to the assistant,
//! nothing is spoken and no audio is kept.
//!
//! One `DictationHub` is managed by the app. The voice loop's hotkey thread
//! asks it first (`hotkey`): with the chat box focused the hub takes the key
//! press, so a spoken turn does not start. The microphone is shared through
//! `MicArbiter`, so even a race can never record both at once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pindot_core::platform::{self, HotkeyEvent, Permission, PermissionState};
use pindot_core::voice::audio::{self, CpalMicrophone, Microphone};
use pindot_core::voice::dictation::{Backend, Dictation, Event, HotkeyRouter, Problem, Route, Sink, State};
use pindot_core::voice::engines::{SpeechToText, load_stt};
use pindot_core::voice::models::{self, ModelKind};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::ui::{Ui, model_ready};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Button,
    Hotkey,
}

impl Source {
    fn word(self) -> &'static str {
        match self {
            Source::Button => "button",
            Source::Hotkey => "hotkey",
        }
    }
}

pub struct DictationHub {
    app: AppHandle,
    d: Dictation,
    focused: AtomicBool,
    /// The running dictation's id and where it started. The lock is held
    /// while one begins, so no event can show before its source is known.
    current: Arc<Mutex<Option<(u64, Source)>>>,
    routing: Mutex<HotkeyRouter>,
    stop: Arc<AtomicBool>,
}

struct AppBackend {
    app: AppHandle,
}

impl Backend for AppBackend {
    fn check(&self) -> Result<String, (Problem, String)> {
        let s = self.app.state::<Ui>().settings().effective();
        let root = models::models_dir().ok();
        if !model_ready(&s.voice.stt_model, ModelKind::SpeechToText, root.as_deref()) {
            return Err((
                Problem::NoModel,
                "There is no speech model yet. Download it under Setup, then dictation works.".into(),
            ));
        }
        let mic = platform::permission_report().into_iter().find(|p| p.permission == Permission::Microphone);
        if let Some(p) = mic
            && matches!(p.state, PermissionState::Denied | PermissionState::Restricted | PermissionState::NotGranted)
        {
            return Err((
                Problem::NoMic,
                p.note.clone().unwrap_or_else(|| "Microphone access is off. Turn it on in the system settings, then try again.".into()),
            ));
        }
        Ok(format!("{}|{}", s.voice.stt_model, root.map(|r| r.display().to_string()).unwrap_or_default()))
    }

    fn load_stt(&self) -> anyhow::Result<Box<dyn SpeechToText>> {
        let s = self.app.state::<Ui>().settings().effective();
        let root = models::models_dir()?;
        let dir = models::resolve(&s.voice.stt_model, ModelKind::SpeechToText, &root)?;
        load_stt(&dir)
    }

    fn open_mic(&self) -> Box<dyn Microphone> {
        let s = self.app.state::<Ui>().settings().effective();
        Box::new(CpalMicrophone::with_device(s.voice.input_device.clone(), None))
    }
}

impl DictationHub {
    pub fn new(app: &AppHandle) -> std::io::Result<Arc<Self>> {
        let current: Arc<Mutex<Option<(u64, Source)>>> = Arc::default();
        let (a, c) = (app.clone(), current.clone());
        let sink: Sink = Arc::new(move |e| {
            let source = c.lock().unwrap().map(|(_, s)| s).unwrap_or(Source::Button);
            let payload = match e {
                Event::State { id, state } => json!({ "kind": "state", "id": id, "state": state }),
                Event::Text { id, text } => json!({ "kind": "text", "id": id, "text": text }),
                Event::Failed { id, problem, message } => json!({ "kind": "error", "id": id, "problem": problem, "message": message }),
                Event::Cancelled { id } => json!({ "kind": "cancelled", "id": id }),
            };
            let mut payload = payload;
            payload["source"] = json!(source.word());
            let _ = a.emit("pindot:dictation", payload);
        });
        let d = Dictation::start(Arc::new(AppBackend { app: app.clone() }), audio::mic_arbiter(), sink)?;
        let hub = Arc::new(Self {
            app: app.clone(),
            d,
            focused: AtomicBool::new(false),
            current,
            routing: Mutex::default(),
            stop: Arc::new(AtomicBool::new(false)),
        });
        hub.level_ticker(app.clone())?;
        Ok(hub)
    }

    /// While listening, tell the screen how loud it is (about 12 times a
    /// second), so the button can pulse with the voice.
    fn level_ticker(self: &Arc<Self>, app: AppHandle) -> std::io::Result<()> {
        let hub = Arc::downgrade(self);
        let stop = self.stop.clone();
        std::thread::Builder::new().name("pindot-dictation-level".into()).spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let Some(h) = hub.upgrade() else { break };
                if h.d.state() == State::Listening {
                    let _ = app.emit("pindot:dictation", json!({ "kind": "level", "level": audio::mic_level() }));
                    drop(h);
                    std::thread::sleep(Duration::from_millis(80));
                } else {
                    drop(h);
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        })?;
        Ok(())
    }

    fn begin(&self, source: Source) -> Result<u64, String> {
        let mut cur = self.current.lock().unwrap();
        match self.d.begin() {
            Ok(id) => {
                *cur = Some((id, source));
                Ok(id)
            }
            Err((_, m)) => Err(m),
        }
    }

    /// The hold-to-talk key, offered to dictation before the voice loop.
    /// `true` = taken here: do not start or end a spoken turn.
    pub fn hotkey(&self, e: HotkeyEvent) -> bool {
        let mut r = self.routing.lock().unwrap();
        match e {
            HotkeyEvent::Down => match r.down(self.focused.load(Ordering::SeqCst), || self.begin(Source::Hotkey)) {
                Route::Voice => false,
                Route::Taken => true,
                Route::TakenRefused(m) => {
                    // Still the box's key: say why, and stay quiet to the voice loop.
                    let _ = self.app.emit(
                        "pindot:dictation",
                        json!({ "kind": "error", "id": 0, "problem": Problem::Busy, "message": m, "source": "hotkey" }),
                    );
                    true
                }
            },
            HotkeyEvent::Up => {
                let (route, id) = r.up();
                if let Some(id) = id {
                    self.d.finish(id);
                }
                route != Route::Voice
            }
            HotkeyEvent::Cancel => {
                let (route, was_held) = r.cancel();
                if was_held {
                    self.d.cancel();
                }
                route != Route::Voice
            }
        }
    }
}

#[tauri::command]
pub fn dictation_start(hub: tauri::State<'_, Arc<DictationHub>>) -> Result<u64, String> {
    hub.begin(Source::Button)
}

#[tauri::command]
pub fn dictation_stop(hub: tauri::State<'_, Arc<DictationHub>>, id: u64) {
    hub.d.finish(id);
}

#[tauri::command]
pub fn dictation_cancel(hub: tauri::State<'_, Arc<DictationHub>>) {
    hub.d.cancel();
}

/// The chat box got or lost focus. Only while it has focus does the
/// hold-to-talk key dictate into it.
#[tauri::command]
pub fn dictation_focus(hub: tauri::State<'_, Arc<DictationHub>>, focused: bool) {
    hub.focused.store(focused, Ordering::SeqCst);
}

impl Drop for DictationHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
