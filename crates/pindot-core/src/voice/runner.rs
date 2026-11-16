//! The voice runner: one thread that owns the `Machine`, feeds it inputs
//! (hotkey, Esc, transcription results, brain and speaker events) and carries
//! out its effects through `VoiceIo`. It also logs the latency of each stage.
//!
//! `LiveIo` is the real `VoiceIo`: a recorder worker (microphone + speech to
//! text), the speaker, the persistent `claude`, the Esc tap and the overlay.

use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::audio::Microphone;
use super::engines::{STT_SAMPLE_RATE, SpeechToText};
use super::machine::{Display, Effect, Input, Machine};
use super::speaker::{Speaker, SpeakerEvent};
use crate::brain::persistent::PersistentBrain;
use crate::server::control::Control;

/// How often the machine gets a `Tick` (idle reset).
pub const TICK: Duration = Duration::from_secs(1);
/// Shorter recordings than this are treated as "didn't catch that".
pub const MIN_SPEECH: Duration = Duration::from_millis(250);

/// What the runner needs from the outside world. Every call returns fast;
/// slow work (recording, transcribing, typing into `claude`, speaking) runs
/// on the parts' own threads and reports back as inputs.
pub trait VoiceIo: Send {
    fn start_recording(&mut self, utterance: u64);
    fn finish_recording(&mut self, utterance: u64);
    fn discard_recording(&mut self);
    fn send_prompt(&mut self, text: &str);
    fn interrupt_turn(&mut self);
    fn new_session(&mut self);
    fn speak(&mut self, seq: u64, text: &str);
    fn stop_speech(&mut self);
    fn arm_esc(&mut self) -> Result<(), String>;
    fn disarm_esc(&mut self);
    fn show(&mut self, display: Display);
    fn notice(&mut self, text: &str);
}

enum Event {
    Input(Input),
    FirstAudio { seq: u64, after: Duration },
    Shutdown,
}

/// Cloneable sender into the runner.
#[derive(Clone)]
pub struct VoiceHandle {
    tx: mpsc::Sender<Event>,
}

impl VoiceHandle {
    pub fn input(&self, input: Input) {
        let _ = self.tx.send(Event::Input(input));
    }

    /// The speaker's events, as the runner wants them.
    pub fn speaker_event(&self, e: SpeakerEvent) {
        match e {
            SpeakerEvent::FirstAudio { seq, after, .. } => {
                let _ = self.tx.send(Event::FirstAudio { seq, after });
            }
            SpeakerEvent::Idle { seq } => self.input(Input::SpeechIdle { seq }),
            SpeakerEvent::Error { seq, error } => tracing::warn!("speech {seq} failed: {error}"),
            SpeakerEvent::Finished { .. } | SpeakerEvent::Generated { .. } => {}
        }
    }
}

pub struct Inbox {
    rx: mpsc::Receiver<Event>,
}

pub fn channel() -> (VoiceHandle, Inbox) {
    let (tx, rx) = mpsc::channel();
    (VoiceHandle { tx }, Inbox { rx })
}

/// Latency of one stage, in milliseconds.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Metric {
    pub stage: &'static str,
    pub ms: u128,
}

pub struct VoiceRunner {
    handle: VoiceHandle,
    metrics: Arc<Mutex<Vec<Metric>>>,
    thread: Option<JoinHandle<()>>,
}

impl VoiceRunner {
    pub fn start(handle: VoiceHandle, inbox: Inbox, machine: Machine, io: Box<dyn VoiceIo>) -> std::io::Result<Self> {
        let metrics = Arc::new(Mutex::new(Vec::new()));
        let m = metrics.clone();
        let thread = std::thread::Builder::new()
            .name("pindot-voice".into())
            .spawn(move || run(inbox.rx, machine, io, m))?;
        Ok(Self { handle, metrics, thread: Some(thread) })
    }

    pub fn handle(&self) -> VoiceHandle {
        self.handle.clone()
    }

    /// Every latency measured so far.
    pub fn metrics(&self) -> Vec<Metric> {
        self.metrics.lock().unwrap().clone()
    }
}

