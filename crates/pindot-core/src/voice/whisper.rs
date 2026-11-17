//! Whisper large-v3-turbo as speech to text, on the ONNX Runtime Pindot
//! already ships (the user's request, 17 Nov 2026: "a downloadable whisper
//! v3 large turbo"). The model is onnx-community/whisper-large-v3-turbo,
//! int8: an encoder (645 MB), a merged decoder with a key/value cache
//! (440 MB) and the tokenizer.
//!
//! Whisper hears 30 seconds at a time: the recording is padded to 30 s,
//! turned into 128 log-mel bands exactly as Hugging Face's
//! `WhisperFeatureExtractor` does, encoded once, then decoded greedily one
//! token at a time. The language is detected from the first decoder step
//! (English when the model is unsure, as short commands often leave it).
//! Every request costs a full encoder pass over the 30 s window: about
//! 7.7 s on a Ryzen 5 5600X CPU whatever the length (decoding a short
//! command adds about 0.7 s), against about 0.1 s for Parakeet. On the
//! project's test recording it is the only engine to get every word ("Open
//! text edit and type hello."). Seen 17 Nov 2026.

use std::borrow::Cow;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};

use super::engines::SpeechToText;

pub const SAMPLE_RATE: usize = 16_000;
pub const N_FFT: usize = 400;
pub const HOP: usize = 160;
pub const N_MELS: usize = 128;
/// 30 seconds: Whisper's window.
pub const N_SAMPLES: usize = 480_000;
pub const N_FRAMES: usize = 3_000;

pub const SOT: i64 = 50_258;
pub const EOT: i64 = 50_257;
pub const LANG_FIRST: i64 = 50_259;
pub const LANG_LAST: i64 = 50_358;
pub const ENGLISH: i64 = 50_259;
pub const TRANSCRIBE: i64 = 50_360;
pub const NO_TIMESTAMPS: i64 = 50_364;
/// Tokens never written (generation_config.json, `suppress_tokens`).
pub const SUPPRESS: &[i64] = &[
    1, 2, 7, 8, 9, 10, 14, 25, 26, 27, 28, 29, 31, 58, 59, 60, 61, 62, 63, 90, 91, 92, 93, 359, 503, 522, 542, 873, 893,
    902, 918, 922, 931, 1350, 1853, 1982, 2460, 2627, 3246, 3253, 3268, 3536, 3846, 3961, 4183, 4667, 6585, 6647, 7273,
    9061, 9383, 10428, 10929, 11938, 12033, 12331, 12562, 13793, 14157, 14635, 15265, 15618, 16553, 16604, 18362, 18956,
    20075, 21675, 22520, 26130, 26161, 26435, 28279, 29464, 31650, 32302, 32470, 36865, 42863, 47425, 49870, 50254,
    50258, 50359, 50360, 50361, 50362, 50363,
];
/// Not as the first written token: a lone space, and ending at once.
pub const BEGIN_SUPPRESS: &[i64] = &[220, EOT];
/// A sane cap on one request's text (Whisper's limit is 448 with the prompt).
pub const MAX_NEW_TOKENS: usize = 224;
/// Below this, the language guess is not trusted and English is used.
pub const LANGUAGE_CONFIDENCE: f32 = 0.5;

/// Slaney's mel scale (librosa's default, and Whisper's).
pub fn hz_to_mel(f: f64) -> f64 {
    if f < 1000.0 { 3.0 * f / 200.0 } else { 15.0 + (f / 1000.0).ln() * 27.0 / 6.4f64.ln() }
}

pub fn mel_to_hz(m: f64) -> f64 {
    if m < 15.0 { 200.0 * m / 3.0 } else { 1000.0 * ((m - 15.0) * 6.4f64.ln() / 27.0).exp() }
}

