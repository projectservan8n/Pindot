//! Speech to text and text to speech behind small traits, so the voice loop
//! can be tested with mocks and the models stay swappable settings.
//!
//! - `MoonshineStt`: Moonshine v2 streaming through transcribe-rs (ONNX
//!   Runtime). It takes the whole push-to-talk buffer at release.
//! - `PhononTts`: Phonon through the ptts runtime, CPU, 8-bit weights. It
//!   streams PCM chunks as they are generated, so playback starts before
//!   the sentence is finished.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};

/// Moonshine and every STT model here take 16 kHz mono f32.
pub const STT_SAMPLE_RATE: u32 = 16_000;

pub trait SpeechToText: Send {
    /// `samples`: 16 kHz mono f32 in [-1, 1]. Returns the text (may be empty).
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

pub trait TextToSpeech: Send + Sync {
    fn sample_rate(&self) -> u32;
    /// Synthesize `text`, handing mono PCM chunks to `on_chunk` as they come.
    /// `on_chunk` returns false to stop early (the user interrupted).
    fn speak(&self, text: &str, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<()>;
}

/// Load the speech-to-text engine that matches the files in `dir`: Parakeet
/// has `nemo128.onnx`, anything else is treated as Moonshine.
pub fn load_stt(dir: &Path) -> Result<Box<dyn SpeechToText>> {
    if dir.join("nemo128.onnx").exists() {
        Ok(Box::new(ParakeetStt::load(dir)?))
    } else {
        Ok(Box::new(MoonshineStt::load(dir)?))
    }
}

pub struct ParakeetStt {
    model: transcribe_rs::onnx::parakeet::ParakeetModel,
}

impl ParakeetStt {
    pub fn load(dir: &Path) -> Result<Self> {
        let t0 = Instant::now();
        let model = transcribe_rs::onnx::parakeet::ParakeetModel::load(dir, &transcribe_rs::onnx::Quantization::Int8)
            .map_err(|e| anyhow!("loading Parakeet from {}: {e}", dir.display()))?;
        tracing::info!("Parakeet loaded from {} in {:?}", dir.display(), t0.elapsed());
        Ok(Self { model })
    }
}

impl SpeechToText for ParakeetStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let params = transcribe_rs::onnx::parakeet::ParakeetParams::default();
        let r = self.model.transcribe_with(samples, &params).map_err(|e| anyhow!("Parakeet: {e}"))?;
        Ok(r.text.trim().to_string())
    }
}

pub struct MoonshineStt {
    model: transcribe_rs::onnx::moonshine::StreamingModel,
}

impl MoonshineStt {
    pub fn load(dir: &Path) -> Result<Self> {
        let threads = std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2);
        let t0 = Instant::now();
        let model = transcribe_rs::onnx::moonshine::StreamingModel::load(
            dir,
            threads,
            &transcribe_rs::onnx::Quantization::FP32,
        )
        .map_err(|e| anyhow!("loading Moonshine from {}: {e}", dir.display()))?;
        tracing::info!("Moonshine loaded from {} in {:?} ({threads} threads)", dir.display(), t0.elapsed());
        Ok(Self { model })
    }
}

impl SpeechToText for MoonshineStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let params = transcribe_rs::onnx::moonshine::MoonshineStreamingParams::default();
        let r = self.model.transcribe_with(samples, &params).map_err(|e| anyhow!("Moonshine: {e}"))?;
        Ok(r.text.trim().to_string())
    }
}

pub struct PhononTts {
    synth: ptts::synth::Synth,
    options: ptts::synth::SpeechOptions,
}

impl PhononTts {
    /// `voice`: an extra voice (its name and embedding file, see
    /// `phonon_voices`); `None` = the checkpoint's own default voice.
    pub fn load(dir: &Path, voice: Option<(&str, &Path)>) -> Result<Self> {
        use ptts::checkpoint::{Checkpoint, ResolveOptions};
        let t0 = Instant::now();
        let checkpoint = Checkpoint::resolve(dir, ResolveOptions { quant: ptts::synth::Quant::Q80, weights: None })
            .map_err(|e| anyhow!("Phonon checkpoint in {}: {e}", dir.display()))?;
        let mut synth = checkpoint
            .builder(ptts::preprocess::Lang::En)
            .device(ptts::synth::DeviceKind::Cpu)
            .build()
            .map_err(|e| anyhow!("loading Phonon: {e}"))?;
        checkpoint.register_voices(&mut synth);
        let mut options = ptts::synth::SpeechOptions::default();
        if let Some((name, path)) = voice {
            synth.add_voice_file(name, path).map_err(|e| anyhow!("Phonon voice {name}: {e}"))?;
            options = options.voice(name);
        }
        tracing::info!("Phonon loaded from {} in {:?}", dir.display(), t0.elapsed());
        Ok(Self { synth, options })
    }
}

impl TextToSpeech for PhononTts {
    fn sample_rate(&self) -> u32 {
        self.synth.sample_rate()
    }

    fn speak(&self, text: &str, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<()> {
        let stream = self.synth.stream_with(text, &self.options).map_err(|e| anyhow!("Phonon: {e}"))?;
        for chunk in stream {
            let chunk = chunk.map_err(|e| anyhow!("Phonon: {e}")).context("generating speech")?;
            if !on_chunk(&chunk) {
                // Dropping the stream stops the generation threads.
                break;
            }
        }
        Ok(())
    }
}
