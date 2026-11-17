//! Speech to text and text to speech by a cloud service, when the user picks
//! one (17 Nov 2026: "add elevenlabs for transcription and voice if they want
//! that", "openrouter and their transcription models"):
//!
//! - ElevenLabs Scribe (listening): `POST /v1/speech-to-text`, the recording
//!   as raw 16 kHz 16-bit mono PCM (`file_format=pcm_s16le_16`, the
//!   documented low-latency form), audio-event tags off.
//! - OpenRouter's transcription models (listening): `POST
//!   /api/v1/audio/transcriptions`, JSON with the recording as base64 WAV.
//! - ElevenLabs Flash v2.5 (speaking): `POST
//!   /v1/text-to-speech/{voice}/stream?output_format=pcm_24000`, read as it
//!   arrives so speech starts before the sentence is done.
//!
//! A setting names one as `cloud:<service>:<model>`. The key comes from the
//! OS credential store, the same one as the brains' keys (OpenRouter's key
//! is shared with the brain). What the user says, or what Rusty says, goes to
//! that service; Settings says so. Nothing here keeps audio.

use std::io::Read;
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::Engine;
use serde_json::{Value, json};

use super::engines::{STT_SAMPLE_RATE, SpeechToText, TextToSpeech};

pub const PREFIX: &str = "cloud:";
pub const ELEVENLABS: &str = "elevenlabs";
pub const OPENROUTER: &str = "openrouter";
pub const ELEVENLABS_URL: &str = "https://api.elevenlabs.io";
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
/// ElevenLabs' current model for recorded audio (Nov 2026).
pub const SCRIBE_MODEL: &str = "scribe_v3";
/// ElevenLabs' low-latency voice for a REST call (Nov 2026).
pub const FLASH_MODEL: &str = "eleven_flash_v2_5";
/// No plan restriction is documented below 44.1 kHz.
pub const TTS_RATE: u32 = 24_000;
pub const ELEVENLABS_STT_SETTING: &str = "cloud:elevenlabs:scribe_v3";
pub const ELEVENLABS_TTS_SETTING: &str = "cloud:elevenlabs:eleven_flash_v2_5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cloud {
    ElevenLabs { model: String },
    OpenRouter { model: String },
}

impl Cloud {
    /// The credential-store name of this service's key.
    pub fn key_id(&self) -> &'static str {
        match self {
            Cloud::ElevenLabs { .. } => ELEVENLABS,
            Cloud::OpenRouter { .. } => OPENROUTER,
        }
    }
    pub fn service(&self) -> &'static str {
        match self {
            Cloud::ElevenLabs { .. } => "ElevenLabs",
            Cloud::OpenRouter { .. } => "OpenRouter",
        }
    }
}

/// A listening setting that names a cloud service, if this one does.
pub fn parse_stt(setting: &str) -> Option<Cloud> {
    let rest = setting.strip_prefix(PREFIX)?;
    let (service, model) = rest.split_once(':')?;
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    match service {
        ELEVENLABS => Some(Cloud::ElevenLabs { model: model.into() }),
        OPENROUTER => Some(Cloud::OpenRouter { model: model.into() }),
        _ => None,
    }
}

/// A speaking setting that names a cloud service (ElevenLabs only).
pub fn parse_tts(setting: &str) -> Option<Cloud> {
    parse_stt(setting).filter(|c| matches!(c, Cloud::ElevenLabs { .. }))
}

/// 16-bit little-endian PCM of `samples` (clipped to [-1, 1]).
pub fn pcm16(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).to_le_bytes()).collect()
}

/// A 16 kHz mono 16-bit WAV file of `samples`.
pub fn wav16k(samples: &[f32]) -> Vec<u8> {
    let data = pcm16(samples);
    let rate = STT_SAMPLE_RATE;
    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}

/// A multipart/form-data body: text fields, then one file. Returns the
/// Content-Type (with its boundary) and the body.
pub fn multipart(fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> (String, Vec<u8>) {
    let boundary = format!("pindot-{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    }
    let (filename, content_type, data) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// The transcript in a `{"text": ...}` reply.
pub fn text_of(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body).map_err(|_| anyhow!("the transcription reply was not understood"))?;
    v["text"].as_str().map(|t| t.trim().to_string()).ok_or_else(|| anyhow!("the transcription reply had no text"))
}

/// A failed call in plain words, for the overlay and the log. Never holds
/// the key (the request is not echoed).
pub fn error_words(service: &str, status: u16, body: &str) -> String {
    let said = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        [&v["detail"]["message"], &v["error"]["message"], &v["detail"], &v["message"]]
            .into_iter()
            .find_map(|x| x.as_str().map(str::to_string))
    });
    let said = said.map(|m| m.chars().take(200).collect::<String>());
    match status {
        401 | 403 => format!("{service} refused the key. Check the {service} key in Settings."),
        402 => format!("{service}: {}", said.unwrap_or_else(|| "the account is out of credit.".into())),
        429 => format!(
            "{service} is over its limit right now{}. Try again in a moment.",
            said.map(|m| format!(" ({m})")).unwrap_or_default()
        ),
        _ => format!("{service} answered {status}{}", said.map(|m| format!(": {m}")).unwrap_or_default()),
    }
}

