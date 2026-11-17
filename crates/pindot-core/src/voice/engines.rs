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

/// The key of a cloud speech service, or what to do when there is none.
pub fn cloud_key(c: &super::cloud::Cloud, keys: &dyn crate::brain::native::secrets::SecretStore) -> Result<String> {
    match keys.get(c.key_id()) {
        Ok(Some(k)) => Ok(k),
        Ok(None) => Err(anyhow!("There is no {} key yet. Add it in Settings, under Listening or Speaking.", c.service())),
        Err(e) => Err(anyhow!("The {} key could not be read from the system's credential store: {e}", c.service())),
    }
}

/// The listening engine a setting names: a cloud service with its key, or
/// a downloaded model.
pub fn load_stt_setting(
    setting: &str,
    root: &Path,
    keys: &dyn crate::brain::native::secrets::SecretStore,
) -> Result<Box<dyn SpeechToText>> {
    use super::cloud::{Cloud, ELEVENLABS_URL, ElevenLabsStt, OPENROUTER_URL, OpenRouterStt};
    if let Some(c) = super::cloud::parse_stt(setting) {
        let key = cloud_key(&c, keys)?;
        tracing::info!("speech to text: {} ({setting})", c.service());
        return Ok(match c {
            Cloud::ElevenLabs { model } => Box::new(ElevenLabsStt { base_url: ELEVENLABS_URL.into(), key, model }),
            Cloud::OpenRouter { model } => Box::new(OpenRouterStt { base_url: OPENROUTER_URL.into(), key, model }),
        });
    }
    let dir = super::models::resolve(setting, super::models::ModelKind::SpeechToText, root)?;
    load_stt(&dir)
}

/// The speaking engine a setting names. `phonon_voice`: a downloaded Phonon
/// voice (name, file); `elevenlabs_voice`: the voice id for ElevenLabs.
pub fn load_tts_setting(
    setting: &str,
    phonon_voice: Option<(&str, &Path)>,
    elevenlabs_voice: Option<&str>,
    root: &Path,
    keys: &dyn crate::brain::native::secrets::SecretStore,
) -> Result<std::sync::Arc<dyn TextToSpeech>> {
    use super::cloud::{Cloud, ELEVENLABS_URL, ElevenLabsTts};
    if let Some(c) = super::cloud::parse_tts(setting) {
        let key = cloud_key(&c, keys)?;
        let Cloud::ElevenLabs { model } = c else { unreachable!("parse_tts gives ElevenLabs only") };
        let voice_id = elevenlabs_voice
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow!("Pick an ElevenLabs voice in Settings > Speaking."))?;
        tracing::info!("text to speech: ElevenLabs ({model})");
        return Ok(std::sync::Arc::new(ElevenLabsTts { base_url: ELEVENLABS_URL.into(), key, model, voice_id: voice_id.into() }));
    }
    let dir = super::models::resolve(setting, super::models::ModelKind::TextToSpeech, root)?;
    Ok(std::sync::Arc::new(PhononTts::load(&dir, phonon_voice)?))
}

/// Can this setting run now: a cloud service whose key is saved, a model
/// that is downloaded, or a folder that exists.
pub fn setting_ready(
    setting: &str,
    kind: super::models::ModelKind,
    root: Option<&Path>,
    keys: &dyn crate::brain::native::secrets::SecretStore,
) -> bool {
    use super::models::{self, ModelKind};
    let cloud = match kind {
        ModelKind::SpeechToText => super::cloud::parse_stt(setting),
        ModelKind::TextToSpeech => super::cloud::parse_tts(setting),
    };
    if let Some(c) = cloud {
        return matches!(keys.get(c.key_id()), Ok(Some(_)));
    }
    if setting.starts_with(super::cloud::PREFIX) {
        return false;
    }
    let p = Path::new(setting);
    if p.is_absolute() {
        return p.is_dir();
    }
    match (models::find(setting), root) {
        (Some(spec), Some(root)) => spec.kind == kind && models::is_present(spec, root),
        _ => false,
    }
}

