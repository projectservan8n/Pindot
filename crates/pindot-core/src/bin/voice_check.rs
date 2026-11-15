//! Voice checks without the app or `claude`:
//!
//!   voice_check devices                 list audio devices (opens no stream)
//!   voice_check models                  download (if needed) and verify both models
//!                                       (`--no-fallback`: the pinned source only)
//!   voice_check stt <in.wav>            transcribe a WAV with the STT model
//!   voice_check tts <text> <out.wav>    synthesize, then check duration, level
//!                                       and a transcribe-back
//!   voice_check play <in.wav> [volume]  play a WAV once on the default output
//!
//! Models come from the voice settings (`PINDOT_STT_MODEL`, ...).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use pindot_core::voice::{
    VoiceSettings,
    audio::{self, AudioSink},
    engines::{PhononTts, STT_SAMPLE_RATE, TextToSpeech, load_stt},
    models::{self, ModelKind, Progress},
};

fn progress(p: Progress) {
    match p {
        Progress::Checking { .. } => {}
        Progress::Downloading { file, done, total, .. } => {
            if done == total {
                eprintln!("downloaded {file}: {total} bytes");
            }
        }
        other => eprintln!("{other:?}"),
    }
}

fn stt_dir(s: &VoiceSettings) -> Result<PathBuf> {
    models::resolve(&s.stt_model, ModelKind::SpeechToText, &models::models_dir()?)
}

fn tts_dir(s: &VoiceSettings) -> Result<PathBuf> {
    models::resolve(&s.tts_model, ModelKind::TextToSpeech, &models::models_dir()?)
}

/// Download (when missing or damaged) a catalog model; a local folder is
/// used as is.
fn ensure(setting: &str, opts: &models::DownloadOptions) -> Result<PathBuf> {
    match models::find(setting) {
        Some(spec) => models::ensure_with(spec, &models::models_dir()?, opts, &mut progress),
        None => Ok(PathBuf::from(setting)),
    }
}

fn main() -> Result<()> {
    pindot_core::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let s = VoiceSettings::load();
    match args.first().map(String::as_str) {
        Some("devices") => {
            let d = audio::list_devices()?;
            println!("default input:  {}", d.default_input.as_deref().unwrap_or("(none)"));
            println!("default output: {}", d.default_output.as_deref().unwrap_or("(none)"));
            for i in &d.inputs {
                println!("input:  {i}");
            }
            for o in &d.outputs {
                println!("output: {o}");
            }
        }
        Some("models") => {
            let t = Instant::now();
            let opts = models::DownloadOptions {
                use_fallback: !args.iter().any(|a| a == "--no-fallback"),
                ..Default::default()
            };
            let a = ensure(&s.stt_model, &opts)?;
            let b = ensure(&s.tts_model, &opts)?;
            println!("stt: {}\ntts: {}\nverified in {:?}", a.display(), b.display(), t.elapsed());
        }
        Some("stt") => {
            let wav = PathBuf::from(args.get(1).context("stt <in.wav>")?);
            let t = Instant::now();
            let mut stt = load_stt(&stt_dir(&s)?)?;
            let load = t.elapsed();
            let (pcm, rate) = audio::read_wav_mono(&wav)?;
            let pcm = audio::resample(&pcm, rate, STT_SAMPLE_RATE);
            let t = Instant::now();
            let text = stt.transcribe(&pcm)?;
            println!(
                "audio {:.2} s, load {:?}, transcribe {:?}\ntext: {text}",
                pcm.len() as f32 / STT_SAMPLE_RATE as f32,
                load,
                t.elapsed()
            );
        }
        Some("tts") => {
            let text = args.get(1).context("tts <text> <out.wav>")?;
            let out = PathBuf::from(args.get(2).context("tts <text> <out.wav>")?);
            let t = Instant::now();
            let voice = pindot_core::voice::phonon_voices::present(s.tts_voice.as_deref(), &models::models_dir()?)?;
            let tts = PhononTts::load(&tts_dir(&s)?, voice.as_ref().map(|(n, p)| (*n, p.as_path())))?;
            let load = t.elapsed();
            let t = Instant::now();
            let mut first: Option<std::time::Duration> = None;
            let mut pcm = Vec::new();
            tts.speak(text, &mut |c| {
                if first.is_none() && !c.is_empty() {
                    first = Some(t.elapsed());
                }
                pcm.extend_from_slice(c);
                true
            })?;
            let total = t.elapsed();
            // The same again: the first utterance of a process pays for
            // warming the model up, later ones do not.
            let t2 = Instant::now();
            let mut again: Option<std::time::Duration> = None;
            tts.speak(text, &mut |c| {
                if again.is_none() && !c.is_empty() {
                    again = Some(t2.elapsed());
                }
                again.is_none()
            })?;
            println!("first audio of a second utterance: {:?}", again.unwrap_or_default());
            let rate = tts.sample_rate();
            audio::write_wav_mono(&out, &pcm, rate)?;
            let secs = pcm.len() as f32 / rate as f32;
            let level = audio::rms(&pcm);
            let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs()));
            println!(
                "load {load:?}, first audio {:?}, total {total:?}, {secs:.2} s of audio at {rate} Hz, rms {level:.4}, peak {peak:.3}",
                first.unwrap_or_default()
            );
            if secs < 0.3 || level < 0.005 {
                bail!("speech looks silent or empty");
            }
            let mut stt = load_stt(&stt_dir(&s)?)?;
            let back = stt.transcribe(&audio::resample(&pcm, rate, STT_SAMPLE_RATE))?;
            println!("transcribed back: {back}");
        }
        Some("play") => {
            let wav = PathBuf::from(args.get(1).context("play <in.wav> [volume]")?);
            let volume = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.2f32);
            let (pcm, rate) = audio::read_wav_mono(&wav)?;
            let mut chunks = pcm.chunks(4800).map(|c| c.to_vec()).collect::<Vec<_>>().into_iter();
            let t = Instant::now();
            audio::CpalSpeaker { volume }.play(rate, &mut || chunks.next(), &|| false, &mut || {
                eprintln!("first audio handed to the device after {:?}", t.elapsed())
            })?;
            println!("played {:.2} s at volume {volume} in {:?}", pcm.len() as f32 / rate as f32, t.elapsed());
        }
        _ => {
            eprintln!("usage: voice_check devices | models | stt <in.wav> | tts <text> <out.wav> | play <in.wav> [volume]");
            std::process::exit(2);
        }
    }
    Ok(())
}