/// Turns a byte stream of 16-bit little-endian PCM into samples, keeping an
/// odd byte for the next read.
#[derive(Default)]
pub struct PcmDecoder {
    odd: Option<u8>,
}

impl PcmDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<f32> {
        let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
        let mut it = bytes.iter().copied();
        if let Some(lo) = self.odd.take() {
            match it.next() {
                Some(hi) => out.push(i16::from_le_bytes([lo, hi]) as f32 / 32_768.0),
                None => {
                    self.odd = Some(lo);
                    return out;
                }
            }
        }
        loop {
            match (it.next(), it.next()) {
                (Some(lo), Some(hi)) => out.push(i16::from_le_bytes([lo, hi]) as f32 / 32_768.0),
                (Some(lo), None) => {
                    self.odd = Some(lo);
                    break;
                }
                _ => break,
            }
        }
        out
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build(),
    )
}

fn read_body(resp: ureq::http::Response<ureq::Body>) -> (u16, String) {
    let status = resp.status().as_u16();
    let body = resp.into_body().with_config().limit(4 * 1024 * 1024).read_to_string().unwrap_or_default();
    (status, body)
}

/// ElevenLabs Scribe.
pub struct ElevenLabsStt {
    pub base_url: String,
    pub key: String,
    pub model: String,
}

impl SpeechToText for ElevenLabsStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let pcm = pcm16(samples);
        let (content_type, body) = multipart(
            &[("model_id", &self.model), ("file_format", "pcm_s16le_16"), ("tag_audio_events", "false")],
            ("audio.pcm", "application/octet-stream", &pcm),
        );
        let url = format!("{}/v1/speech-to-text", self.base_url);
        let resp = agent(Duration::from_secs(60))
            .post(&url)
            .header("xi-api-key", &self.key)
            .header("Content-Type", &content_type)
            .send(&body[..])
            .map_err(|e| anyhow!("ElevenLabs could not be reached: {e}"))?;
        let (status, body) = read_body(resp);
        if status >= 400 {
            return Err(anyhow!(error_words("ElevenLabs", status, &body)));
        }
        text_of(&body)
    }
}

/// An OpenRouter transcription model.
pub struct OpenRouterStt {
    pub base_url: String,
    pub key: String,
    pub model: String,
}

impl SpeechToText for OpenRouterStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let data = base64::engine::general_purpose::STANDARD.encode(wav16k(samples));
        let body = json!({ "model": self.model, "input_audio": { "data": data, "format": "wav" } });
        let url = format!("{}/audio/transcriptions", self.base_url);
        let resp = agent(Duration::from_secs(60))
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(&body.to_string())
            .map_err(|e| anyhow!("OpenRouter could not be reached: {e}"))?;
        let (status, body) = read_body(resp);
        if status >= 400 {
            return Err(anyhow!(error_words("OpenRouter", status, &body)));
        }
        text_of(&body)
    }
}

/// ElevenLabs text to speech, streamed as raw PCM.
pub struct ElevenLabsTts {
    pub base_url: String,
    pub key: String,
    pub model: String,
    pub voice_id: String,
}

impl TextToSpeech for ElevenLabsTts {
    fn sample_rate(&self) -> u32 {
        TTS_RATE
    }

    fn speak(&self, text: &str, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let url = format!("{}/v1/text-to-speech/{}/stream?output_format=pcm_{TTS_RATE}", self.base_url, self.voice_id);
        let body = json!({ "text": text, "model_id": self.model });
        let resp = agent(Duration::from_secs(120))
            .post(&url)
            .header("xi-api-key", &self.key)
            .header("Content-Type", "application/json")
            .send(&body.to_string())
            .map_err(|e| anyhow!("ElevenLabs could not be reached: {e}"))?;
        if resp.status().as_u16() >= 400 {
            let (status, body) = read_body(resp);
            return Err(anyhow!(error_words("ElevenLabs", status, &body)));
        }
        let mut reader = resp.into_body().into_reader();
        let mut decoder = PcmDecoder::default();
        let mut buf = vec![0u8; 4_800];
        loop {
            let n = reader.read(&mut buf).map_err(|e| anyhow!("ElevenLabs' audio stopped: {e}"))?;
            if n == 0 {
                return Ok(());
            }
            let samples = decoder.push(&buf[..n]);
            if !samples.is_empty() && !on_chunk(&samples) {
                return Ok(());
            }
        }
    }
}