/// The `n_mels` triangular filters over the `n_fft / 2 + 1` FFT bins,
/// Slaney-normalised, from 0 Hz to half the sample rate: `[mel][bin]`.
pub fn mel_filters(n_mels: usize, n_fft: usize, sample_rate: usize) -> Vec<Vec<f32>> {
    // Hugging Face's `mel_filter_bank(..., norm="slaney", mel_scale="slaney")`.
    let bins = n_fft / 2 + 1;
    let nyquist = (sample_rate / 2) as f64;
    let fft_freqs: Vec<f64> = (0..bins).map(|i| nyquist * i as f64 / (bins - 1) as f64).collect();
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(nyquist));
    let pts: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    (0..n_mels)
        .map(|m| {
            let (left, centre, right) = (pts[m], pts[m + 1], pts[m + 2]);
            let enorm = 2.0 / (right - left);
            fft_freqs
                .iter()
                .map(|&f| {
                    let down = (f - left) / (centre - left);
                    let up = (right - f) / (right - centre);
                    (down.min(up).max(0.0) * enorm) as f32
                })
                .collect()
        })
        .collect()
}

/// Whisper's input: the recording padded or cut to 30 s, as `N_MELS` x
/// `N_FRAMES` log-mel values, row by row (`[mel][frame]`).
pub fn log_mel(samples: &[f32]) -> Vec<f32> {
    use rustfft::{FftPlanner, num_complex::Complex};
    // Pad (or cut) to 30 s, then reflect-pad half a window on each side
    // (`center=True`), as the feature extractor does.
    let mut audio = vec![0.0f32; N_SAMPLES];
    let n = samples.len().min(N_SAMPLES);
    audio[..n].copy_from_slice(&samples[..n]);
    let half = N_FFT / 2;
    let mut padded = Vec::with_capacity(N_SAMPLES + N_FFT);
    padded.extend((1..=half).rev().map(|i| audio[i]));
    padded.extend_from_slice(&audio);
    padded.extend((1..=half).map(|i| audio[N_SAMPLES - 1 - i]));
    // Periodic Hann window.
    let window: Vec<f32> =
        (0..N_FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos()).collect();
    let filters = mel_filters(N_MELS, N_FFT, SAMPLE_RATE);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N_FFT);
    let bins = N_FFT / 2 + 1;
    let mut out = vec![0.0f32; N_MELS * N_FRAMES];
    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let mut power = vec![0.0f32; bins];
    // 3001 frames are computed and the last is dropped: N_FRAMES remain.
    for t in 0..N_FRAMES {
        let start = t * HOP;
        for i in 0..N_FFT {
            buf[i] = Complex::new(padded[start + i] * window[i], 0.0);
        }
        fft.process(&mut buf);
        for (k, p) in power.iter_mut().enumerate() {
            *p = buf[k].norm_sqr();
        }
        for (m, f) in filters.iter().enumerate() {
            let e: f32 = f.iter().zip(&power).map(|(w, p)| w * p).sum();
            out[m * N_FRAMES + t] = e.max(1e-10).log10();
        }
    }
    let max = out.iter().cloned().fold(f32::MIN, f32::max);
    for v in &mut out {
        *v = (v.max(max - 8.0) + 4.0) / 4.0;
    }
    out
}

/// On a cached decoder pass, is this cache input carried over from the first
/// pass rather than taken from the pass's output? The encoder (cross-
/// attention) entries are; the decoder (self-attention) ones grow each pass.
pub fn keep_from_first_pass(cache_input: &str, cached_pass: bool) -> bool {
    cached_pass && cache_input.contains(".encoder.")
}

/// Which token comes next: the most likely one that may be written now.
pub fn next_token(logits: &[f32], first: bool) -> i64 {
    let allowed = |t: i64| {
        // Text tokens and the end; every other special token (languages,
        // tasks, timestamps) is written by Pindot or never.
        (t < EOT || t == EOT)
            && !SUPPRESS.contains(&t)
            && !(first && BEGIN_SUPPRESS.contains(&t))
    };
    logits
        .iter()
        .enumerate()
        .filter(|&(t, _)| allowed(t as i64))
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(EOT, |(t, _)| t as i64)
}