/// Load the speech-to-text engine that matches the files in `dir`: Parakeet
/// has `nemo128.onnx`, Whisper its `onnx/` encoder and decoder with a
/// tokenizer, anything else is treated as Moonshine.
pub fn load_stt(dir: &Path) -> Result<Box<dyn SpeechToText>> {
    if super::whisper::WhisperStt::is_whisper_dir(dir) {
        Ok(Box::new(super::whisper::WhisperStt::load(dir)?))
    } else if dir.join("nemo128.onnx").exists() {
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
        let tts = Self { synth, options };
        // The first utterance of a process pays for warming the model up:
        // about half a second on a desktop CPU, where later ones start in
        // under 0.1 s. Pay it here, while the voice loop starts, not in
        // front of the first reply. One chunk is enough; nothing is played.
        let warm = Instant::now();
        if let Err(e) = tts.speak("Hi.", &mut |_| false) {
            tracing::warn!("warming Phonon up failed (the first reply will start late): {e:#}");
        }
        tracing::info!("Phonon loaded from {} in {:?} (warm-up {:?})", dir.display(), t0.elapsed(), warm.elapsed());
        Ok(tts)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::native::secrets::{MemorySecrets, SecretStore};
    use crate::voice::cloud;
    use crate::voice::models::ModelKind;

    fn err_of<T>(r: Result<T>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => format!("{e:#}"),
        }
    }

    #[test]
    fn a_cloud_setting_is_ready_once_its_key_is_saved() {
        let keys = MemorySecrets::default();
        let root = std::env::temp_dir().join("pindot-no-models-here");
        assert!(!setting_ready(cloud::ELEVENLABS_STT_SETTING, ModelKind::SpeechToText, Some(&root), &keys));
        keys.set("elevenlabs", "xi-abc").unwrap();
        assert!(setting_ready(cloud::ELEVENLABS_STT_SETTING, ModelKind::SpeechToText, Some(&root), &keys));
        assert!(setting_ready(cloud::ELEVENLABS_TTS_SETTING, ModelKind::TextToSpeech, None, &keys));
        assert!(!setting_ready("cloud:openrouter:openai/whisper-large-v3", ModelKind::SpeechToText, None, &keys));
        keys.set("openrouter", "sk-or-1").unwrap();
        assert!(setting_ready("cloud:openrouter:openai/whisper-large-v3", ModelKind::SpeechToText, None, &keys));
        // A cloud listening setting is not a speaking one.
        assert!(!setting_ready("cloud:openrouter:openai/whisper-large-v3", ModelKind::TextToSpeech, None, &keys));
        // A model that is not downloaded is not ready.
        assert!(!setting_ready("parakeet-tdt-0.6b-v3-int8", ModelKind::SpeechToText, Some(&root), &keys));
    }

    #[test]
    fn a_cloud_engine_needs_its_key_and_says_where_to_add_it() {
        let keys = MemorySecrets::default();
        let root = std::env::temp_dir();
        let e = err_of(load_stt_setting(cloud::ELEVENLABS_STT_SETTING, &root, &keys));
        assert!(e.contains("ElevenLabs") && e.contains("Settings"), "{e}");
        let e = err_of(load_stt_setting("cloud:openrouter:openai/whisper-large-v3", &root, &keys));
        assert!(e.contains("OpenRouter") && e.contains("key"), "{e}");
        keys.set("elevenlabs", "xi-abc").unwrap();
        assert!(load_stt_setting(cloud::ELEVENLABS_STT_SETTING, &root, &keys).is_ok(), "no network to build it");
        let e = err_of(load_tts_setting(cloud::ELEVENLABS_TTS_SETTING, None, None, &root, &keys));
        assert!(e.contains("voice"), "{e}");
        let tts = load_tts_setting(cloud::ELEVENLABS_TTS_SETTING, None, Some("v123"), &root, &keys).unwrap();
        assert_eq!(tts.sample_rate(), cloud::TTS_RATE);
    }
}