/// The voices of the account behind `key`: `(voice_id, name)`.
pub fn elevenlabs_voices(base_url: &str, key: &str) -> Result<Vec<(String, String)>> {
    let url = format!("{base_url}/v2/voices?page_size=100");
    let resp = agent(Duration::from_secs(20))
        .get(&url)
        .header("xi-api-key", key)
        .call()
        .map_err(|e| anyhow!("ElevenLabs could not be reached: {e}"))?;
    let (status, body) = read_body(resp);
    if status >= 400 {
        return Err(anyhow!(error_words("ElevenLabs", status, &body)));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| anyhow!("ElevenLabs' voice list was not understood"))?;
    Ok(v["voices"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["voice_id"].as_str()?.to_string(), x["name"].as_str().unwrap_or("?").to_string())))
                .collect()
        })
        .unwrap_or_default())
}

/// OpenRouter's transcription models (public list, no key): `(id, name)`.
pub fn openrouter_stt_models(base_url: &str) -> Result<Vec<(String, String)>> {
    let url = format!("{base_url}/models?output_modalities=transcription");
    let resp = agent(Duration::from_secs(20)).get(&url).call().map_err(|e| anyhow!("OpenRouter could not be reached: {e}"))?;
    let (status, body) = read_body(resp);
    if status >= 400 {
        return Err(anyhow!(error_words("OpenRouter", status, &body)));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| anyhow!("OpenRouter's model list was not understood"))?;
    let mut out: Vec<(String, String)> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["id"].as_str()?.to_string(), x["name"].as_str().unwrap_or("?").to_string())))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_setting_names_a_cloud_service_and_its_model() {
        assert_eq!(parse_stt("cloud:elevenlabs:scribe_v3"), Some(Cloud::ElevenLabs { model: "scribe_v3".into() }));
        assert_eq!(
            parse_stt("cloud:openrouter:openai/whisper-large-v3-turbo"),
            Some(Cloud::OpenRouter { model: "openai/whisper-large-v3-turbo".into() }),
            "a model id may hold a slash"
        );
        assert_eq!(parse_tts("cloud:elevenlabs:eleven_flash_v2_5"), Some(Cloud::ElevenLabs { model: "eleven_flash_v2_5".into() }));
        for local in ["parakeet-tdt-0.6b-v3-int8", "/opt/models/x", "cloud:", "cloud:elevenlabs:", "cloud:nope:m"] {
            assert_eq!(parse_stt(local), None, "{local}");
        }
        assert_eq!(parse_tts("cloud:openrouter:some/tts"), None, "OpenRouter does not speak here");
        assert_eq!(Cloud::OpenRouter { model: "m".into() }.key_id(), "openrouter", "the brain's key");
        assert_eq!(parse_stt(ELEVENLABS_STT_SETTING).unwrap().key_id(), "elevenlabs");
        assert_eq!(parse_tts(ELEVENLABS_TTS_SETTING), Some(Cloud::ElevenLabs { model: FLASH_MODEL.into() }));
    }

    #[test]
    fn audio_is_sent_as_16_bit_pcm_and_wav() {
        assert_eq!(pcm16(&[0.0, 1.0, -1.0, 2.0]), vec![0, 0, 0xff, 0x7f, 0x01, 0x80, 0xff, 0x7f], "clipped, little-endian");
        let wav = wav16k(&[0.0; 10]);
        assert_eq!(wav.len(), 44 + 20);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), STT_SAMPLE_RATE);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16, "16-bit");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 20);
    }

    #[test]
    fn a_form_holds_the_fields_then_the_file() {
        let (ct, body) = multipart(&[("model_id", "scribe_v3")], ("audio.pcm", "application/octet-stream", b"\x01\x02"));
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").expect("content type");
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("Content-Disposition: form-data; name=\"model_id\"\r\n\r\nscribe_v3\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"audio.pcm\"\r\nContent-Type: application/octet-stream\r\n\r\n\u{1}\u{2}\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn replies_and_errors_in_plain_words() {
        assert_eq!(text_of(r#"{"text":" open notepad ","language_code":"en"}"#).unwrap(), "open notepad");
        assert!(text_of("not json").is_err());
        assert!(text_of(r#"{"words":[]}"#).is_err());
        let refused = error_words("ElevenLabs", 401, r#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#);
        assert!(refused.contains("ElevenLabs") && refused.contains("key"), "{refused}");
        let quota = error_words("ElevenLabs", 429, "{}");
        assert!(quota.contains("limit") || quota.contains("quota"), "{quota}");
        let credit = error_words("OpenRouter", 402, r#"{"error":{"message":"Insufficient credits"}}"#);
        assert!(credit.contains("Insufficient credits"), "{credit}");
        let other = error_words("OpenRouter", 500, r#"{"error":{"message":"upstream timed out"}}"#);
        assert!(other.contains("upstream timed out") && other.contains("500"), "{other}");
    }

    #[test]
    fn pcm_split_across_reads_comes_out_whole() {
        let mut d = PcmDecoder::default();
        let a = d.push(&[0x00, 0x40, 0x00]);
        assert_eq!(a, vec![0.5]);
        let b = d.push(&[0xc0]);
        assert_eq!(b, vec![-0.5], "the odd byte waited for its partner");
        assert!(d.push(&[]).is_empty());
    }

    /// A one-request server that records what it got and gives `reply`.
    fn serve(reply: &'static str, status: u16) -> (String, Arc<Mutex<(String, Vec<(String, String)>, Vec<u8>)>>) {
        serve_bytes(reply.as_bytes(), status)
    }

    /// The same, with a reply of raw bytes (audio).
    fn serve_bytes(reply: &'static [u8], status: u16) -> (String, Arc<Mutex<(String, Vec<(String, String)>, Vec<u8>)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new((String::new(), vec![], vec![])));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut headers = vec![];
            let mut len = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                let h = h.trim_end().to_string();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h.split_once(": ").unwrap();
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.parse().unwrap();
                }
                headers.push((k.to_lowercase(), v.to_string()));
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            *s2.lock().unwrap() = (line.trim_end().to_string(), headers, body);
            let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(reply).unwrap();
        });
        (base, seen)
    }

    #[test]
    fn scribe_gets_the_recording_as_raw_pcm_with_the_key_header() {
        let (base, seen) = serve(r#"{"text":"hello there"}"#, 200);
        let mut stt = ElevenLabsStt { base_url: base, key: "xi-test".into(), model: SCRIBE_MODEL.into() };
        assert_eq!(stt.transcribe(&[0.25; 1600]).unwrap(), "hello there");
        let (line, headers, body) = seen.lock().unwrap().clone();
        assert_eq!(line, "POST /v1/speech-to-text HTTP/1.1");
        assert!(headers.contains(&("xi-api-key".into(), "xi-test".into())));
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("scribe_v3") && body.contains("pcm_s16le_16") && body.contains("name=\"tag_audio_events\"\r\n\r\nfalse"));
        assert!(body.matches("name=\"file\"").count() == 1);
    }

    #[test]
    fn openrouter_gets_base64_wav_and_a_bearer_key() {
        let (base, seen) = serve(r#"{"text":"open notepad","usage":{"seconds":1}}"#, 200);
        let mut stt = OpenRouterStt { base_url: base, key: "sk-or-test".into(), model: "openai/whisper-large-v3-turbo".into() };
        assert_eq!(stt.transcribe(&[0.25; 1600]).unwrap(), "open notepad");
        let (line, headers, body) = seen.lock().unwrap().clone();
        assert_eq!(line, "POST /audio/transcriptions HTTP/1.1");
        assert!(headers.contains(&("authorization".into(), "Bearer sk-or-test".into())));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["model"], "openai/whisper-large-v3-turbo");
        assert_eq!(v["input_audio"]["format"], "wav");
        let wav = base64::engine::general_purpose::STANDARD.decode(v["input_audio"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(wav.len(), 44 + 3200);
    }

    #[test]
    fn a_refused_key_is_an_error_that_does_not_hold_it() {
        let (base, _) = serve(r#"{"detail":{"message":"Invalid API key"}}"#, 401);
        let mut stt = ElevenLabsStt { base_url: base, key: "xi-secret-123".into(), model: SCRIBE_MODEL.into() };
        let e = format!("{:#}", stt.transcribe(&[0.25; 1600]).unwrap_err());
        assert!(e.contains("ElevenLabs") && !e.contains("xi-secret-123"), "{e}");
    }

    #[test]
    fn elevenlabs_speech_streams_pcm_to_the_player() {
        // 0x4000 = 0.5, then -0.5: four bytes, two samples.
        let (base, seen) = serve_bytes(&[0x00, 0x40, 0x00, 0xc0], 200);
        let tts = ElevenLabsTts { base_url: base, key: "xi-test".into(), model: FLASH_MODEL.into(), voice_id: "v123".into() };
        assert_eq!(tts.sample_rate(), 24_000);
        let mut got = vec![];
        tts.speak("Hi.", &mut |c| {
            got.extend_from_slice(c);
            true
        })
        .unwrap();
        assert_eq!(got, vec![0.5, -0.5]);
        let (line, headers, body) = seen.lock().unwrap().clone();
        assert_eq!(line, "POST /v1/text-to-speech/v123/stream?output_format=pcm_24000 HTTP/1.1");
        assert!(headers.contains(&("xi-api-key".into(), "xi-test".into())));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, json!({ "text": "Hi.", "model_id": "eleven_flash_v2_5" }));
    }
}
