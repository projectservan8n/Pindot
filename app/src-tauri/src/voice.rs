//! The voice loop inside the app: hold the hotkey (Fn on macOS), speak,
//! release. Pindot transcribes the words locally, types them into ONE
//! persistent interactive `claude` (see `pindot_core::brain::persistent`),
//! shows listening / thinking / acting / speaking on the overlay and speaks
//! the reply locally. The parts live in `pindot_core::voice`.
//!
//! `--test-voice WAV` is TEST ONLY and logged loudly: the microphone is
//! replaced by the WAV file, the hotkey is pressed and released
//! synthetically once `claude` is ready, the spoken reply is written to WAV
//! files (and played at low volume), the latency of each stage is printed,
//! and the app quits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pindot_core::brain::persistent::{BrainConfig, PersistentBrain, session_dir};
use pindot_core::platform::{HotkeyBinding, HotkeyEvent, ListenerHandle};
use pindot_core::server::ServerState;
use pindot_core::server::control::ControlEvent;
use pindot_core::voice::audio::{self, AudioSink, CpalMicrophone, CpalSpeaker, Microphone, WavMicrophone, WavSink};
use pindot_core::voice::engines::{PhononTts, STT_SAMPLE_RATE, TextToSpeech, load_stt};
use pindot_core::voice::machine::{Display, Input, Machine};
use pindot_core::voice::models::{self, ModelKind};
use pindot_core::voice::phonon_voices;
use pindot_core::voice::runner::{self, LiveIo, Metric, Recorder, StatusSink, VoiceHandle, VoiceRunner, VoiceStatus};
use pindot_core::voice::speaker::{Speaker, SpeakerEvent};
use pindot_core::voice::{VoiceSettings, voice_system_prompt};
use tauri::{AppHandle, Emitter, Manager};

/// Playback volume of the test path (the "one play at low volume").
const TEST_VOLUME: f32 = 0.2;
/// How long the test holds the hotkey.
const TEST_HOLD: Duration = Duration::from_millis(1500);
const TEST_READY_TIMEOUT: Duration = Duration::from_secs(120);

pub struct ServerLink {
    pub mcp_url: String,
    pub stop_hook_url: String,
    pub token: String,
}

pub struct VoiceOptions {
    /// TEST ONLY: use this WAV instead of the microphone and press the
    /// hotkey synthetically.
    pub test_wav: Option<PathBuf>,
    /// `claude --model` for voice turns (overrides the voice setting).
    pub model: Option<String>,
    /// Test: how long the whole spoken interaction may take.
    pub test_timeout: Duration,
    /// Test: quit when done (otherwise keep running, e.g. to clean up).
    pub exit_when_done: bool,
    /// Test: press Esc during the first turn (once a tool ran and a
    /// test-approved app is in front), then speak the request again.
    pub test_esc: bool,
    /// The hold-to-talk chord (from the settings).
    pub hotkey: HotkeyBinding,
    /// Load the voice models and watch the hold-to-talk key. When false the
    /// assistant still runs for Chat: typed messages, answers on screen.
    pub with_voice: bool,
}

/// Keeps the voice loop alive. `shutdown` stops the hotkey tap, speech, the
/// recorder and the `claude` child.
#[derive(Default)]
pub struct VoiceApp {
    running: Mutex<Option<Running>>,
    closed: AtomicBool,
    /// Why setup failed (models, `claude`, audio), for the settings screen.
    failed: Mutex<Option<String>>,
}

struct Running {
    /// Hold-to-talk (models, microphone, speech) is part of this run.
    voice: bool,
    hotkey: Option<ListenerHandle>,
    runner: VoiceRunner,
    /// Typed chat messages go in here.
    handle: VoiceHandle,
    /// Feeds the companion cursor; `None` when that setting is off.
    _companion: Option<Companion>,
}

