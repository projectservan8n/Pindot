//! Microphone in, speaker out (cpal), plus WAV stand-ins for tests.
//!
//! Streams live on their own threads (a cpal stream is not `Send` on every
//! platform) and are always dropped when recording or playback ends, so no
//! audio device stays open between turns.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use super::engines::STT_SAMPLE_RATE;

#[derive(Debug, Clone, Default)]
pub struct AudioDevices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

/// Enumerate audio devices. Does not open any stream (no microphone use).
pub fn list_devices() -> Result<AudioDevices> {
    let host = cpal::default_host();
    let mut d = AudioDevices {
        default_input: host.default_input_device().map(|x| x.to_string()),
        default_output: host.default_output_device().map(|x| x.to_string()),
        ..Default::default()
    };
    for dev in host.input_devices().map_err(|e| anyhow!("{e}"))? {
        d.inputs.push(dev.to_string());
    }
    for dev in host.output_devices().map_err(|e| anyhow!("{e}"))? {
        d.outputs.push(dev.to_string());
    }
    Ok(d)
}

/// Push-to-talk capture: `start` on key down, `stop` on key up.
pub trait Microphone: Send {
    fn start(&mut self) -> Result<()>;
    /// Stop and return what was heard as 16 kHz mono f32.
    fn stop(&mut self) -> Result<Vec<f32>>;
    /// Stop and throw the audio away (Esc).
    fn cancel(&mut self) {
        let _ = self.stop();
    }
}

/// Plays mono PCM. Blocks until everything was played or `stop()` is true.
pub trait AudioSink: Send + Sync {
    /// `next` yields chunks; `None` means the utterance is complete.
    /// `on_first_audio` fires when the first non-empty chunk is handed to
    /// the device (for the latency figures).
    fn play(
        &self,
        sample_rate: u32,
        next: &mut dyn FnMut() -> Option<Vec<f32>>,
        stop: &dyn Fn() -> bool,
        on_first_audio: &mut dyn FnMut(),
    ) -> Result<()>;
}

// ---------------------------------------------------------------- resampling

/// Linear-interpolation resampler for a stream of mono chunks. Downsampling
/// first averages over the ratio (a box filter) to keep aliasing down;
/// that is plenty for speech.
pub struct Resampler {
    from: u32,
    to: u32,
    /// Position of the next output sample, in input samples, relative to
    /// the start of `carry`.
    pos: f64,
    carry: Vec<f32>,
    /// Last filtered sample of the previous chunk (interpolation history).
    last: Option<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { from, to, pos: 0.0, carry: vec![], last: None }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.from == self.to {
            return input.to_vec();
        }
        let step = self.from as f64 / self.to as f64;
        let filtered: Vec<f32> = if step > 1.0 {
            let w = step.round().max(1.0) as usize;
            let mut src = self.carry.clone();
            src.extend_from_slice(input);
            // Moving average over w samples, causal.
            let mut out = Vec::with_capacity(input.len());
            let mut acc = 0.0f32;
            let start = self.carry.len();
            for i in 0..src.len() {
                acc += src[i];
                if i >= w {
                    acc -= src[i - w];
                }
                if i >= start {
                    out.push(acc / w.min(i + 1) as f32);
                }
            }
            // Keep the last w-1 raw samples for the next chunk's average.
            let keep = w.saturating_sub(1).min(src.len());
            self.carry = src[src.len() - keep..].to_vec();
            out
        } else {
            input.to_vec()
        };
        self.interp(&filtered, step)
    }

    fn interp(&mut self, input: &[f32], step: f64) -> Vec<f32> {
        // `src[0]` is the last sample of the previous chunk, when there was one.
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + 2);
        let prev = self.last.take();
        let mut src: Vec<f32> = Vec::with_capacity(input.len() + 1);
        if let Some(p) = prev {
            src.push(p);
        }
        src.extend_from_slice(input);
        if src.len() < 2 {
            self.last = src.last().copied();
            return out;
        }
        let mut pos = self.pos;
        while pos + 1.0 < src.len() as f64 {
            let i = pos.floor() as usize;
            let frac = (pos - i as f64) as f32;
            out.push(src[i] * (1.0 - frac) + src[i + 1] * frac);
            pos += step;
        }
        // Re-base so the last sample becomes index 0 of the next call.
        self.pos = pos - (src.len() - 1) as f64;
        self.last = src.last().copied();
        out
    }
}

/// Resample a whole buffer at once.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let mut r = Resampler::new(from, to);
    r.process(input)
}

// ---------------------------------------------------------------- WAV helpers

/// Read a WAV file as mono f32 at its own rate.
pub fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<std::result::Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<std::result::Result<_, _>>()?
        }
    };
    let mono = raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
    Ok((mono, spec.sample_rate))
}

/// Write mono f32 PCM as 16-bit WAV.
pub fn write_wav_mono(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).with_context(|| format!("creating {}", path.display()))?;
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

// ---------------------------------------------------------------- microphone