/// The language to transcribe in, from the first step's logits: the most
/// likely language token, or English when its share among the languages is
/// below `LANGUAGE_CONFIDENCE`.
pub fn pick_language(logits: &[f32]) -> i64 {
    let langs = &logits[LANG_FIRST as usize..=LANG_LAST as usize];
    let max = langs.iter().cloned().fold(f32::MIN, f32::max);
    let sum: f32 = langs.iter().map(|&l| (l - max).exp()).sum();
    let (best, score) = langs.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).expect("100 languages");
    let share = (score - max).exp() / sum;
    if share < LANGUAGE_CONFIDENCE { ENGLISH } else { LANG_FIRST + best as i64 }
}

pub struct WhisperStt {
    encoder: Session,
    decoder: Session,
    tokenizer: tokenizers::Tokenizer,
    /// The decoder's cache inputs, e.g. `past_key_values.0.decoder.key`.
    past_names: Vec<String>,
}

/// The files of the model, relative to its folder.
pub const ENCODER_FILE: &str = "onnx/encoder_model_int8.onnx";
pub const DECODER_FILE: &str = "onnx/decoder_model_merged_int8.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

impl WhisperStt {
    pub fn is_whisper_dir(dir: &Path) -> bool {
        dir.join(ENCODER_FILE).is_file() && dir.join(DECODER_FILE).is_file() && dir.join(TOKENIZER_FILE).is_file()
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let t0 = Instant::now();
        let open = |f: &str| {
            transcribe_rs::onnx::session::create_session(&dir.join(f)).map_err(|e| anyhow!("loading Whisper's {f}: {e}"))
        };
        let encoder = open(ENCODER_FILE)?;
        let decoder = open(DECODER_FILE)?;
        let tokenizer = tokenizers::Tokenizer::from_file(dir.join(TOKENIZER_FILE))
            .map_err(|e| anyhow!("loading Whisper's tokenizer: {e}"))?;
        let past_names: Vec<String> =
            decoder.inputs().iter().map(|o| o.name().to_string()).filter(|n| n.starts_with("past_key_values.")).collect();
        if past_names.is_empty() {
            return Err(anyhow!("{DECODER_FILE} is not a decoder with a cache (no past_key_values inputs)"));
        }
        tracing::info!("Whisper loaded from {} in {:?} ({} cache inputs)", dir.display(), t0.elapsed(), past_names.len());
        Ok(Self { encoder, decoder, tokenizer, past_names })
    }

    fn encode(&mut self, samples: &[f32]) -> Result<DynValue> {
        let features = Tensor::from_array((vec![1i64, N_MELS as i64, N_FRAMES as i64], log_mel(samples)))?;
        let mut out = self.encoder.run(ort::inputs!["input_features" => features])?;
        out.remove("last_hidden_state").ok_or_else(|| anyhow!("Whisper's encoder gave no last_hidden_state"))
    }

