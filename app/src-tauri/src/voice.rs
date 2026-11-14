//! The voice loop inside the app: hold the hotkey (Fn on macOS), speak,
//! release. Deskhand transcribes the words locally, types them into ONE
//! persistent interactive `claude` (see `deskhand_core::brain::persistent`),
//! shows listening / thinking / acting / speaking on the overlay and speaks
//! the reply locally. The parts live in `deskhand_core::voice`.
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
use deskhand_core::brain::persistent::{BrainConfig, PersistentBrain, session_dir};
use deskhand_core::platform::{HotkeyBinding, HotkeyEvent, ListenerHandle};
use deskhand_core::server::ServerState;
use deskhand_core::server::control::ControlEvent;
use deskhand_core::voice::audio::{self, AudioSink, CpalMicrophone, CpalSpeaker, Microphone, WavMicrophone, WavSink};
use deskhand_core::voice::engines::{MoonshineStt, PhononTts, STT_SAMPLE_RATE, SpeechToText, TextToSpeech};
use deskhand_core::voice::machine::{Display, Input, Machine};
use deskhand_core::voice::models::{self, ModelKind};
use deskhand_core::voice::runner::{self, LiveIo, Metric, Recorder, StatusSink, VoiceHandle, VoiceRunner, VoiceStatus};
use deskhand_core::voice::speaker::Speaker;
use deskhand_core::voice::{VOICE_SYSTEM_PROMPT, VoiceSettings};
use tauri::{AppHandle, Emitter};

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
    hotkey: Option<ListenerHandle>,
    runner: VoiceRunner,
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
            Some(r) if r.hotkey.is_some() => VoiceState::Running,
            Some(_) => VoiceState::Failed("the hold-to-talk key could not be watched (see the log)".into()),
            None => VoiceState::Starting,
        }
    }

    fn metrics(&self) -> Vec<Metric> {
        self.running.lock().unwrap().as_ref().map(|r| r.runner.metrics()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceState {
    Starting,
    Running,
    Failed(String),
    Stopped,
}

/// Start the voice loop on a background thread (models may need a download
/// first). Returns at once.
pub fn start(app: &AppHandle, state: Arc<ServerState>, link: ServerLink, opts: VoiceOptions) -> Arc<VoiceApp> {
    let voice = Arc::new(VoiceApp::default());
    let (v, app) = (voice.clone(), app.clone());
    let spawned = std::thread::Builder::new().name("deskhand-voice-setup".into()).spawn(move || {
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
    let _ = app.emit("deskhand:voice", VoiceStatus { state, notice });
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

    // Models: present and verified. Never downloaded here: the user starts
    // the download on the Setup screen after seeing each license.
    let root = models::models_dir()?;
    let t0 = Instant::now();
    let stt_dir = models::resolve(&settings.stt_model, ModelKind::SpeechToText, &root)?;
    let tts_dir = models::resolve(&settings.tts_model, ModelKind::TextToSpeech, &root)?;
    println!("voice: models checked in {:.2} s ({} and {})", t0.elapsed().as_secs_f64(), settings.stt_model, settings.tts_model);
    let stt = MoonshineStt::load(&stt_dir)?;
    let tts: Arc<dyn TextToSpeech> = Arc::new(PhononTts::load(&tts_dir, settings.tts_voice.as_deref())?);

    let (handle, inbox) = runner::channel();
    let probe = opts.test_wav.as_ref().map(|_| Arc::new(Probe::default()));

    let mic: Box<dyn Microphone> = match &opts.test_wav {
        Some(p) => Box::new(WavMicrophone { path: p.clone() }),
        None => Box::new(CpalMicrophone::new()),
    };
    let recorder = Recorder::start(mic, Box::new(stt), handle.clone())?;

    let wav_sink = match &opts.test_wav {
        Some(_) => {
            let dir = std::env::temp_dir().join(format!("deskhand-test-voice-{}", std::process::id()));
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Some(Arc::new(WavSink::new(dir, Some(Arc::new(CpalSpeaker { volume: TEST_VOLUME })))))
        }
        None => None,
    };
    let sink: Arc<dyn AudioSink> = match &wav_sink {
        Some(w) => w.clone(),
        None => Arc::new(CpalSpeaker { volume: settings.volume }),
    };
    let h = handle.clone();
    let speaker = Speaker::new(tts, sink, Arc::new(move |e| h.speaker_event(e)))?;

    let config = BrainConfig {
        claude_bin: None,
        cwd: session_dir()?,
        mcp_url: link.mcp_url,
        stop_hook_url: link.stop_hook_url,
        token: link.token,
        model: opts.model.clone().or(settings.model.clone()),
        effort: settings.effort.clone(),
        permission_mode: settings.permission_mode,
        system_prompt: VOICE_SYSTEM_PROMPT.into(),
    };
    let (h, p) = (handle.clone(), probe.clone());
    let brain = PersistentBrain::start(
        config,
        state.clone(),
        Arc::new(move |i: Input| {
            if let Some(p) = &p {
                p.input(&i);
            }
            h.input(i)
        }),
    )?;

    let status: StatusSink = {
        let (app, p) = (app.clone(), probe.clone());
        Arc::new(move |s: &VoiceStatus| {
            if let Some(p) = &p {
                p.status(s);
            }
            let _ = app.emit("deskhand:voice", s);
        })
    };
    let io = LiveIo { recorder, speaker, brain, control: Some(state.control.clone()), status, shown: Display::Idle };
    let machine = Machine::new(settings.speak_mode, settings.idle_reset(), Instant::now());
    let runner = VoiceRunner::start(handle.clone(), inbox, machine, Box::new(io))?;

    forward_esc(&state, handle.clone(), Arc::downgrade(voice));
    let hotkey_label = opts.hotkey.label_for(deskhand_core::platform::Os::current());
    let hotkey = match start_hotkey(opts.hotkey.clone(), handle.clone()) {
        Ok(l) => Some(l),
        Err(e) => {
            println!("voice: the hold-to-talk hotkey is off: {e}");
            show(app, Display::Idle, Some(format!("Hold-to-talk is off: {e}")));
            None
        }
    };
    let tap_ok = hotkey.is_some();
    {
        let mut running = voice.running.lock().unwrap();
        if voice.closed.load(Ordering::SeqCst) {
            // The app quit while the models loaded.
            return Ok(());
        }
        *running = Some(Running { hotkey, runner });
    }
    println!(
        "voice: ready (speak mode {:?}, idle reset {} s). Hold {hotkey_label} to talk; Esc stops everything.",
        settings.speak_mode,
        settings.idle_reset().as_secs(),
    );

    if let (Some(probe), Some(sink)) = (probe, wav_sink) {
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
        std::thread::Builder::new().name("deskhand-test-voice".into()).spawn(move || {
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
    let spawned = std::thread::Builder::new().name("deskhand-voice-esc".into()).spawn(move || {
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

fn start_hotkey(binding: HotkeyBinding, handle: VoiceHandle) -> Result<ListenerHandle, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = deskhand_core::platform::native().hotkey.start(binding, tx).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("deskhand-hotkey".into())
        .spawn(move || {
            for e in rx {
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
                match deskhand_core::platform::macos::hotkey::post_synthetic_fn(down) {
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
        let ax = deskhand_core::platform::native().accessibility;
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
            use deskhand_core::platform::macos::stopkey::{post_test_escape, watch_escape};
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
        let mut stt = match MoonshineStt::load(&self.stt_dir) {
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
