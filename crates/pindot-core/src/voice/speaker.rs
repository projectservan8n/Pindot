//! The speech worker: a queue of utterances, spoken one after another on a
//! dedicated thread. Generation and playback overlap (the TTS streams PCM
//! chunks to the sink as it makes them), so the first audio starts after the
//! first chunk, not after the whole sentence. `stop` cuts the current
//! utterance and drops everything queued.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::audio::AudioSink;
use super::engines::TextToSpeech;

#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerEvent {
    /// The first audio of `seq` went to the device, `after` the `say` call.
    /// `text` is what is being said (for captions).
    FirstAudio { seq: u64, after: Duration, text: String },
    /// All of `seq`'s audio is generated: `secs` long. Generation runs ahead
    /// of playback, so this usually comes early (captions pace to it).
    Generated { seq: u64, secs: f32 },
    /// `seq` ended (played out, or cut by `stop`).
    Finished { seq: u64, interrupted: bool },
    Error { seq: u64, error: String },
    /// Nothing left to say; `seq` is the last utterance handled.
    Idle { seq: u64 },
}

struct Job {
    seq: u64,
    text: String,
    at: Instant,
    generation: u64,
}

pub struct Speaker {
    tx: Option<mpsc::Sender<Job>>,
    generation: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Speaker {
    pub fn new(
        tts: Arc<dyn TextToSpeech>,
        sink: Arc<dyn AudioSink>,
        on_event: Arc<dyn Fn(SpeakerEvent) + Send + Sync>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let generation = Arc::new(AtomicU64::new(0));
        let g = generation.clone();
        let thread = std::thread::Builder::new()
            .name("pindot-speaker".into())
            .spawn(move || worker(rx, g, tts, sink, on_event))?;
        Ok(Self { tx: Some(tx), generation, thread: Some(thread) })
    }

    pub fn say(&self, seq: u64, text: &str) {
        let job = Job { seq, text: text.to_string(), at: Instant::now(), generation: self.generation.load(Ordering::SeqCst) };
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }

    /// Stop speaking now and forget everything queued.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn worker(
    rx: mpsc::Receiver<Job>,
    generation: Arc<AtomicU64>,
    tts: Arc<dyn TextToSpeech>,
    sink: Arc<dyn AudioSink>,
    on_event: Arc<dyn Fn(SpeakerEvent) + Send + Sync>,
) {
    while let Ok(first) = rx.recv() {
        let mut next = Some(first);
        let mut last = 0;
        while let Some(job) = next.take() {
            last = job.seq;
            if job.generation == generation.load(Ordering::SeqCst) {
                speak_one(&job, &generation, &*tts, &*sink, &*on_event);
            }
            next = rx.try_recv().ok();
        }
        on_event(SpeakerEvent::Idle { seq: last });
    }
}

fn speak_one(
    job: &Job,
    generation: &AtomicU64,
    tts: &dyn TextToSpeech,
    sink: &dyn AudioSink,
    on_event: &(dyn Fn(SpeakerEvent) + Send + Sync),
) {
    let stopped = || generation.load(Ordering::SeqCst) != job.generation;
    // Unbounded: generation never blocks, so a stopped playback can't
    // deadlock the generator thread.
    let (ctx, crx) = mpsc::channel::<Vec<f32>>();
    let mut gen_result = Ok(());
    let mut play_result = Ok(());
    std::thread::scope(|s| {
        let generator = s.spawn(|| {
            let mut samples = 0usize;
            let r = tts.speak(&job.text, &mut |chunk| {
                samples += chunk.len();
                !stopped() && ctx.send(chunk.to_vec()).is_ok()
            });
            drop(ctx);
            if r.is_ok() && !stopped() {
                let secs = samples as f32 / tts.sample_rate().max(1) as f32;
                on_event(SpeakerEvent::Generated { seq: job.seq, secs });
            }
            r
        });
        play_result = sink.play(tts.sample_rate(), &mut || crx.recv().ok(), &stopped, &mut || {
            on_event(SpeakerEvent::FirstAudio { seq: job.seq, after: job.at.elapsed(), text: job.text.clone() })
        });
        gen_result = generator.join().unwrap_or_else(|_| Err(anyhow::anyhow!("speech generator panicked")));
    });
    if let Err(e) = gen_result.and(play_result) {
        tracing::warn!("speech {} failed: {e:#}", job.seq);
        on_event(SpeakerEvent::Error { seq: job.seq, error: format!("{e:#}") });
    }
    on_event(SpeakerEvent::Finished { seq: job.seq, interrupted: stopped() });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A TTS that "speaks" 10 chunks of 100 samples, slowly.
    pub struct FakeTts {
        pub chunk_delay: Duration,
    }

    impl TextToSpeech for FakeTts {
        fn sample_rate(&self) -> u32 {
            1000
        }
        fn speak(&self, _text: &str, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> anyhow::Result<()> {
            for _ in 0..10 {
                std::thread::sleep(self.chunk_delay);
                if !on_chunk(&[0.5; 100]) {
                    break;
                }
            }
            Ok(())
        }
    }

    /// A sink that counts samples instead of playing them.
    #[derive(Default)]
    pub struct CountingSink {
        pub samples: Mutex<Vec<usize>>,
    }

    impl AudioSink for CountingSink {
        fn play(
            &self,
            _rate: u32,
            next: &mut dyn FnMut() -> Option<Vec<f32>>,
            stop: &dyn Fn() -> bool,
            on_first_audio: &mut dyn FnMut(),
        ) -> anyhow::Result<()> {
            let mut n = 0;
            let mut first = true;
            while !stop() {
                match next() {
                    Some(c) => {
                        if first {
                            first = false;
                            on_first_audio();
                        }
                        n += c.len();
                    }
                    None => break,
                }
            }
            self.samples.lock().unwrap().push(n);
            Ok(())
        }
    }

    fn speaker(delay_ms: u64) -> (Speaker, Arc<CountingSink>, mpsc::Receiver<SpeakerEvent>) {
        let sink = Arc::new(CountingSink::default());
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let s = Speaker::new(
            Arc::new(FakeTts { chunk_delay: Duration::from_millis(delay_ms) }),
            sink.clone(),
            Arc::new(move |e| {
                let _ = tx.lock().unwrap().send(e);
            }),
        )
        .unwrap();
        (s, sink, rx)
    }

    fn until_idle(rx: &mpsc::Receiver<SpeakerEvent>) -> Vec<SpeakerEvent> {
        let mut v = vec![];
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(5)) {
            let idle = matches!(e, SpeakerEvent::Idle { .. });
            v.push(e);
            if idle {
                break;
            }
        }
        v
    }

    #[test]
    fn speaks_queued_utterances_in_order() {
        let (s, sink, rx) = speaker(1);
        s.say(1, "one");
        s.say(2, "two");
        let mut ev = until_idle(&rx);
        if !ev.iter().any(|e| *e == SpeakerEvent::Finished { seq: 2, interrupted: false }) {
            ev.extend(until_idle(&rx));
        }
        assert!(ev.iter().any(|e| matches!(e, SpeakerEvent::FirstAudio { seq: 1, .. })));
        // The full length is known once generated (captions pace to it).
        assert!(ev.iter().any(|e| matches!(e, SpeakerEvent::Generated { seq: 1, secs } if *secs > 0.0)), "{ev:?}");
        assert!(ev.contains(&SpeakerEvent::Finished { seq: 1, interrupted: false }));
        assert!(ev.contains(&SpeakerEvent::Finished { seq: 2, interrupted: false }));
        assert_eq!(*sink.samples.lock().unwrap(), vec![1000, 1000]);
    }

    #[test]
    fn stop_cuts_speech_and_drops_the_queue() {
        let (s, sink, rx) = speaker(30);
        s.say(1, "long");
        s.say(2, "queued");
        std::thread::sleep(Duration::from_millis(100));
        s.stop();
        let ev = until_idle(&rx);
        assert!(ev.contains(&SpeakerEvent::Finished { seq: 1, interrupted: true }), "{ev:?}");
        assert!(!ev.iter().any(|e| matches!(e, SpeakerEvent::FirstAudio { seq: 2, .. })));
        assert!(ev.contains(&SpeakerEvent::Idle { seq: 2 }));
        let played = sink.samples.lock().unwrap().clone();
        assert_eq!(played.len(), 1);
        assert!(played[0] < 1000, "cut short: {played:?}");
        // Still usable after a stop.
        s.say(3, "again");
        let ev = until_idle(&rx);
        assert!(ev.contains(&SpeakerEvent::Finished { seq: 3, interrupted: false }));
    }
}