/// The microphone's latest level (RMS of the last callback, 0..1), for the
/// overlay's waveform. One microphone is open at a time, so one value.
static MIC_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn mic_level() -> f32 {
    f32::from_bits(MIC_LEVEL.load(std::sync::atomic::Ordering::Relaxed))
}

fn set_mic_level(v: f32) {
    MIC_LEVEL.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The default input device through cpal. Each `start` opens a fresh stream
/// on a dedicated thread; `stop` closes it.
#[derive(Default)]
pub struct CpalMicrophone {
    running: Option<(Arc<AtomicBool>, JoinHandle<Result<(Vec<f32>, u32)>>)>,
}

impl CpalMicrophone {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Microphone for CpalMicrophone {
    fn start(&mut self) -> Result<()> {
        if self.running.is_some() {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let handle = std::thread::Builder::new().name("pindot-mic".into()).spawn(move || {
            let host = cpal::default_host();
            let dev = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = ready_tx.send(Err(anyhow!("no microphone (no default input device)")));
                    bail!("no input device");
                }
            };
            let cfg = dev.default_input_config().map_err(|e| anyhow!("microphone config: {e}"))?;
            let rate = cfg.sample_rate();
            let channels = cfg.channels() as usize;
            let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
            let stream = match build_input(&dev, cfg.sample_format(), cfg.config(), channels, buf.clone()) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("opening the microphone: {e}")));
                    return Err(e);
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(anyhow!("starting the microphone: {e}")));
                bail!("{e}");
            }
            let _ = ready_tx.send(Ok(()));
            while !stop_t.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(stream);
            let samples = std::mem::take(&mut *buf.lock().unwrap());
            Ok((samples, rate))
        })?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => {
                self.running = Some((stop, handle));
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err(anyhow!("the microphone did not start within 3 s"))
            }
        }
    }

    fn stop(&mut self) -> Result<Vec<f32>> {
        set_mic_level(0.0);
        let Some((stop, handle)) = self.running.take() else {
            return Ok(vec![]);
        };
        stop.store(true, Ordering::SeqCst);
        let (samples, rate) = handle.join().map_err(|_| anyhow!("microphone thread panicked"))??;
        Ok(resample(&samples, rate, STT_SAMPLE_RATE))
    }
}

fn build_input(
    dev: &cpal::Device,
    format: SampleFormat,
    config: cpal::StreamConfig,
    channels: usize,
    buf: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream> {
    fn make<T>(
        dev: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        buf: Arc<Mutex<Vec<f32>>>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        let err = |e: cpal::Error| tracing::warn!("microphone stream error: {e}");
        dev.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let mut b = buf.lock().unwrap();
                let mut energy = 0.0f32;
                let mut n = 0usize;
                for frame in data.chunks(channels.max(1)) {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    let v = sum / frame.len() as f32;
                    energy += v * v;
                    n += 1;
                    b.push(v);
                }
                if n > 0 {
                    set_mic_level((energy / n as f32).sqrt());
                }
            },
            err,
            None,
        )
        .map_err(|e| anyhow!("{e}"))
    }
    match format {
        SampleFormat::F32 => make::<f32>(dev, config, channels, buf),
        SampleFormat::I16 => make::<i16>(dev, config, channels, buf),
        SampleFormat::I32 => make::<i32>(dev, config, channels, buf),
        SampleFormat::U16 => make::<u16>(dev, config, channels, buf),
        other => bail!("unsupported microphone sample format {other}"),
    }
}

/// TEST ONLY: a "microphone" that returns a WAV file's audio on `stop`.
pub struct WavMicrophone {
    pub path: PathBuf,
}

impl Microphone for WavMicrophone {
    fn start(&mut self) -> Result<()> {
        tracing::warn!("TEST VOICE INPUT: the microphone is replaced by {}", self.path.display());
        Ok(())
    }

    fn stop(&mut self) -> Result<Vec<f32>> {
        let (samples, rate) = read_wav_mono(&self.path)?;
        Ok(resample(&samples, rate, STT_SAMPLE_RATE))
    }
}

// ---------------------------------------------------------------- speaker

/// The default output device through cpal, at `volume` (0..1).
pub struct CpalSpeaker {
    pub volume: f32,
}