impl Drop for VoiceRunner {
    fn drop(&mut self) {
        let _ = self.handle.tx.send(Event::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Default)]
struct Clock {
    released: Option<Instant>,
    prompt_sent: Option<Instant>,
    /// First `Speak` seq of the current request, to time release -> audio.
    first_seq: Option<u64>,
}

fn run(rx: mpsc::Receiver<Event>, mut machine: Machine, mut io: Box<dyn VoiceIo>, metrics: Arc<Mutex<Vec<Metric>>>) {
    let mut clock = Clock::default();
    let record = |stage: &'static str, d: Duration| {
        let ms = d.as_millis();
        tracing::info!("voice latency: {stage} {ms} ms");
        metrics.lock().unwrap().push(Metric { stage, ms });
    };
    loop {
        let input = match rx.recv_timeout(TICK) {
            Ok(Event::Input(i)) => i,
            Ok(Event::FirstAudio { seq, after }) => {
                record("text to speech, first audio", after);
                if clock.first_seq == Some(seq)
                    && let Some(t) = clock.released.take()
                {
                    record("hotkey release to first spoken audio", t.elapsed());
                }
                continue;
            }
            Ok(Event::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => Input::Tick,
        };
        match &input {
            Input::HotkeyDown => clock = Clock::default(),
            Input::HotkeyUp => clock.released = Some(Instant::now()),
            Input::Transcribed { .. } => {
                if let Some(t) = clock.released {
                    record("hotkey release to text", t.elapsed());
                }
            }
            Input::Stop { .. } => {
                if let Some(t) = clock.prompt_sent.take() {
                    record("claude turn (prompt typed to Stop hook)", t.elapsed());
                }
            }
            _ => {}
        }
        if input != Input::Tick {
            tracing::debug!("voice input: {input:?}");
        }
        for effect in machine.handle(input, Instant::now()) {
            tracing::debug!("voice effect: {effect:?}");
            match effect {
                Effect::StartRecording { utterance } => io.start_recording(utterance),
                Effect::FinishRecording { utterance } => io.finish_recording(utterance),
                Effect::DiscardRecording => io.discard_recording(),
                Effect::SendPrompt { text } => {
                    tracing::info!("voice: sending the spoken request to claude: {text:?}");
                    clock.prompt_sent = Some(Instant::now());
                    io.send_prompt(&text)
                }
                Effect::InterruptTurn => io.interrupt_turn(),
                Effect::NewSession => {
                    tracing::info!("voice: idle for a while; starting a fresh claude session");
                    io.new_session()
                }
                Effect::Speak { seq, text } => {
                    clock.first_seq.get_or_insert(seq);
                    tracing::info!("voice: speaking #{seq}: {text:?}");
                    io.speak(seq, &text)
                }
                Effect::StopSpeech => io.stop_speech(),
                Effect::ArmEsc => {
                    if let Err(e) = io.arm_esc() {
                        tracing::warn!("Esc is not armed for this voice interaction: {e}");
                    }
                }
                Effect::DisarmEsc => io.disarm_esc(),
                Effect::Show(d) => io.show(d),
                Effect::Notice(n) => {
                    tracing::info!("voice notice: {n}");
                    io.notice(&n)
                }
            }
        }
    }
    io.stop_speech();
    io.disarm_esc();
    io.show(Display::Idle);
}

// ------------------------------------------------------------------ recorder

enum RecCmd {
    Start,
    Finish(u64),
    Discard,
}

/// Microphone and speech to text on their own thread. Loads the model there
/// too, so start-up doesn't block the caller.
pub struct Recorder {
    tx: Option<mpsc::Sender<RecCmd>>,
    thread: Option<JoinHandle<()>>,
}

impl Recorder {
    pub fn start(mut mic: Box<dyn Microphone>, mut stt: Box<dyn SpeechToText>, handle: VoiceHandle) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<RecCmd>();
        let thread = std::thread::Builder::new().name("pindot-recorder".into()).spawn(move || {
            let mut start_error: Option<String> = None;
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    RecCmd::Start => {
                        start_error = mic.start().err().map(|e| format!("{e:#}"));
                        if let Some(e) = &start_error {
                            tracing::warn!("microphone did not start: {e}");
                        }
                    }
                    RecCmd::Discard => mic.cancel(),
                    RecCmd::Finish(utterance) => {
                        if let Some(error) = start_error.take() {
                            mic.cancel();
                            handle.input(Input::TranscribeFailed { utterance, error });
                            continue;
                        }
                        let samples = match mic.stop() {
                            Ok(s) => s,
                            Err(e) => {
                                handle.input(Input::TranscribeFailed { utterance, error: format!("{e:#}") });
                                continue;
                            }
                        };
                        let secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
                        if secs < MIN_SPEECH.as_secs_f64() {
                            handle.input(Input::Transcribed { utterance, text: String::new() });
                            continue;
                        }
                        let t = Instant::now();
                        match stt.transcribe(&samples) {
                            Ok(text) => {
                                tracing::info!(
                                    "voice latency: speech to text {} ms for {secs:.2} s of audio",
                                    t.elapsed().as_millis()
                                );
                                tracing::info!("voice: heard {text:?}");
                                handle.input(Input::Transcribed { utterance, text });
                            }
                            Err(e) => handle.input(Input::TranscribeFailed { utterance, error: format!("{e:#}") }),
                        }
                    }
                }
            }
            mic.cancel();
        })?;
        Ok(Self { tx: Some(tx), thread: Some(thread) })
    }

    fn send(&self, c: RecCmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(c);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------ live io

/// What the overlay shows for the voice loop.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VoiceStatus {
    pub state: Display,
    pub notice: Option<String>,
}

