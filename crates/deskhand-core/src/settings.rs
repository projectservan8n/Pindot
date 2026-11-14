//! The app's settings file: `settings.json` in the data dir (0600).
//!
//! Versioned. Version 1 replaced Phase 2's `voice-settings.json`; on first
//! load that file is folded into `voice` and renamed
//! `voice-settings.migrated.json`. Unknown fields (from a newer Deskhand) are
//! kept when saving. Environment variables (`DESKHAND_PORT`,
//! `DESKHAND_STT_MODEL`, ...) override what is loaded but are never saved.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::platform::{HotkeyBinding, Os};
use crate::voice::VoiceSettings;
use crate::voice::models;

pub const SETTINGS_VERSION: u32 = 1;
pub const SETTINGS_FILE: &str = "settings.json";
pub const LEGACY_VOICE_FILE: &str = "voice-settings.json";
pub const MIGRATED_VOICE_FILE: &str = "voice-settings.migrated.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Hold-to-talk chord (`"Fn"`, `"Right Option"`, `"Ctrl+Cmd"`, `"Ctrl+Win"`).
    pub hotkey: HotkeyBinding,
    /// Loopback port of the MCP server; fixed so `claude mcp add` keeps
    /// working. A change applies after a restart.
    pub port: u16,
    /// Start the voice loop when everything it needs is ready.
    pub voice_enabled: bool,
    /// The user went through the setup screen once.
    pub onboarding_done: bool,
    pub voice: VoiceSettings,
    /// Fields this version doesn't know, kept as they were.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            hotkey: HotkeyBinding::platform_default(),
            port: crate::DEFAULT_PORT,
            voice_enabled: true,
            onboarding_done: false,
            voice: VoiceSettings::default(),
            extra: Default::default(),
        }
    }
}

/// What `load_in` had to do, for the log and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadNote {
    /// `settings.json` was read.
    Loaded,
    /// No file yet: defaults.
    Fresh,
    /// Phase 2's `voice-settings.json` was folded in and the result saved.
    MigratedVoiceSettings,
    /// The file could not be parsed; defaults are used and the bad file was
    /// kept as `settings.invalid.json`.
    Invalid(String),
    /// Written by a newer Deskhand: loaded what we know.
    NewerVersion(u32),
}