    /// One decoder pass. `past`: the cache from the last pass, or `None` for
    /// the first. Returns the logits of the last position and the new cache.
    fn step(
        &mut self,
        tokens: &[i64],
        hidden: &DynValue,
        past: Option<&[(String, DynValue)]>,
    ) -> Result<(Vec<f32>, Vec<(String, DynValue)>)> {
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = Vec::new();
        inputs.push(("input_ids".into(), Tensor::from_array((vec![1i64, tokens.len() as i64], tokens.to_vec()))?.into()));
        inputs.push(("encoder_hidden_states".into(), hidden.into()));
        inputs.push(("use_cache_branch".into(), Tensor::from_array((vec![1i64], vec![past.is_some()]))?.into()));
        match past {
            Some(cache) => {
                for (name, value) in cache {
                    inputs.push((name.as_str().into(), value.into()));
                }
            }
            None => {
                // The first pass reads no cache: empty ones of the right rank.
                for name in &self.past_names {
                    let empty = Tensor::from_array((vec![1i64, 20, 0, 64], Vec::<f32>::new()))?;
                    inputs.push((name.as_str().into(), empty.into()));
                }
            }
        }
        let mut out = self.decoder.run(inputs)?;
        let logits = {
            let (shape, data) = out["logits"].try_extract_tensor::<f32>()?;
            let vocab = *shape.last().ok_or_else(|| anyhow!("logits without a shape"))? as usize;
            data[data.len() - vocab..].to_vec()
        };
        let mut cache = Vec::with_capacity(self.past_names.len());
        for name in &self.past_names {
            let present = name.replacen("past_key_values", "present", 1);
            let fresh = out.remove(&present);
            let value = match (keep_from_first_pass(name, past.is_some()), past) {
                // The cross-attention keys and values belong to the audio and
                // never change. On a cached pass the merged decoder hands back
                // placeholders for them: using those broke every token after
                // the second ("Open text and then." for "Open text edit and
                // type hello.").
                (true, Some(cache)) => {
                    let (_, old) = cache.iter().find(|(n, _)| n == name).ok_or_else(|| anyhow!("{name} went missing"))?;
                    old.view().try_upgrade().map_err(|_| anyhow!("{name} could not be kept"))?
                }
                _ => fresh.ok_or_else(|| anyhow!("Whisper's decoder gave no {present}"))?,
            };
            cache.push((name.clone(), value));
        }
        Ok((logits, cache))
    }
}

