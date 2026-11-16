//! Dictation: speak, and the words come back as text for the chat box.
//!
//! The same local speech to text as the voice loop and the microphone chosen
//! in Settings, but nothing is sent anywhere. The text is handed back and the
//! person edits it and sends it themselves. Nothing is spoken back and no
//! audio is kept: the samples are dropped as soon as they are transcribed.
//!
//! One worker thread owns the microphone (a cpal stream is not `Send`, so
//! the stream stays on the microphone's own thread) and the speech model.
//! The microphone is wrapped in an `ArbitedMic`, so a dictation and a spoken
//! turn can never record at the same time.
//!
//! Everything the world gives us (the device, the model, the permission
//! check) comes through the `Backend` trait, so the logic is tested with
//! fakes and no test ever opens a microphone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use super::audio::{self, ArbitedMic, MicArbiter, MicOwner, Microphone};
use super::engines::{STT_SAMPLE_RATE, SpeechToText};
use super::runner::MIN_SPEECH;

/// A dictation that is never stopped ends itself after this long, so the
/// microphone is not left open by accident.
pub const MAX_LISTEN: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Listening,
    Transcribing,
}

/// What went wrong, so the screen can say what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// Another dictation, or a spoken turn, has the microphone.
    Busy,
    /// No speech model downloaded, or it would not load.
    NoModel,
    /// No microphone, or no permission to use it.
    NoMic,
    /// Nothing was recognised.
    Nothing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    State { id: u64, state: State },
    /// The recognised words (never empty). Not sent anywhere by this module.
    Text { id: u64, text: String },
    Failed { id: u64, problem: Problem, message: String },
    /// Cancelled on request: no text follows.
    Cancelled { id: u64 },
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The outside world, as the dictation worker sees it.
pub trait Backend: Send + Sync {
    /// Read-only check before anything opens: a speech model is there, a
    /// microphone exists, and the permission is granted. Opens no device and
    /// never shows a system prompt. `Ok` carries the model's identity so a
    /// changed setting reloads the model.
    fn check(&self) -> std::result::Result<String, (Problem, String)>;
    fn load_stt(&self) -> Result<Box<dyn SpeechToText>>;
    /// A microphone object. Nothing is opened until `start`.
    fn open_mic(&self) -> Box<dyn Microphone>;
}

impl Microphone for Box<dyn Microphone> {
    fn start(&mut self) -> Result<()> {
        (**self).start()
    }
    fn stop(&mut self) -> Result<Vec<f32>> {
        (**self).stop()
    }
    fn cancel(&mut self) {
        (**self).cancel()
    }
}

enum Cmd {
    Begin(u64),
    Finish(u64),
    Cancel,
}

struct Shared {
    state: Mutex<State>,
    last_id: AtomicU64,
    /// Every id up to this one was cancelled.
    cancelled: AtomicU64,
}

pub struct Dictation {
    tx: Option<mpsc::Sender<Cmd>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Dictation {
    pub fn start(backend: Arc<dyn Backend>, arbiter: Arc<MicArbiter>, sink: Sink) -> std::io::Result<Self> {
        Self::start_with(backend, arbiter, sink, MAX_LISTEN)
    }

    fn start_with(backend: Arc<dyn Backend>, arbiter: Arc<MicArbiter>, sink: Sink, max_listen: Duration) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared { state: Mutex::new(State::Idle), last_id: AtomicU64::new(0), cancelled: AtomicU64::new(0) });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("pindot-dictation".into())
            .spawn(move || Worker { backend, arbiter, sink, shared: s, stt: None, active: None, max_listen }.run(rx))?;
        Ok(Self { tx: Some(tx), shared, thread: Some(thread) })
    }

    pub fn state(&self) -> State {
        *self.shared.state.lock().unwrap()
    }

    /// Start listening. Returns the dictation's id, or says why not (already
    /// dictating, or still turning the last words into text). The microphone
    /// itself may still turn out to be busy: that arrives as an event.
    pub fn begin(&self) -> std::result::Result<u64, (Problem, String)> {
        let mut st = self.shared.state.lock().unwrap();
        match *st {
            State::Idle => {}
            State::Listening => return Err((Problem::Busy, "Dictation is already listening.".into())),
            State::Transcribing => return Err((Problem::Busy, "Still turning your last words into text. One moment.".into())),
        }
        let id = self.shared.last_id.fetch_add(1, Ordering::SeqCst) + 1;
        *st = State::Listening;
        drop(st);
        self.send(Cmd::Begin(id));
        Ok(id)
    }

    /// Stop listening and turn what was said into text.
    pub fn finish(&self, id: u64) {
        self.send(Cmd::Finish(id));
    }

    /// Stop and throw the audio away. No text follows, even from a
    /// transcription already under way.
    pub fn cancel(&self) {
        self.shared.cancelled.fetch_max(self.shared.last_id.load(Ordering::SeqCst), Ordering::SeqCst);
        self.send(Cmd::Cancel);
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(c);
        }
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.cancel();
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Active {
    id: u64,
    mic: ArbitedMic<Box<dyn Microphone>>,
    since: Instant,
}