pub type StatusSink = Arc<dyn Fn(&VoiceStatus) + Send + Sync>;

pub struct LiveIo {
    /// `None` while the voice parts are off (no models, no hold-to-talk):
    /// the assistant still answers typed messages, on screen only.
    pub recorder: Option<Recorder>,
    pub speaker: Option<Speaker>,
    /// Answers a `Speak` with "done" when there is no speaker, so the
    /// machine never waits for speech that cannot happen.
    pub handle: Option<VoiceHandle>,
    pub brain: PersistentBrain,
    /// `None`: no Esc tap (tests, or a platform without one).
    pub control: Option<Arc<Control>>,
    pub status: StatusSink,
    pub shown: Display,
}

impl VoiceIo for LiveIo {
    fn start_recording(&mut self, utterance: u64) {
        let _ = utterance;
        if let Some(r) = &self.recorder {
            r.send(RecCmd::Start);
        }
    }
    fn finish_recording(&mut self, utterance: u64) {
        if let Some(r) = &self.recorder {
            r.send(RecCmd::Finish(utterance));
        }
    }
    fn discard_recording(&mut self) {
        if let Some(r) = &self.recorder {
            r.send(RecCmd::Discard);
        }
    }
    fn send_prompt(&mut self, text: &str) {
        self.brain.send(text);
    }
    fn interrupt_turn(&mut self) {
        self.brain.interrupt();
    }
    fn new_session(&mut self) {
        self.brain.new_session();
    }
    fn speak(&mut self, seq: u64, text: &str) {
        match (&self.speaker, &self.handle) {
            (Some(sp), _) => sp.say(seq, text),
            (None, Some(h)) => h.input(Input::SpeechIdle { seq }),
            (None, None) => {}
        }
    }
    fn stop_speech(&mut self) {
        if let Some(sp) = &self.speaker {
            sp.stop();
        }
    }
    fn arm_esc(&mut self) -> Result<(), String> {
        match &self.control {
            Some(c) => c.arm_for_voice(),
            None => Ok(()),
        }
    }
    fn disarm_esc(&mut self) {
        if let Some(c) = &self.control {
            c.disarm_for_voice();
        }
    }
    fn show(&mut self, display: Display) {
        self.shown = display;
        (self.status)(&VoiceStatus { state: display, notice: None });
    }
    fn notice(&mut self, text: &str) {
        (self.status)(&VoiceStatus { state: self.shown, notice: Some(text.to_string()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::SpeakMode;

    /// Records every call; plays the part of the recorder, brain and speaker
    /// by answering through the handle like the real parts would.
    struct MockIo {
        handle: VoiceHandle,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl MockIo {
        fn log(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }
    }

    impl VoiceIo for MockIo {
        fn start_recording(&mut self, u: u64) {
            self.log(format!("record {u}"));
        }
        fn finish_recording(&mut self, utterance: u64) {
            self.log(format!("finish {utterance}"));
            self.handle.input(Input::Transcribed { utterance, text: "open text edit".into() });
        }
        fn discard_recording(&mut self) {
            self.log("discard".into());
        }
        fn send_prompt(&mut self, text: &str) {
            self.log(format!("prompt {text}"));
            let s = "s1".to_string();
            self.handle.input(Input::TranscriptPrompt { session_id: s.clone() });
            self.handle.input(Input::TranscriptToolUse { session_id: s.clone(), name: "open_app".into() });
            self.handle.input(Input::TranscriptToolResult { session_id: s.clone() });
            self.handle.input(Input::Stop { session_id: s, last_message: Some("TextEdit is open.".into()) });
        }
        fn interrupt_turn(&mut self) {
            self.log("interrupt".into());
        }
        fn new_session(&mut self) {
            self.log("new session".into());
        }
        fn speak(&mut self, seq: u64, text: &str) {
            self.log(format!("speak {text}"));
            self.handle.speaker_event(SpeakerEvent::FirstAudio { seq, after: Duration::from_millis(5), text: String::new() });
            self.handle.speaker_event(SpeakerEvent::Idle { seq });
        }
        fn stop_speech(&mut self) {
            self.log("stop speech".into());
        }
        fn arm_esc(&mut self) -> Result<(), String> {
            self.log("arm".into());
            Ok(())
        }
        fn disarm_esc(&mut self) {
            self.log("disarm".into());
        }
        fn show(&mut self, d: Display) {
            self.log(format!("show {d:?}"));
        }
        fn notice(&mut self, t: &str) {
            self.log(format!("notice {t}"));
        }
    }

    #[test]
    fn runner_drives_a_turn_end_to_end_and_times_it() {
        let (handle, inbox) = channel();
        let calls = Arc::new(Mutex::new(vec![]));
        let io = MockIo { handle: handle.clone(), calls: calls.clone() };
        let mut machine = Machine::new(SpeakMode::EachMessage, Duration::from_secs(600), Instant::now());
        machine.handle(Input::SessionReady { session_id: "s1".into() }, Instant::now());
        let runner = VoiceRunner::start(handle.clone(), inbox, machine, Box::new(io)).unwrap();
        handle.input(Input::HotkeyDown);
        handle.input(Input::HotkeyUp);
        let t = Instant::now();
        while !calls.lock().unwrap().iter().any(|c| c == "disarm") && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let metrics = runner.metrics();
        drop(runner);
        let calls = calls.lock().unwrap().clone();
        let pos = |s: &str| calls.iter().position(|c| c == s).unwrap_or_else(|| panic!("no {s:?} in {calls:?}"));
        assert!(pos("arm") < pos("record 1"));
        assert!(pos("show Listening") < pos("finish 1"));
        assert!(pos("prompt open text edit") < pos("show Acting"));
        assert!(pos("show Acting") < pos("speak TextEdit is open."));
        assert!(pos("speak TextEdit is open.") < pos("show Idle"));
        assert!(pos("show Idle") <= pos("disarm"));
        let stages: Vec<&str> = metrics.iter().map(|m| m.stage).collect();
        for s in [
            "hotkey release to text",
            "claude turn (prompt typed to Stop hook)",
            "text to speech, first audio",
            "hotkey release to first spoken audio",
        ] {
            assert!(stages.contains(&s), "missing {s}: {stages:?}");
        }
    }

    #[test]
    fn recorder_reports_short_audio_and_microphone_errors() {
        struct Mic(Option<&'static str>, usize);
        impl Microphone for Mic {
            fn start(&mut self) -> anyhow::Result<()> {
                match self.0 {
                    Some(e) => Err(anyhow::anyhow!(e)),
                    None => Ok(()),
                }
            }
            fn stop(&mut self) -> anyhow::Result<Vec<f32>> {
                Ok(vec![0.1; self.1])
            }
        }
        struct Stt;
        impl SpeechToText for Stt {
            fn transcribe(&mut self, s: &[f32]) -> anyhow::Result<String> {
                Ok(format!("{} samples", s.len()))
            }
        }
        let run = |mic: Mic| {
            let (handle, inbox) = channel();
            let r = Recorder::start(Box::new(mic), Box::new(Stt), handle).unwrap();
            r.send(RecCmd::Start);
            r.send(RecCmd::Finish(7));
            match inbox.rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::Input(i) => i,
                _ => panic!("expected an input"),
            }
        };
        assert_eq!(run(Mic(None, 16000)), Input::Transcribed { utterance: 7, text: "16000 samples".into() });
        assert_eq!(run(Mic(None, 100)), Input::Transcribed { utterance: 7, text: String::new() });
        assert_eq!(run(Mic(Some("no microphone"), 0)), Input::TranscribeFailed { utterance: 7, error: "no microphone".into() });
    }
}