impl AudioSink for CpalSpeaker {
    fn play(
        &self,
        sample_rate: u32,
        next: &mut dyn FnMut() -> Option<Vec<f32>>,
        stop: &dyn Fn() -> bool,
        on_first_audio: &mut dyn FnMut(),
    ) -> Result<()> {
        let host = cpal::default_host();
        let dev = host.default_output_device().context("no speaker (no default output device)")?;
        let cfg = dev.default_output_config().map_err(|e| anyhow!("speaker config: {e}"))?;
        let out_rate = cfg.sample_rate();
        let channels = cfg.channels() as usize;
        let queue = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let volume = self.volume.clamp(0.0, 1.0);
        let stream = build_output(&dev, cfg.sample_format(), cfg.config(), channels, queue.clone(), volume)?;
        stream.play().map_err(|e| anyhow!("starting the speaker: {e}"))?;
        let mut rs = Resampler::new(sample_rate, out_rate);
        let mut first = true;
        // Feed while generating, but keep at most ~2 s queued.
        let max_queued = out_rate as usize * 2;
        while !stop() {
            if queue.lock().unwrap().len() > max_queued {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match next() {
                Some(chunk) => {
                    let out = rs.process(&chunk);
                    if first && !out.is_empty() {
                        first = false;
                        on_first_audio();
                    }
                    queue.lock().unwrap().extend(out);
                }
                None => break,
            }
        }
        // Drain what is queued, unless stopped.
        let deadline = Instant::now() + Duration::from_secs(30);
        while !stop() && !queue.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !stop() {
            // Let the device play out its own buffer.
            std::thread::sleep(Duration::from_millis(120));
        }
        drop(stream);
        Ok(())
    }
}

fn build_output(
    dev: &cpal::Device,
    format: SampleFormat,
    config: cpal::StreamConfig,
    channels: usize,
    queue: Arc<Mutex<VecDeque<f32>>>,
    volume: f32,
) -> Result<cpal::Stream> {
    fn make<T>(
        dev: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        queue: Arc<Mutex<VecDeque<f32>>>,
        volume: f32,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32> + Send + 'static,
    {
        let err = |e: cpal::Error| tracing::warn!("speaker stream error: {e}");
        dev.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let mut q = queue.lock().unwrap();
                for frame in data.chunks_mut(channels.max(1)) {
                    let v = q.pop_front().unwrap_or(0.0) * volume;
                    for s in frame.iter_mut() {
                        *s = T::from_sample(v);
                    }
                }
            },
            err,
            None,
        )
        .map_err(|e| anyhow!("opening the speaker: {e}"))
    }
    match format {
        SampleFormat::F32 => make::<f32>(dev, config, channels, queue, volume),
        SampleFormat::I16 => make::<i16>(dev, config, channels, queue, volume),
        SampleFormat::I32 => make::<i32>(dev, config, channels, queue, volume),
        SampleFormat::U16 => make::<u16>(dev, config, channels, queue, volume),
        other => bail!("unsupported speaker sample format {other}"),
    }
}

/// Writes every utterance to a WAV file (and optionally also plays it).
/// Used by the test path and the TTS check.
pub struct WavSink {
    pub dir: PathBuf,
    pub also: Option<Arc<dyn AudioSink>>,
    count: Mutex<u32>,
    pub written: Mutex<Vec<PathBuf>>,
}

impl WavSink {
    pub fn new(dir: PathBuf, also: Option<Arc<dyn AudioSink>>) -> Self {
        Self { dir, also, count: Mutex::new(0), written: Mutex::new(vec![]) }
    }
}

impl AudioSink for WavSink {
    fn play(
        &self,
        sample_rate: u32,
        next: &mut dyn FnMut() -> Option<Vec<f32>>,
        stop: &dyn Fn() -> bool,
        on_first_audio: &mut dyn FnMut(),
    ) -> Result<()> {
        let mut all = Vec::new();
        let mut tee = || {
            let c = next();
            if let Some(c) = &c {
                all.extend_from_slice(c);
            }
            c
        };
        match &self.also {
            Some(sink) => sink.play(sample_rate, &mut tee, stop, on_first_audio)?,
            None => {
                let mut fired = false;
                while !stop() {
                    match tee() {
                        Some(c) => {
                            if !fired && !c.is_empty() {
                                fired = true;
                                on_first_audio();
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let n = {
            let mut c = self.count.lock().unwrap();
            *c += 1;
            *c
        };
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("speech-{n:02}.wav"));
        write_wav_mono(&path, &all, sample_rate)?;
        tracing::info!("speech written to {} ({:.2} s)", path.display(), all.len() as f32 / sample_rate as f32);
        self.written.lock().unwrap().push(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_duration_and_a_tone() {
        let rate = 48_000;
        let tone: Vec<f32> = (0..rate).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin()).collect();
        // Chunked processing gives the same length as one go (within a sample).
        let mut r = Resampler::new(rate, 16_000);
        let mut out = vec![];
        for c in tone.chunks(1234) {
            out.extend(r.process(c));
        }
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        assert!(rms(&out) > 0.6, "a 440 Hz tone survives the low-pass: {}", rms(&out));
        let up = resample(&out, 16_000, 24_000);
        assert!((up.len() as i64 - 24_000).abs() <= 3, "{}", up.len());
        assert_eq!(resample(&tone[..10], rate, rate).len(), 10);
    }

    #[test]
    fn wav_round_trip() {
        let p = std::env::temp_dir().join(format!("dh-wav-{}.wav", std::process::id()));
        let s: Vec<f32> = (0..1600).map(|i| (i as f32 / 10.0).sin() * 0.5).collect();
        write_wav_mono(&p, &s, 16_000).unwrap();
        let (back, rate) = read_wav_mono(&p).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(back.len(), s.len());
        assert!((rms(&back) - rms(&s)).abs() < 0.01);
        let _ = std::fs::remove_file(&p);
    }
}