/// While a voice request is in progress (and a moment after), sends the user's pointer
/// and the mic level to the overlay about 60 times a second, so the AI cursor can sit beside
/// the user's pointer and show a live waveform. Idle, it only checks the
/// state 10 times a second and sends nothing.
struct Companion {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const COMPANION_LINGER: Duration = Duration::from_millis(2500);

impl Companion {
    fn start(app: AppHandle, shown: Arc<Mutex<Display>>) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new().name("pindot-companion".into()).spawn(move || {
            let mut active_until: Option<Instant> = None;
            let mut sent: Option<(f64, f64, f32)> = None;
            let mut sent_at = Instant::now();
            while !s.load(Ordering::SeqCst) {
                let state = *shown.lock().unwrap();
                let now = Instant::now();
                if state != Display::Idle {
                    active_until = Some(now + COMPANION_LINGER);
                }
                if !active_until.is_some_and(|t| now < t) {
                    sent = None;
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                if let Some(p) = pindot_core::platform::pointer_location() {
                    let level = if state == Display::Listening { audio::mic_level() } else { 0.0 };
                    let same = sent.is_some_and(|(x, y, l)| {
                        (x - p.x).abs() < 0.5 && (y - p.y).abs() < 0.5 && (l - level).abs() < 0.002
                    });
                    // Only changes, plus a heartbeat so a new panel catches up.
                    if !same || sent_at.elapsed() > Duration::from_millis(500) {
                        let _ = app.emit("pindot:companion", serde_json::json!({ "x": p.x, "y": p.y, "level": level }));
                        sent = Some((p.x, p.y, level));
                        sent_at = Instant::now();
                    }
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl VoiceApp {
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let running = self.running.lock().unwrap().take();
        if let Some(r) = running {
            // The tap first, so no new press arrives while the rest stops.
            drop(r.hotkey);
            drop(r.runner);
            println!("voice loop stopped (hotkey tap, audio and claude closed)");
        }
    }

    /// Running (the hotkey tap may still be off, see the log), starting,
    /// failed or stopped.
    pub fn state(&self) -> VoiceState {
        if let Some(e) = self.failed.lock().unwrap().clone() {
            return VoiceState::Failed(e);
        }
        if self.closed.load(Ordering::SeqCst) {
            return VoiceState::Stopped;
        }
        match self.running.lock().unwrap().as_ref() {
            Some(r) if !r.voice => VoiceState::ChatOnly,
            Some(r) if r.hotkey.is_some() => VoiceState::Running,
            Some(_) => VoiceState::Failed("the hold-to-talk key could not be watched (see the log)".into()),
            None => VoiceState::Starting,
        }
    }

    /// Hold-to-talk and speech are part of the running loop (not just Chat).
    pub fn speech_on(&self) -> bool {
        self.running.lock().unwrap().as_ref().is_some_and(|r| r.voice)
    }

    pub fn handle(&self) -> Option<VoiceHandle> {
        self.running.lock().unwrap().as_ref().map(|r| r.handle.clone())
    }

    fn metrics(&self) -> Vec<Metric> {
        self.running.lock().unwrap().as_ref().map(|r| r.runner.metrics()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceState {
    Starting,
    /// Hold-to-talk and speech are running (and Chat).
    Running,
    /// Only the assistant for Chat runs; hold-to-talk is off.
    ChatOnly,
    Failed(String),
    Stopped,
}

/// Start the voice loop on a background thread (models may need a download
/// first). Returns at once.
pub fn start(app: &AppHandle, state: Arc<ServerState>, link: ServerLink, opts: VoiceOptions) -> Arc<VoiceApp> {
    let voice = Arc::new(VoiceApp::default());
    let (v, app) = (voice.clone(), app.clone());
    let spawned = std::thread::Builder::new().name("pindot-voice-setup".into()).spawn(move || {
        let test = opts.test_wav.is_some();
        let exit_when_done = opts.exit_when_done;
        if let Err(e) = setup(&app, &v, state, link, opts) {
            println!("voice: could not start: {e:#}");
            *v.failed.lock().unwrap() = Some(format!("{e:#}"));
            show(&app, Display::Idle, Some(format!("Voice is off: {e:#}")));
            if test {
                println!("test-voice: RESULT CHECK OUTPUT");
                if exit_when_done {
                    app.exit(1);
                }
            }
        }
    });
    if let Err(e) = spawned {
        println!("voice: could not start a thread: {e}");
    }
    voice
}

fn show(app: &AppHandle, state: Display, notice: Option<String>) {
    let _ = app.emit("pindot:voice", VoiceStatus { state, notice });
}

/// The brain folder to start in. A saved folder that is gone is announced
/// (overlay notice and log) and the session starts the plain, isolated way.
fn brain_folder_for_start(app: &AppHandle, settings: &VoiceSettings) -> Option<PathBuf> {
    let state = pindot_core::brain::folder::FolderState::of(settings.brain_folder.as_deref());
    if let Some(msg) = state.notice() {
        println!("voice: {msg}");
        show(app, Display::Idle, Some(msg));
    }
    state.ready_path()
}

/// The voice parts: models loaded, microphone and speech ready.
struct VoiceParts {
    recorder: Recorder,
    speaker: Speaker,
    wav_sink: Option<Arc<WavSink>>,
    stt_dir: PathBuf,
}

/// Load the models and build the recorder and the speaker. Slow (models);
/// opens no microphone and no speaker. Any failure here leaves Chat alone.
fn load_voice_parts(app: &AppHandle, settings: &VoiceSettings, opts: &VoiceOptions, handle: &VoiceHandle) -> Result<VoiceParts> {
    // Models: present and verified. Never downloaded here: the user starts
    // the download on the Setup screen after seeing each license.
    let root = models::models_dir()?;
    let t0 = Instant::now();
    let stt_dir = models::resolve(&settings.stt_model, ModelKind::SpeechToText, &root)?;
    let tts_dir = models::resolve(&settings.tts_model, ModelKind::TextToSpeech, &root)?;
    println!("voice: models checked in {:.2} s ({} and {})", t0.elapsed().as_secs_f64(), settings.stt_model, settings.tts_model);
    let stt = load_stt(&stt_dir)?;
    // The chosen voice, if it was downloaded in Settings; else the default.
    let tts_voice = phonon_voices::present(settings.tts_voice.as_deref(), &root).unwrap_or_else(|e| {
        println!("voice: {e:#}; speaking with the default voice");
        None
    });
    let tts: Arc<dyn TextToSpeech> =
        Arc::new(PhononTts::load(&tts_dir, tts_voice.as_ref().map(|(n, p)| (*n, p.as_path())))?);

    let mic: Box<dyn Microphone> = match &opts.test_wav {
        Some(p) => Box::new(WavMicrophone { path: p.clone() }),
        None => {
            // Listing devices opens no stream. A saved microphone that is not
            // plugged in is announced, then the system default is used.
            let present = audio::list_devices().map(|d| d.inputs).unwrap_or_default();
            let choice = audio::InputChoice::resolve(settings.input_device.as_deref(), &present);
            if let Some(msg) = choice.notice() {
                println!("voice: {msg}");
                show(app, Display::Idle, Some(msg));
            }
            let app_n = app.clone();
            let on_notice: audio::MicNotice = Arc::new(move |msg| show(&app_n, Display::Idle, Some(msg)));
            // One microphone user at a time: dictation into the chat box
            // takes the same lease, so a spoken turn and a dictation can
            // never both record.
            Box::new(audio::ArbitedMic::new(
                CpalMicrophone::with_device(settings.input_device.clone(), Some(on_notice)),
                audio::mic_arbiter(),
                audio::MicOwner::Voice,
            ))
        }
    };
    let recorder = Recorder::start(mic, stt, handle.clone())?;

    let wav_sink = match &opts.test_wav {
        Some(_) => {
            let dir = std::env::temp_dir().join(format!("pindot-test-voice-{}", std::process::id()));
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Some(Arc::new(WavSink::new(dir, Some(Arc::new(CpalSpeaker { volume: TEST_VOLUME })))))
        }
        None => None,
    };
    let sink: Arc<dyn AudioSink> = match &wav_sink {
        Some(w) => w.clone(),
        None => Arc::new(CpalSpeaker { volume: settings.volume }),
    };
    let (h, capt_app, captions) = (handle.clone(), app.clone(), settings.captions);
    let speaker = Speaker::new(
        tts,
        sink,
        Arc::new(move |e: SpeakerEvent| {
            // Captions: what is being said, from its first sound to its end.
            if captions {
                match &e {
                    SpeakerEvent::FirstAudio { seq, text, .. } => {
                        let _ = capt_app.emit("pindot:caption", serde_json::json!({ "seq": seq, "text": text }));
                    }
                    SpeakerEvent::Generated { seq, secs } => {
                        let _ = capt_app.emit("pindot:caption-timing", serde_json::json!({ "seq": seq, "secs": secs }));
                    }
                    SpeakerEvent::Finished { seq, interrupted } => {
                        let _ = capt_app
                            .emit("pindot:caption-end", serde_json::json!({ "seq": seq, "interrupted": interrupted }));
                    }
                    _ => {}
                }
            }
            h.speaker_event(e)
        }),
    )?;
    Ok(VoiceParts { recorder, speaker, wav_sink, stt_dir })
}

fn setup(app: &AppHandle, voice: &Arc<VoiceApp>, state: Arc<ServerState>, link: ServerLink, opts: VoiceOptions) -> Result<()> {
    let settings = VoiceSettings::load();
    if let Some(w) = &opts.test_wav {
        let banner = format!(
            "TEST VOICE MODE: the microphone is replaced by {} and the hotkey is pressed synthetically. Never for normal use.",
            w.display()
        );
        println!("*** {banner} ***");
        tracing::warn!("{banner}");
    }

    let (handle, inbox) = runner::channel();
    let probe = opts.test_wav.as_ref().map(|_| Arc::new(Probe::default()));

    // The assistant (the brain) first and on its own: Chat must not wait
    // for models, a microphone or a permission. Its events queue in the
    // runner's inbox until the runner starts.
    let config = BrainConfig {
        claude_bin: None,
        cwd: session_dir()?,
        mcp_url: link.mcp_url,
        stop_hook_url: link.stop_hook_url,
        token: link.token,
        model: opts.model.clone().or(settings.model.clone()),
        effort: settings.effort.clone(),
        permission_mode: settings.permission_mode,
        system_prompt: voice_system_prompt(&settings),
        brain_folder: brain_folder_for_start(app, &settings),
    };
    let (h, p, chat_app) = (handle.clone(), probe.clone(), app.clone());
    let brain = PersistentBrain::start(
        config,
        state.clone(),
        Arc::new(move |i: Input| {
            if let Some(p) = &p {
                p.input(&i);
            }
            // The chat window follows whichever session the brain is on.
            if let Input::SessionReady { session_id } = &i
                && let Some(hub) = chat_app.try_state::<Arc<crate::chat::ChatHub>>()
            {
                hub.session_ready(session_id);
            }
            h.input(i)
        }),
    )?;

    // Then, only if wanted, the voice parts. A failure here is kept for the
    // settings screen and leaves Chat running (the test path stays strict).
    let parts = if opts.with_voice {
        match load_voice_parts(app, &settings, &opts, &handle) {
            Ok(p) => Some(p),
            Err(e) if opts.test_wav.is_some() => return Err(e),
            Err(e) => {
                println!("voice: hold-to-talk could not start: {e:#}; chat still runs");
                *voice.failed.lock().unwrap() = Some(format!("{e:#}"));
                show(app, Display::Idle, Some(format!("Hold-to-talk is off: {e:#}")));
                None
            }
        }
    } else {
        None
    };
    let with_voice = parts.is_some();
    let (recorder, speaker, wav_sink, stt_dir) = match parts {
        Some(p) => (Some(p.recorder), Some(p.speaker), p.wav_sink, Some(p.stt_dir)),
        None => (None, None, None, None),
    };

    let shown = Arc::new(Mutex::new(Display::Idle));
    let status: StatusSink = {
        let (app, p, shown) = (app.clone(), probe.clone(), shown.clone());
        Arc::new(move |s: &VoiceStatus| {
            if let Some(p) = &p {
                p.status(s);
            }
            *shown.lock().unwrap() = s.state;
            if let Some(hub) = app.try_state::<Arc<crate::chat::ChatHub>>() {
                hub.set_display(s.state);
            }
            let _ = app.emit("pindot:voice", s);
        })
    };
    let io = LiveIo { recorder, speaker, handle: Some(handle.clone()), brain, control: Some(state.control.clone()), status, shown: Display::Idle };
    let machine = Machine::new(settings.speak_mode, settings.idle_reset(), Instant::now()).with_tap_to_talk(settings.tap_to_talk);
    let runner = VoiceRunner::start(handle.clone(), inbox, machine, Box::new(io))?;

    forward_esc(&state, handle.clone(), Arc::downgrade(voice));
    let hotkey_label = opts.hotkey.label_for(pindot_core::platform::Os::current());
    let hotkey = if with_voice {
        match start_hotkey(opts.hotkey.clone(), handle.clone(), app.clone()) {
            Ok(l) => Some(l),
            Err(e) => {
                println!("voice: the hold-to-talk hotkey is off: {e}");
                show(app, Display::Idle, Some(format!("Hold-to-talk is off: {e}")));
                None
            }
        }
    } else {
        None
    };
    let tap_ok = hotkey.is_some();
    {
        let mut running = voice.running.lock().unwrap();
        if voice.closed.load(Ordering::SeqCst) {
            // The app quit while the models loaded.
            return Ok(());
        }
        let companion = if settings.companion {
            Companion::start(app.clone(), shown).map_err(|e| println!("voice: no companion cursor: {e}")).ok()
        } else {
            None
        };
        *running = Some(Running { voice: with_voice, hotkey, runner, handle: handle.clone(), _companion: companion });
    }
    if with_voice {
        println!(
            "voice: ready (speak mode {:?}, idle reset {} s). Hold {hotkey_label} to talk; Esc stops everything.",
            settings.speak_mode,
            settings.idle_reset().as_secs(),
        );
    } else {
        println!("voice: the assistant is ready for Chat (hold-to-talk is off)");
    }

    if let (Some(probe), Some(sink), Some(stt_dir)) = (probe, wav_sink, stt_dir) {
        let test = TestRun {
            app: app.clone(),
            voice: voice.clone(),
            state,
            probe,
            handle,
            tap_ok,
            sink,
            stt_dir,
            timeout: opts.test_timeout,
            esc: opts.test_esc,
        };
        let exit_when_done = opts.exit_when_done;
        std::thread::Builder::new().name("pindot-test-voice".into()).spawn(move || {
            let ok = test.run();
            println!("test-voice: RESULT {}", if ok { "PASS" } else { "CHECK OUTPUT" });
            if exit_when_done {
                std::thread::sleep(Duration::from_millis(300));
                test.app.exit(if ok { 0 } else { 1 });
            } else {
                println!("test-voice: still running (--keep-running); quit with Ctrl+C");
            }
        })?;
    }
    Ok(())
}

/// The Phase 1 emergency stop (Esc) also drops the recording, stops speech
/// and interrupts the turn.
fn forward_esc(state: &Arc<ServerState>, handle: VoiceHandle, voice: std::sync::Weak<VoiceApp>) {
    let mut rx = state.control.subscribe();
    let spawned = std::thread::Builder::new().name("pindot-voice-esc".into()).spawn(move || {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            // A restarted voice loop has its own forwarder; this one ends.
            if voice.upgrade().is_none_or(|v| v.closed.load(Ordering::SeqCst)) {
                break;
            }
            match rx.blocking_recv() {
                Ok(ControlEvent::EmergencyStop) => handle.input(Input::Esc),
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("Esc will not stop the voice loop: {e}");
    }
}

fn start_hotkey(binding: HotkeyBinding, handle: VoiceHandle, app: AppHandle) -> Result<ListenerHandle, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = pindot_core::platform::native().hotkey.start(binding, tx).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("pindot-hotkey".into())
        .spawn(move || {
            for e in rx {
                // Logged so a quiet hotkey can be told from a quiet voice loop.
                tracing::info!("voice: hold-to-talk key {e:?}");
                // With the chat box focused the key dictates into it
                // instead of sending a spoken turn (see `dictation`).
                if let Some(hub) = app.try_state::<std::sync::Arc<crate::dictation::DictationHub>>()
                    && hub.hotkey(e)
                {
                    continue;
                }
                handle.input(match e {
                    HotkeyEvent::Down => Input::HotkeyDown,
                    HotkeyEvent::Up => Input::HotkeyUp,
                    // Another key joined the chord: it was a shortcut, not
                    // push-to-talk, so the recording is dropped.
                    HotkeyEvent::Cancel => Input::HotkeyCancel,
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(listener)
}

// ------------------------------------------------------------- test path

/// What the test path saw, with times: the brain's inputs and the overlay.
#[derive(Default)]
struct Probe {
    s: Mutex<ProbeState>,
}

#[derive(Default, Clone)]
struct ProbeState {
    session: Option<String>,
    unavailable: Option<String>,
    stops: Vec<(Instant, Option<String>)>,
    failed: Vec<(Instant, String)>,
    tools: Vec<(Instant, String)>,
    texts: Vec<(Instant, String)>,
    shown: Vec<(Instant, Display)>,
    notices: Vec<(Instant, String)>,
}

fn since<T: Clone>(v: &[(Instant, T)], t: Instant) -> Vec<T> {
    v.iter().filter(|(at, _)| *at >= t).map(|(_, x)| x.clone()).collect()
}

impl Probe {
    fn input(&self, i: &Input) {
        let mut s = self.s.lock().unwrap();
        let now = Instant::now();
        match i {
            Input::SessionReady { session_id } => s.session = Some(session_id.clone()),
            Input::BrainUnavailable { message } => s.unavailable = Some(message.clone()),
            Input::Stop { last_message, .. } => s.stops.push((now, last_message.clone())),
            Input::TurnFailed { message } => s.failed.push((now, message.clone())),
            Input::TranscriptToolUse { name, .. } => s.tools.push((now, name.clone())),
            Input::TranscriptText { text, .. } => s.texts.push((now, text.clone())),
            _ => {}
        }
    }

    fn status(&self, st: &VoiceStatus) {
        let mut s = self.s.lock().unwrap();
        match &st.notice {
            Some(n) => s.notices.push((Instant::now(), n.clone())),
            None => s.shown.push((Instant::now(), st.state)),
        }
    }

    fn get(&self) -> ProbeState {
        self.s.lock().unwrap().clone()
    }
}

struct TestRun {
    app: AppHandle,
    voice: Arc<VoiceApp>,
    state: Arc<ServerState>,
    probe: Arc<Probe>,
    handle: VoiceHandle,
    tap_ok: bool,
    sink: Arc<WavSink>,
    stt_dir: PathBuf,
    timeout: Duration,
    esc: bool,
}

impl TestRun {
    /// Press the hotkey through the real tap (a marked synthetic Fn), or
    /// straight into the runner when the tap is not available.
    fn press(&self, down: bool, through_tap: bool) {
        #[cfg(target_os = "macos")]
        {
            if through_tap {
                println!("test-voice: posting a synthetic Fn {}", if down { "down" } else { "up" });
                match pindot_core::platform::macos::hotkey::post_synthetic_fn(down) {
                    Ok(()) => return,
                    Err(e) => println!("test-voice: posting Fn failed ({e}); injecting the hotkey directly"),
                }
            }
        }
        let _ = through_tap;
        println!("test-voice: injecting hotkey {} directly (no tap)", if down { "down" } else { "up" });
        self.handle.input(if down { Input::HotkeyDown } else { Input::HotkeyUp });
    }

    fn wait(&self, limit: Duration, done: impl Fn(&ProbeState) -> bool) -> Option<ProbeState> {
        let t = Instant::now();
        loop {
            let s = self.probe.get();
            if done(&s) {
                return Some(s);
            }
            if t.elapsed() > limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn run(&self) -> bool {
        let t0 = Instant::now();
        let Some(s) = self.wait(TEST_READY_TIMEOUT, |s| s.session.is_some() || s.unavailable.is_some()) else {
            println!("test-voice: claude was not ready within {} s", TEST_READY_TIMEOUT.as_secs());
            return false;
        };
        if let Some(m) = s.unavailable {
            println!("test-voice: the brain is unavailable: {m}");
            return false;
        }
        println!("test-voice: claude session ready {:.1} s after the models loaded", t0.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_millis(500));
        if !self.esc {
            return self.interaction("press 1", false);
        }
        let first = self.interaction("press 1 (Esc during the turn)", true);
        std::thread::sleep(Duration::from_secs(2));
        let second = self.interaction("press 2 (same session, after the interrupt)", false);
        first && second
    }

    /// One hold-speak-release. With `esc`, press Esc once the turn has run a
    /// tool and a test-approved app is in front, and expect no Stop hook.
    fn interaction(&self, label: &str, esc: bool) -> bool {
        println!("test-voice: === {label} ===");
        let files_before = self.sink.written.lock().unwrap().len();
        let pressed_at = Instant::now();
        let listening = |s: &ProbeState| since(&s.shown, pressed_at).contains(&Display::Listening);
        self.press(true, self.tap_ok);
        let mut through_tap = self.tap_ok;
        if self.tap_ok && self.wait(Duration::from_secs(1), listening).is_none() {
            println!("test-voice: the Fn tap did not report the synthetic press; injecting the hotkey directly");
            through_tap = false;
            self.press(true, false);
        }
        if through_tap {
            println!("test-voice: the Fn tap reported the synthetic press (overlay shows listening)");
        }
        std::thread::sleep(TEST_HOLD);
        let released_at = Instant::now();
        self.press(false, through_tap);

        let mut esc_ok = true;
        if esc {
            esc_ok = self.press_esc_mid_turn(pressed_at);
        }
        // Done when the overlay is back to idle after the press: during a
        // live turn it never is.
        let finished = |s: &ProbeState| {
            let after = since(&s.shown, pressed_at);
            after.len() > 1 && after.last() == Some(&Display::Idle)
        };
        let s = match self.wait(self.timeout, finished) {
            Some(_) => {
                // Let the last WAV be written.
                std::thread::sleep(Duration::from_millis(300));
                self.probe.get()
            }
            None => {
                println!("test-voice: no finished interaction within {} s", self.timeout.as_secs());
                let s = self.probe.get();
                self.report(&s, pressed_at, released_at, files_before);
                return false;
            }
        };
        let spoken_ok = self.report(&s, pressed_at, released_at, files_before);
        let stops = since(&s.stops, pressed_at);
        let failed = since(&s.failed, pressed_at);
        if esc {
            let ok = esc_ok && stops.is_empty() && failed.is_empty();
            println!(
                "test-voice: interrupt check: Esc consumed {esc_ok}, Stop hooks for this turn {} (expect 0), halted now {}",
                stops.len(),
                self.state.control.is_halted()
            );
            ok
        } else {
            !stops.is_empty() && failed.is_empty() && spoken_ok
        }
    }

    /// Post one unmarked Esc while the voice turn runs, but only with a
    /// test-approved app (one the test opened) in front, so a missed Esc
    /// could only reach that app. Checks that no app received it.
    fn press_esc_mid_turn(&self, pressed_at: Instant) -> bool {
        let ax = pindot_core::platform::native().accessibility;
        let front_is_test_app = || {
            let Some(pid) = ax.frontmost_pid().ok().flatten() else { return false };
            let Some(app) = ax.app_info(pid) else { return false };
            self.state.auto_approve.iter().any(|n| n.eq_ignore_ascii_case(&app.name))
        };
        let t = Instant::now();
        loop {
            let s = self.probe.get();
            let live = since(&s.shown, pressed_at).last().is_some_and(|d| *d != Display::Idle);
            if !live {
                println!("test-voice: the turn ended before Esc could be pressed");
                return false;
            }
            if !since(&s.tools, pressed_at).is_empty() && front_is_test_app() {
                break;
            }
            if t.elapsed() > self.timeout {
                println!("test-voice: never had a tool call with a test app in front; no Esc posted");
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        #[cfg(target_os = "macos")]
        {
            use pindot_core::platform::macos::stopkey::{post_test_escape, watch_escape};
            let (tx, rx) = std::sync::mpsc::channel();
            let watcher = watch_escape(tx);
            if let Err(e) = &watcher {
                println!("test-voice: Esc watcher failed ({e}); posting anyway");
            }
            std::thread::sleep(Duration::from_millis(100));
            println!(
                "test-voice: posting one synthetic (unmarked) Esc during the turn; overlay {:?}, action in flight {}",
                self.probe.get().shown.last().map(|(_, d)| *d),
                self.state.control.in_flight()
            );
            if let Err(e) = post_test_escape() {
                println!("test-voice: posting Esc failed: {e}");
                return false;
            }
            std::thread::sleep(Duration::from_millis(400));
            drop(watcher);
            let leaked = rx.try_iter().count();
            println!(
                "test-voice: Esc events that reached apps: {leaked} ({})",
                if leaked == 0 { "consumed" } else { "NOT consumed" }
            );
            leaked == 0
        }
        #[cfg(not(target_os = "macos"))]
        {
            println!("test-voice: no synthetic Esc on this platform; sending the stop directly");
            self.handle.input(Input::Esc);
            true
        }
    }

    /// Print what happened since the press; returns whether every new
    /// spoken reply is real audio.
    fn report(&self, s: &ProbeState, pressed_at: Instant, released_at: Instant, files_before: usize) -> bool {
        let mut states: Vec<String> = vec![];
        for (t, d) in s.shown.iter().filter(|(t, _)| *t >= pressed_at) {
            states.push(format!("{d:?} @{:.2}s", t.duration_since(pressed_at).as_secs_f64()));
        }
        println!("test-voice: overlay states after the press: {}", states.join(" -> "));
        let notices = since(&s.notices, pressed_at);
        if !notices.is_empty() {
            println!("test-voice: notices: {notices:?}");
        }
        println!("test-voice: tools called (transcript): {:?}", since(&s.tools, pressed_at));
        println!("test-voice: assistant text messages (transcript): {:?}", since(&s.texts, pressed_at));
        for f in since(&s.failed, pressed_at) {
            println!("test-voice: turn failed: {f}");
        }
        let stop = s.stops.iter().find(|(t, _)| *t >= pressed_at);
        match stop {
            Some((t, msg)) => {
                println!(
                    "test-voice: Stop hook {:.2} s after the release; last message {:?}",
                    t.duration_since(released_at).as_secs_f64(),
                    msg
                );
                if let Some((first, _)) = s.shown.iter().find(|(t, d)| *t >= pressed_at && *d == Display::Speaking) {
                    if first <= t {
                        println!(
                            "test-voice: speech started {} ms BEFORE the Stop hook (spoken as the message landed)",
                            (*t - *first).as_millis()
                        );
                    } else {
                        println!("test-voice: speech started {} ms after the Stop hook", (*first - *t).as_millis());
                    }
                }
            }
            None => println!("test-voice: no Stop hook for this turn"),
        }
        for m in self.voice.metrics() {
            println!("test-voice: latency so far, {}: {} ms", m.stage, m.ms);
        }
        let files: Vec<PathBuf> = self.sink.written.lock().unwrap().iter().skip(files_before).cloned().collect();
        let mut spoken_ok = !files.is_empty();
        let mut stt = match load_stt(&self.stt_dir) {
            Ok(s) => Some(s),
            Err(e) => {
                println!("test-voice: no transcribe-back ({e:#})");
                None
            }
        };
        for f in &files {
            match audio::read_wav_mono(f) {
                Ok((samples, rate)) => {
                    let secs = samples.len() as f64 / rate as f64;
                    let rms = audio::rms(&samples);
                    let back = stt
                        .as_mut()
                        .and_then(|m| m.transcribe(&audio::resample(&samples, rate, STT_SAMPLE_RATE)).ok())
                        .unwrap_or_default();
                    println!(
                        "test-voice: spoken audio {}: {secs:.2} s at {rate} Hz, rms {rms:.4}, transcribed back as {back:?}",
                        f.display()
                    );
                    spoken_ok &= secs > 0.3 && rms > 0.005;
                }
                Err(e) => {
                    println!("test-voice: could not read {}: {e:#}", f.display());
                    spoken_ok = false;
                }
            }
        }
        if files.is_empty() {
            println!("test-voice: nothing was spoken in this interaction");
        }
        spoken_ok
    }
}