struct Worker {
    backend: Arc<dyn Backend>,
    arbiter: Arc<MicArbiter>,
    sink: Sink,
    shared: Arc<Shared>,
    /// The loaded model and the identity it was loaded for.
    stt: Option<(String, Box<dyn SpeechToText>)>,
    active: Option<Active>,
    max_listen: Duration,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            let cmd = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => match &self.active {
                    Some(a) if a.since.elapsed() >= self.max_listen => Cmd::Finish(a.id),
                    _ => continue,
                },
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match cmd {
                Cmd::Begin(id) => self.begin(id),
                Cmd::Finish(id) => self.finish(id),
                Cmd::Cancel => self.cancel(),
            }
        }
        if let Some(mut a) = self.active.take() {
            a.mic.cancel();
        }
    }

    fn cancelled(&self, id: u64) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst) >= id
    }

    fn set(&self, id: u64, state: State) {
        *self.shared.state.lock().unwrap() = state;
        (self.sink)(Event::State { id, state });
    }

    fn fail(&self, id: u64, problem: Problem, message: String) {
        tracing::warn!("dictation: {message}");
        (self.sink)(Event::Failed { id, problem, message });
        self.set(id, State::Idle);
    }

    fn begin(&mut self, id: u64) {
        if self.cancelled(id) {
            (self.sink)(Event::Cancelled { id });
            return self.set(id, State::Idle);
        }
        let key = match self.backend.check() {
            Ok(k) => k,
            Err((p, m)) => return self.fail(id, p, m),
        };
        let mut mic = ArbitedMic::new(self.backend.open_mic(), self.arbiter.clone(), MicOwner::Dictation);
        if let Err(e) = mic.start() {
            let message = format!("{e:#}");
            let problem = if message == audio::busy_message(MicOwner::Voice) || message == audio::busy_message(MicOwner::Dictation) {
                Problem::Busy
            } else {
                Problem::NoMic
            };
            let message = if problem == Problem::Busy {
                message
            } else {
                format!("The microphone would not start ({message}). Check the microphone in Settings.")
            };
            return self.fail(id, problem, message);
        }
        self.set(id, State::Listening);
        // The model loads while the person speaks, so the wait is hidden.
        if self.stt.as_ref().map(|(k, _)| k.as_str()) != Some(key.as_str()) {
            match self.backend.load_stt() {
                Ok(m) => self.stt = Some((key, m)),
                Err(e) => {
                    mic.cancel();
                    return self.fail(
                        id,
                        Problem::NoModel,
                        format!("The speech model could not be loaded: {e:#}. Check it under Setup."),
                    );
                }
            }
        }
        if self.cancelled(id) {
            mic.cancel();
            (self.sink)(Event::Cancelled { id });
            return self.set(id, State::Idle);
        }
        self.active = Some(Active { id, mic, since: Instant::now() });
    }

    fn finish(&mut self, id: u64) {
        let Some(mut a) = self.active.take_if(|a| a.id == id) else { return };
        self.set(id, State::Transcribing);
        let samples = match a.mic.stop() {
            Ok(s) => s,
            Err(e) => return self.fail(id, Problem::NoMic, format!("The microphone stopped working: {e:#}")),
        };
        if self.cancelled(id) {
            (self.sink)(Event::Cancelled { id });
            return self.set(id, State::Idle);
        }
        let secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
        if secs < MIN_SPEECH.as_secs_f64() {
            return self.fail(id, Problem::Nothing, "That was too short to hear. Hold it a little longer.".into());
        }
        let Some((_, stt)) = self.stt.as_mut() else {
            return self.fail(id, Problem::NoModel, "The speech model is not loaded. Check it under Setup.".into());
        };
        let started = Instant::now();
        let result = stt.transcribe(&samples);
        drop(samples);
        match result {
            Ok(text) => {
                tracing::info!("dictation: {secs:.1} s of speech recognised in {} ms", started.elapsed().as_millis());
                let text = text.trim().to_string();
                if self.cancelled(id) {
                    (self.sink)(Event::Cancelled { id });
                } else if text.is_empty() {
                    return self.fail(id, Problem::Nothing, "I did not catch any words. Try again a bit closer to the microphone.".into());
                } else {
                    (self.sink)(Event::Text { id, text });
                }
                self.set(id, State::Idle);
            }
            Err(e) => self.fail(id, Problem::Failed, format!("Speech recognition failed: {e:#}")),
        }
    }

    fn cancel(&mut self) {
        if let Some(mut a) = self.active.take() {
            a.mic.cancel();
            (self.sink)(Event::Cancelled { id: a.id });
            self.set(a.id, State::Idle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    struct Fake {
        /// What `check` says.
        check: Arc<Mutex<std::result::Result<String, (Problem, String)>>>,
        samples: usize,
        text: Arc<Mutex<String>>,
        opened: Arc<AtomicUsize>,
        loads: Arc<AtomicUsize>,
        load_fails: bool,
        /// Held while "transcribing", so a test can cancel in the middle.
        gate: Arc<Mutex<()>>,
    }

    impl Fake {
        fn new(text: &str) -> Self {
            Fake {
                check: Arc::new(Mutex::new(Ok("model-a".into()))),
                samples: 16_000,
                text: Arc::new(Mutex::new(text.into())),
                opened: Arc::default(),
                loads: Arc::default(),
                load_fails: false,
                gate: Arc::default(),
            }
        }
    }

    struct FakeMic(Fake);
    impl Microphone for FakeMic {
        fn start(&mut self) -> Result<()> {
            self.0.opened.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&mut self) -> Result<Vec<f32>> {
            Ok(vec![0.2; self.0.samples])
        }
    }
    struct FakeStt(Fake);
    impl SpeechToText for FakeStt {
        fn transcribe(&mut self, _: &[f32]) -> Result<String> {
            let _wait = self.0.gate.lock().unwrap();
            Ok(self.0.text.lock().unwrap().clone())
        }
    }
    impl Backend for Fake {
        fn check(&self) -> std::result::Result<String, (Problem, String)> {
            self.check.lock().unwrap().clone()
        }
        fn load_stt(&self) -> Result<Box<dyn SpeechToText>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.load_fails {
                anyhow::bail!("broken file");
            }
            Ok(Box::new(FakeStt(self.clone())))
        }
        fn open_mic(&self) -> Box<dyn Microphone> {
            Box::new(FakeMic(self.clone()))
        }
    }

    struct Rig {
        d: Dictation,
        events: mpsc::Receiver<Event>,
        arbiter: Arc<MicArbiter>,
        fake: Fake,
    }

    fn rig(fake: Fake) -> Rig {
        rig_with(fake, MAX_LISTEN)
    }

    fn rig_with(fake: Fake, max: Duration) -> Rig {
        let (tx, events) = mpsc::channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        });
        let arbiter = MicArbiter::new();
        let d = Dictation::start_with(Arc::new(fake.clone()), arbiter.clone(), sink, max).unwrap();
        Rig { d, events, arbiter, fake }
    }

    impl Rig {
        fn next(&self) -> Event {
            self.events.recv_timeout(Duration::from_secs(5)).expect("an event")
        }
        /// Everything up to and including the next Idle.
        fn until_idle(&self) -> Vec<Event> {
            let mut out = vec![];
            loop {
                let e = self.next();
                let done = matches!(e, Event::State { state: State::Idle, .. });
                out.push(e);
                if done {
                    return out;
                }
            }
        }
    }

    #[test]
    fn speech_comes_back_as_text_and_nothing_else_happens() {
        let r = rig(Fake::new("  hello there  "));
        let id = r.d.begin().unwrap();
        assert_eq!(r.next(), Event::State { id, state: State::Listening });
        assert_eq!(r.arbiter.owner(), Some(MicOwner::Dictation));
        r.d.finish(id);
        let rest = r.until_idle();
        assert_eq!(
            rest,
            vec![
                Event::State { id, state: State::Transcribing },
                Event::Text { id, text: "hello there".into() },
                Event::State { id, state: State::Idle },
            ]
        );
        assert_eq!(r.arbiter.owner(), None, "the microphone is given back");
        assert_eq!(r.d.state(), State::Idle);
        assert_eq!(r.fake.opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_model_loads_once_and_again_when_the_setting_changes() {
        let r = rig(Fake::new("one"));
        for _ in 0..2 {
            let id = r.d.begin().unwrap();
            r.next();
            r.d.finish(id);
            r.until_idle();
        }
        assert_eq!(r.fake.loads.load(Ordering::SeqCst), 1);
        *r.fake.check.lock().unwrap() = Ok("model-b".into());
        let id = r.d.begin().unwrap();
        r.next();
        r.d.finish(id);
        r.until_idle();
        assert_eq!(r.fake.loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancel_while_listening_inserts_nothing() {
        let r = rig(Fake::new("never shown"));
        let id = r.d.begin().unwrap();
        r.next();
        r.d.cancel();
        assert_eq!(r.next(), Event::Cancelled { id });
        assert_eq!(r.next(), Event::State { id, state: State::Idle });
        assert_eq!(r.arbiter.owner(), None);
        assert_eq!(r.d.state(), State::Idle);
    }

    #[test]
    fn cancel_during_transcribing_drops_the_text() {
        let fake = Fake::new("late words");
        let hold = fake.gate.clone();
        let r = rig(fake);
        let id = r.d.begin().unwrap();
        r.next();
        let blocked = hold.lock().unwrap();
        r.d.finish(id);
        assert_eq!(r.next(), Event::State { id, state: State::Transcribing });
        assert_eq!(r.d.state(), State::Transcribing);
        r.d.cancel();
        drop(blocked);
        let rest = r.until_idle();
        assert!(rest.iter().all(|e| !matches!(e, Event::Text { .. })), "{rest:?}");
        assert!(rest.contains(&Event::Cancelled { id }));
    }

    #[test]
    fn a_second_press_while_busy_is_refused_with_a_reason() {
        let r = rig(Fake::new("x"));
        let id = r.d.begin().unwrap();
        r.next();
        let (p, m) = r.d.begin().unwrap_err();
        assert_eq!(p, Problem::Busy);
        assert!(m.contains("already listening"), "{m}");
        r.d.finish(id);
        r.until_idle();
        assert!(r.d.begin().is_ok());
    }

    #[test]
    fn a_spoken_turn_holding_the_microphone_blocks_dictation_before_it_opens() {
        let r = rig(Fake::new("x"));
        let voice = r.arbiter.try_acquire(MicOwner::Voice).unwrap();
        let id = r.d.begin().unwrap();
        match r.next() {
            Event::Failed { id: i, problem, message } => {
                assert_eq!((i, problem), (id, Problem::Busy));
                assert!(message.contains("listening"), "{message}");
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(r.fake.opened.load(Ordering::SeqCst), 0, "the device was never opened");
        assert_eq!(r.next(), Event::State { id, state: State::Idle });
        drop(voice);
        assert!(r.d.begin().is_ok());
    }

    #[test]
    fn a_missing_model_or_microphone_is_reported_and_nothing_opens() {
        let fake = Fake::new("x");
        *fake.check.lock().unwrap() = Err((Problem::NoModel, "No speech model yet. Download it in Setup.".into()));
        let r = rig(fake);
        let id = r.d.begin().unwrap();
        assert_eq!(
            r.next(),
            Event::Failed { id, problem: Problem::NoModel, message: "No speech model yet. Download it in Setup.".into() }
        );
        assert_eq!(r.fake.opened.load(Ordering::SeqCst), 0);
        *r.fake.check.lock().unwrap() = Err((Problem::NoMic, "Microphone access is off.".into()));
        r.until_idle();
        let id2 = r.d.begin().unwrap();
        assert!(matches!(r.next(), Event::Failed { id, problem: Problem::NoMic, .. } if id == id2));
        assert_eq!(r.fake.opened.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_model_that_will_not_load_gives_the_microphone_back() {
        let mut fake = Fake::new("x");
        fake.load_fails = true;
        let r = rig(fake);
        r.d.begin().unwrap();
        let events = r.until_idle();
        assert!(events.iter().any(|e| matches!(e, Event::Failed { problem: Problem::NoModel, .. })), "{events:?}");
        assert_eq!(r.arbiter.owner(), None);
    }

    #[test]
    fn a_blip_or_silence_says_so_instead_of_inserting() {
        let mut short = Fake::new("x");
        short.samples = 100;
        let r = rig(short);
        let id = r.d.begin().unwrap();
        r.next();
        r.d.finish(id);
        let events = r.until_idle();
        assert!(events.iter().any(|e| matches!(e, Event::Failed { problem: Problem::Nothing, .. })), "{events:?}");

        let r = rig(Fake::new("   "));
        let id = r.d.begin().unwrap();
        r.next();
        r.d.finish(id);
        let events = r.until_idle();
        assert!(events.iter().any(|e| matches!(e, Event::Failed { problem: Problem::Nothing, .. })), "{events:?}");
        assert!(events.iter().all(|e| !matches!(e, Event::Text { .. })));
    }

    #[test]
    fn a_dictation_nobody_stops_ends_itself() {
        let r = rig_with(Fake::new("forgotten"), Duration::from_millis(200));
        let id = r.d.begin().unwrap();
        r.next();
        let events = r.until_idle();
        assert!(events.contains(&Event::Text { id, text: "forgotten".into() }), "{events:?}");
        assert_eq!(r.arbiter.owner(), None);
    }

    #[test]
    fn dropping_it_while_listening_closes_the_microphone() {
        let r = rig(Fake::new("x"));
        r.d.begin().unwrap();
        r.next();
        let arbiter = r.arbiter.clone();
        drop(r);
        assert_eq!(arbiter.owner(), None);
    }
}