impl SpeechToText for WhisperStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let t0 = Instant::now();
        let hidden = self.encode(samples).context("Whisper's encoder")?;
        let t_enc = t0.elapsed();
        let (first, _) = self.step(&[SOT], &hidden, None).context("Whisper's language step")?;
        let lang = pick_language(&first);
        let prompt = [SOT, lang, TRANSCRIBE, NO_TIMESTAMPS];
        let (mut logits, mut cache) = self.step(&prompt, &hidden, None).context("Whisper's decoder")?;
        let mut text_ids: Vec<u32> = Vec::new();
        for i in 0..MAX_NEW_TOKENS {
            let next = next_token(&logits, i == 0);
            if next == EOT {
                break;
            }
            text_ids.push(next as u32);
            let (l, c) = self.step(&[next], &hidden, Some(&cache)).context("Whisper's decoder")?;
            logits = l;
            cache = c;
        }
        let text = self.tokenizer.decode(&text_ids, true).map_err(|e| anyhow!("Whisper's tokenizer: {e}"))?;
        tracing::info!(
            "Whisper: {} tokens in {:?} (encoder {:?}), language token {lang}, tokens {:?}",
            text_ids.len(),
            t0.elapsed(),
            t_enc,
            &text_ids[..text_ids.len().min(40)]
        );
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real model on the test recording ("Open TextEdit and type
    /// hello"). Downloads 1.1 GB the first time into PINDOT_MODELS_DIR (or
    /// the data folder's models).
    ///   cargo test --release -p pindot-core --lib real_whisper -- --ignored --nocapture
    #[test]
    #[ignore = "downloads Whisper large-v3 turbo (1.1 GB) and runs it"]
    fn real_whisper_transcribes_the_test_recording() {
        use crate::voice::models;
        let root = models::models_dir().unwrap();
        let dir = models::ensure(&models::WHISPER_LARGE_V3_TURBO_INT8, &root, &mut |_| {}).expect("the model is there");
        let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/phase2/test-utterance.wav");
        crate::init_logging();
        let (samples, rate) = crate::voice::audio::read_wav_mono(&wav).unwrap();
        println!("recording: {:.2} s", samples.len() as f64 / 16_000.0);
        assert_eq!(rate, 16_000);
        let mut w = WhisperStt::load(&dir).unwrap();
        for round in 0..2 {
            let t = Instant::now();
            let text = w.transcribe(&samples).unwrap();
            println!("round {round}: {text:?} in {:?}", t.elapsed());
            let lower = text.to_lowercase();
            assert!(lower.contains("open") && lower.contains("hello"), "{text}");
        }
        // On pure silence turbo makes a word up ("you"), and its no-speech
        // token scores 0.000 there, so it cannot be asked. The voice loop
        // never sends silence (`audio::is_silence`, before speech to text).
        let quiet = w.transcribe(&vec![0.0; 16_000]).unwrap();
        println!("silence: {quiet:?}");
    }

    /// Prints a few frames of the test recording's features, for a check
    /// against an independent calculation.
    #[test]
    #[ignore = "prints values for a by-hand comparison"]
    fn print_features_of_the_test_recording() {
        let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/phase2/test-utterance.wav");
        let (samples, _) = crate::voice::audio::read_wav_mono(&wav).unwrap();
        let m = log_mel(&samples);
        for frame in [20usize, 60, 100] {
            let v: Vec<String> = [0usize, 10, 30, 60, 100, 127].iter().map(|&b| format!("{:.5}", m[b * N_FRAMES + frame])).collect();
            println!("frame {frame}: {}", v.join(" "));
        }
    }

    #[test]
    fn the_mel_scale_is_slaneys() {
        assert!((hz_to_mel(0.0)).abs() < 1e-9);
        assert!((hz_to_mel(500.0) - 7.5).abs() < 1e-9, "linear below 1 kHz: 3 mels per 200 Hz");
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
        assert!((hz_to_mel(2000.0) - (15.0 + 27.0 * 2f64.ln() / 6.4f64.ln())).abs() < 1e-9, "logarithmic above");
        for f in [0.0, 300.0, 999.0, 1000.0, 4321.0, 8000.0] {
            assert!((mel_to_hz(hz_to_mel(f)) - f).abs() < 1e-6, "{f} Hz round-trips");
        }
    }

    #[test]
    fn the_filters_are_triangles_that_cover_the_band() {
        let f = mel_filters(N_MELS, N_FFT, SAMPLE_RATE);
        assert_eq!(f.len(), N_MELS);
        assert!(f.iter().all(|row| row.len() == N_FFT / 2 + 1));
        assert!(f.iter().flatten().all(|&w| w >= 0.0));
        // Every filter has some weight, rises then falls once.
        for (m, row) in f.iter().enumerate() {
            let peak = row.iter().cloned().fold(0.0f32, f32::max);
            assert!(peak > 0.0, "filter {m} is empty");
        }
        // Slaney normalisation: a filter's area in Hz is about 1 (2 / width x
        // width / 2), so the low, narrow filters have the tallest peaks.
        let low = f[1].iter().cloned().fold(0.0, f32::max);
        let high = f[N_MELS - 2].iter().cloned().fold(0.0, f32::max);
        assert!(low > high * 5.0, "low {low} high {high}");
        // A bin at 1 kHz (bin 25) falls in the filters whose centres are next
        // to 15 mels: centres sit every `top / (N_MELS + 1)` mels.
        let at_1k: Vec<usize> = (0..N_MELS).filter(|&m| f[m][25] > 0.0).collect();
        let near = near_1k();
        assert!(!at_1k.is_empty() && at_1k.iter().all(|&m| m.abs_diff(near) <= 1), "{at_1k:?} vs {near}");
    }

    #[test]
    fn silence_and_a_tone_give_the_expected_features() {
        let quiet = log_mel(&[0.0; 16_000]);
        assert_eq!(quiet.len(), N_MELS * N_FRAMES, "always 30 s");
        // All silence: log10(1e-10) = -10 everywhere, (x + 4) / 4 = -1.5.
        assert!(quiet.iter().all(|&v| (v + 1.5).abs() < 1e-4), "{:?}", &quiet[..4]);

        // A 1 kHz tone for the first second: loudest in the bands round 1 kHz,
        // and the top of the range is 2 (max - 8 clamp keeps 8 dB of range).
        let tone: Vec<f32> = (0..16_000).map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16_000.0).sin()).collect();
        let m = log_mel(&tone);
        let frame = 50; // half a second in
        let col: Vec<f32> = (0..N_MELS).map(|b| m[b * N_FRAMES + frame]).collect();
        let loudest = col.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert!(loudest.abs_diff(near_1k()) <= 1, "loudest band {loudest}, expected {}", near_1k());
        let max = m.iter().cloned().fold(f32::MIN, f32::max);
        let min = m.iter().cloned().fold(f32::MAX, f32::min);
        assert!((max - min - 2.0).abs() < 1e-4, "range {min}..{max}: 8 log10 units / 4");
        // After the tone the frames are at the floor.
        assert!((m[loudest * N_FRAMES + 2_000] - min).abs() < 1e-4);
    }

    #[test]
    fn a_recording_longer_than_30_s_is_cut() {
        let long = vec![0.1f32; N_SAMPLES + 16_000];
        assert_eq!(log_mel(&long).len(), N_MELS * N_FRAMES);
    }

    /// The band whose centre is nearest 1 kHz (15 mels): 41 or 42.
    fn near_1k() -> usize {
        let step = hz_to_mel(8000.0) / (N_MELS + 1) as f64;
        (hz_to_mel(1000.0) / step).round() as usize - 1
    }

    fn logits_with(best: &[(i64, f32)]) -> Vec<f32> {
        let mut l = vec![0.0f32; 51_866];
        for &(t, v) in best {
            l[t as usize] = v;
        }
        l
    }

    #[test]
    fn the_audio_part_of_the_cache_is_kept_from_the_first_pass() {
        assert!(keep_from_first_pass("past_key_values.0.encoder.key", true));
        assert!(keep_from_first_pass("past_key_values.3.encoder.value", true));
        assert!(!keep_from_first_pass("past_key_values.0.decoder.key", true), "self-attention grows");
        assert!(!keep_from_first_pass("past_key_values.0.encoder.key", false), "the first pass makes it");
    }

    #[test]
    fn the_next_token_skips_suppressed_and_special_tokens() {
        assert_eq!(next_token(&logits_with(&[(1000, 5.0)]), false), 1000);
        // Suppressed and special tokens never win.
        assert_eq!(next_token(&logits_with(&[(359, 9.0), (1000, 5.0)]), false), 1000);
        assert_eq!(next_token(&logits_with(&[(50_365, 9.0), (1000, 5.0)]), false), 1000, "a timestamp");
        assert_eq!(next_token(&logits_with(&[(ENGLISH, 9.0), (1000, 5.0)]), false), 1000, "a language");
        // The end may come, but not as the first token, nor a lone space.
        assert_eq!(next_token(&logits_with(&[(EOT, 9.0), (1000, 5.0)]), false), EOT);
        assert_eq!(next_token(&logits_with(&[(EOT, 9.0), (1000, 5.0)]), true), 1000);
        assert_eq!(next_token(&logits_with(&[(220, 9.0), (1000, 5.0)]), true), 1000);
    }

    #[test]
    fn the_language_is_the_confident_guess_or_english() {
        let tagalog = 50_348;
        assert_eq!(pick_language(&logits_with(&[(tagalog, 10.0), (ENGLISH, 1.0)])), tagalog);
        // Two languages neck and neck with others: not sure, so English.
        let unsure = logits_with(&[(50_262, 3.0), (50_263, 3.0), (50_264, 3.0)]);
        assert_eq!(pick_language(&unsure), ENGLISH);
        // A non-language token with a higher score is ignored.
        assert_eq!(pick_language(&logits_with(&[(1000, 50.0), (50_261, 10.0)])), 50_261);
    }
}