impl Settings {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(SETTINGS_FILE)
    }

    pub fn path() -> Result<PathBuf> {
        Ok(Self::path_in(&crate::paths::data_dir()?))
    }

    /// Load from the data dir, migrating old files, without env overrides.
    pub fn load() -> Result<(Settings, LoadNote)> {
        Self::load_in(&crate::paths::data_dir()?)
    }

    pub fn load_in(dir: &Path) -> Result<(Settings, LoadNote)> {
        let path = Self::path_in(dir);
        match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    let note = if s.version > SETTINGS_VERSION {
                        LoadNote::NewerVersion(s.version)
                    } else {
                        LoadNote::Loaded
                    };
                    if s.version < SETTINGS_VERSION {
                        s.version = SETTINGS_VERSION;
                    }
                    s.sanitize();
                    Ok((s, note))
                }
                Err(e) => {
                    let keep = dir.join("settings.invalid.json");
                    let _ = fs::rename(&path, &keep);
                    tracing::warn!("settings.json could not be read ({e}); using defaults, kept it as {}", keep.display());
                    Ok((Settings::default(), LoadNote::Invalid(e.to_string())))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let legacy = dir.join(LEGACY_VOICE_FILE);
                if let Ok(text) = fs::read_to_string(&legacy) {
                    let mut s = Settings::default();
                    match serde_json::from_str::<VoiceSettings>(&text) {
                        Ok(v) => s.voice = v,
                        Err(e) => tracing::warn!("{LEGACY_VOICE_FILE} ignored during migration: {e}"),
                    }
                    s.sanitize();
                    s.save_in(dir)?;
                    fs::rename(&legacy, dir.join(MIGRATED_VOICE_FILE))
                        .with_context(|| format!("renaming {}", legacy.display()))?;
                    return Ok((s, LoadNote::MigratedVoiceSettings));
                }
                Ok((Settings::default(), LoadNote::Fresh))
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_in(&crate::paths::data_dir()?)
    }

    pub fn save_in(&self, dir: &Path) -> Result<()> {
        crate::paths::ensure_private_dir(dir)?;
        let mut out = self.clone();
        if out.version < SETTINGS_VERSION {
            out.version = SETTINGS_VERSION;
        }
        let text = serde_json::to_string_pretty(&out)?;
        crate::paths::write_private(&Self::path_in(dir), text.as_bytes())
    }

    /// Clamp values that a hand-edited file could put out of range.
    fn sanitize(&mut self) {
        self.voice.volume = self.voice.volume.clamp(0.0, 1.0);
        self.voice.idle_reset_secs = self.voice.idle_reset_secs.max(30);
        if self.port < 1024 {
            self.port = crate::DEFAULT_PORT;
        }
        if self.hotkey.check_for(Os::current()).is_err() {
            self.hotkey = HotkeyBinding::platform_default();
        }
    }

    /// The settings with environment overrides applied (what the app runs
    /// with). Never saved.
    pub fn effective(&self) -> Settings {
        self.with_env(|k| std::env::var(k).ok())
    }

    pub fn with_env(&self, get: impl Fn(&str) -> Option<String>) -> Settings {
        let mut s = self.clone();
        if let Some(p) = get("DESKHAND_PORT").and_then(|v| v.parse::<u16>().ok()).filter(|p| *p >= 1024) {
            s.port = p;
        }
        if let Some(h) = get("DESKHAND_HOTKEY").and_then(|v| v.parse::<HotkeyBinding>().ok()) {
            s.hotkey = h;
        }
        s.voice.apply_env(get);
        s
    }

    /// Check a settings object coming from the UI before it is saved.
    pub fn validate(&self) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if let Err(e) = self.hotkey.check_for(Os::current()) {
            problems.push(format!("Shortcut: {e}"));
        }
        if self.port < 1024 {
            problems.push("Port: use a number from 1024 to 65535".into());
        }
        if !(0.0..=1.0).contains(&self.voice.volume) {
            problems.push("Volume: must be between 0 and 1".into());
        }
        if self.voice.idle_reset_secs < 30 {
            problems.push("New session after: at least 30 seconds".into());
        }
        for (what, id, kind) in [
            ("Speech to text", &self.voice.stt_model, models::ModelKind::SpeechToText),
            ("Text to speech", &self.voice.tts_model, models::ModelKind::TextToSpeech),
        ] {
            let ok = models::find(id).is_some_and(|m| m.kind == kind) || Path::new(id).is_absolute();
            if !ok {
                problems.push(format!("{what}: unknown model \"{id}\""));
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::SpeakMode;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deskhand-settings-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fresh_dir_gives_defaults() {
        let dir = temp_dir("fresh");
        let (s, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::Fresh);
        assert_eq!(s, Settings::default());
        assert_eq!(s.port, 47823);
        assert_eq!(s.voice.speak_mode, SpeakMode::EachMessage, "Phase 2 default kept");
        assert_eq!(s.hotkey.to_string(), if cfg!(windows) { "Ctrl+Cmd" } else { "Fn" });
        assert!(!s.onboarding_done);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_and_load_round_trip_with_user_only_file() {
        let dir = temp_dir("roundtrip");
        let mut s = Settings::default();
        s.hotkey = "Right Option".parse().unwrap();
        s.port = 48000;
        s.voice.speak_mode = SpeakMode::AtStop;
        s.voice.volume = 0.5;
        s.onboarding_done = true;
        s.save_in(&dir).unwrap();
        let (back, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(back, s);
        let text = fs::read_to_string(Settings::path_in(&dir)).unwrap();
        assert!(text.contains("\"hotkey\": \"Right Option\""), "{text}");
        assert!(text.contains("\"speak_mode\": \"at_stop\""), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(Settings::path_in(&dir)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn migrates_phase_2_voice_settings() {
        let dir = temp_dir("migrate");
        fs::write(
            dir.join(LEGACY_VOICE_FILE),
            r#"{"speak_mode":"at_stop","volume":0.3,"model":"haiku","idle_reset_secs":120}"#,
        )
        .unwrap();
        let (s, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::MigratedVoiceSettings);
        assert_eq!(s.voice.speak_mode, SpeakMode::AtStop);
        assert_eq!(s.voice.volume, 0.3);
        assert_eq!(s.voice.model.as_deref(), Some("haiku"));
        assert_eq!(s.voice.idle_reset_secs, 120);
        assert_eq!(s.voice.stt_model, models::DEFAULT_STT_MODEL, "missing fields default");
        assert!(!dir.join(LEGACY_VOICE_FILE).exists(), "old file renamed");
        assert!(dir.join(MIGRATED_VOICE_FILE).exists());
        // The second load reads the new file and does not migrate again.
        let (again, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(again, s);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn versionless_and_partial_files_get_defaults_and_unknown_fields_survive() {
        let dir = temp_dir("partial");
        fs::write(Settings::path_in(&dir), r#"{"hotkey":"ctrl+cmd","future_thing":{"a":1},"voice":{"volume":7}}"#)
            .unwrap();
        let (s, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.hotkey.to_string(), "Ctrl+Cmd");
        assert_eq!(s.voice.volume, 1.0, "clamped");
        assert_eq!(s.port, 47823);
        s.save_in(&dir).unwrap();
        let text = fs::read_to_string(Settings::path_in(&dir)).unwrap();
        assert!(text.contains("future_thing"), "{text}");
        assert!(text.contains("\"version\": 1"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn newer_and_invalid_files() {
        let dir = temp_dir("newer");
        fs::write(Settings::path_in(&dir), r#"{"version":9,"port":50000}"#).unwrap();
        let (s, note) = Settings::load_in(&dir).unwrap();
        assert_eq!(note, LoadNote::NewerVersion(9));
        assert_eq!(s.port, 50000);
        fs::write(Settings::path_in(&dir), r#"{"hotkey":"Q"}"#).unwrap();
        let (s, note) = Settings::load_in(&dir).unwrap();
        assert!(matches!(note, LoadNote::Invalid(_)));
        assert_eq!(s, Settings::default());
        assert!(dir.join("settings.invalid.json").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn env_overrides_apply_but_are_not_saved() {
        let s = Settings::default();
        let env = |k: &str| match k {
            "DESKHAND_PORT" => Some("48111".to_string()),
            "DESKHAND_HOTKEY" => Some("Right Cmd".to_string()),
            "DESKHAND_SPEAK_MODE" => Some("at_stop".to_string()),
            _ => None,
        };
        let e = s.with_env(env);
        assert_eq!(e.port, 48111);
        assert_eq!(e.hotkey.to_string(), "Right Cmd");
        assert_eq!(e.voice.speak_mode, SpeakMode::AtStop);
        assert_eq!(s.port, 47823, "the stored settings are untouched");
    }

    #[test]
    fn validate_reports_each_problem() {
        let mut s = Settings::default();
        assert_eq!(s.validate(), Ok(()));
        s.port = 80;
        s.voice.volume = 2.0;
        s.voice.idle_reset_secs = 5;
        s.voice.stt_model = "phonon-31m-en".into(); // a TTS model in the STT slot
        let problems = s.validate().unwrap_err();
        assert_eq!(problems.len(), 4, "{problems:?}");
    }
}
