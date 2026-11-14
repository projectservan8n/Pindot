//! The speech models: a small pinned catalog and a minimal downloader.
//!
//! Weights are never in git. They download at first use into the per-user
//! data dir (`<data dir>/models/<model id>/`, or `DESKHAND_MODELS_DIR`), from
//! a URL pinned to one repository revision, and every file is checked
//! against its size and SHA-256 before it is used, on every load.
//!
//! Which model the voice loop uses is a setting (`VoiceSettings`), not a
//! constant: a catalog id from here, or the path of a local folder laid out
//! the way the runtime expects (used as is, no download, no checksum).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    SpeechToText,
    TextToSpeech,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub kind: ModelKind,
    pub display: &'static str,
    pub license: &'static str,
    /// Where it comes from, for the NOTICE and the settings screen.
    pub source: &'static str,
    /// URL prefix pinned to one revision; a file's URL is `base_url + name`.
    pub base_url: &'static str,
    pub files: &'static [ModelFile],
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Moonshine v2 Small, streaming, English: the official ONNX Runtime export
/// from Moonshine AI's asset repo, the `quantized_26_07_30` build (the last
/// one that ships `frontend.ort` as one file, which transcribe-rs 0.3.12
/// needs). MIT.
pub const MOONSHINE_SMALL_STREAMING_EN: ModelSpec = ModelSpec {
    id: "moonshine-small-streaming-en",
    kind: ModelKind::SpeechToText,
    display: "Moonshine v2 Small streaming (English)",
    license: "MIT",
    source: "huggingface.co/moonshine-ai/moonshine-voice-assets@e318117 model/small-streaming-en/quantized_26_07_30",
    base_url: "https://huggingface.co/moonshine-ai/moonshine-voice-assets/resolve/e318117203f245fda2ca0142db6b8c057d86671c/model/small-streaming-en/quantized_26_07_30/",
    files: &[
        ModelFile { name: "frontend.ort", size: 30_984_520, sha256: "d1be8145bc9ef3e8625bb7bcb7a5b2930eeb828c91d6c1897bc215d7de6f2b93" },
        ModelFile { name: "encoder.ort", size: 44_148_576, sha256: "2d4d973e91e8aca08c51e7e7efa28a46ab265b63d809d5294d18b86bcd85b993" },
        ModelFile { name: "adapter.ort", size: 2_870_368, sha256: "c665f742364febad597cc9ac1e0b341ffbee0e24a1466e2f3bde95e6e4771762" },
        ModelFile { name: "cross_kv.ort", size: 5_356_536, sha256: "e2d3417144e9514055ebfefe8dcc4c0a55a55adcb8530435844c75c53e352bf6" },
        ModelFile { name: "decoder_kv.ort", size: 81_878_600, sha256: "1a05465b1dd955858dfcbee039c0020fb5dd982b0f5094c34e61735d518d771b" },
        ModelFile { name: "tokenizer.bin", size: 249_974, sha256: "6884b35fd6377d4c4d32336a0bc152f36b64d1e45b6503683cdc238250a8472d" },
        ModelFile { name: "streaming_config.json", size: 512, sha256: "26f02b6afb22d60871a5efd85c3d38e569cc0ddb6c5eb6e93d3260152ae8a47a" },
    ],
};

/// Phonon 31m, English, 8-bit quantized GGUF with its default voice, from
/// Gradium. Apache-2.0 (weights, tokenizer and voices).
pub const PHONON_31M_EN: ModelSpec = ModelSpec {
    id: "phonon-31m-en",
    kind: ModelKind::TextToSpeech,
    display: "Phonon 31m (English)",
    license: "Apache-2.0",
    source: "huggingface.co/gradium/phonon-31m-en@92741c5",
    base_url: "https://huggingface.co/gradium/phonon-31m-en/resolve/92741c5b07d085ff840e26fea8ce43110b87852f/",
    files: &[
        ModelFile { name: "config.json", size: 4_634, sha256: "b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85" },
        ModelFile { name: "model.q8.gguf", size: 64_512_352, sha256: "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae" },
        ModelFile { name: "tokenizer.json", size: 277_967, sha256: "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936" },
        ModelFile { name: "default-voice.safetensors", size: 256_136, sha256: "dc537478eaadc1a5257fcda2e3501ab81e14c111a5d95a5838f350db2afd0f1a" },
    ],
};

pub const CATALOG: &[&ModelSpec] = &[&MOONSHINE_SMALL_STREAMING_EN, &PHONON_31M_EN];

pub const DEFAULT_STT_MODEL: &str = MOONSHINE_SMALL_STREAMING_EN.id;
pub const DEFAULT_TTS_MODEL: &str = PHONON_31M_EN.id;

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().copied().find(|s| s.id == id)
}

/// `DESKHAND_MODELS_DIR`, else `<data dir>/models`.
pub fn models_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("DESKHAND_MODELS_DIR") {
        Some(d) => PathBuf::from(d),
        None => crate::paths::data_dir()?.join("models"),
    };
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

#[derive(Debug, Clone)]
pub enum Progress {
    Checking { file: &'static str },
    Downloading { file: &'static str, done: u64, total: u64 },
    Verified { file: &'static str },
}

/// A model setting: a catalog id, or a local folder used as is.
pub fn resolve(setting: &str, kind: ModelKind, root: &Path, progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    let as_path = Path::new(setting);
    if as_path.is_absolute() && as_path.is_dir() {
        tracing::info!("using the local model folder {} (no download, no checksum)", as_path.display());
        return Ok(as_path.to_path_buf());
    }
    let spec = find(setting).with_context(|| {
        let ids: Vec<_> = CATALOG.iter().filter(|s| s.kind == kind).map(|s| s.id).collect();
        format!("unknown model {setting:?}: use one of {ids:?} or the absolute path of a model folder")
    })?;
    if spec.kind != kind {
        bail!("{} is a {:?} model, not {:?}", spec.id, spec.kind, kind);
    }
    ensure(spec, root, progress)
}

/// Make sure every file of `spec` is present and intact under `root/<id>`,
/// downloading what is missing or wrong. Returns the model folder.
pub fn ensure(spec: &ModelSpec, root: &Path, progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    let dir = root.join(spec.id);
    fs::create_dir_all(&dir)?;
    for f in spec.files {
        let path = dir.join(f.name);
        progress(Progress::Checking { file: f.name });
        if path.exists() && file_ok(&path, f)? {
            progress(Progress::Verified { file: f.name });
            continue;
        }
        download(&format!("{}{}", spec.base_url, f.name), &path, f, progress)
            .with_context(|| format!("downloading {} of {}", f.name, spec.id))?;
        progress(Progress::Verified { file: f.name });
    }
    Ok(dir)
}

/// True when every file is present with the right size (no hashing): a
/// cheap "is it downloaded" check for the UI.
pub fn is_present(spec: &ModelSpec, root: &Path) -> bool {
    let dir = root.join(spec.id);
    spec.files.iter().all(|f| fs::metadata(dir.join(f.name)).map(|m| m.len() == f.size).unwrap_or(false))
}

fn file_ok(path: &Path, f: &ModelFile) -> Result<bool> {
    if fs::metadata(path)?.len() != f.size {
        return Ok(false);
    }
    Ok(sha256_file(path)? == f.sha256)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn download(url: &str, dest: &Path, f: &ModelFile, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    tracing::info!("downloading {url} ({} bytes)", f.size);
    let part = dest.with_extension("part");
    let mut resp = ureq::get(url)
        .header("User-Agent", concat!("Deskhand/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut reader = resp.body_mut().as_reader();
    let mut out = fs::File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > f.size {
            let _ = fs::remove_file(&part);
            bail!("{url} is larger than the pinned {} bytes", f.size);
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        if done - last_report >= 8 << 20 || done == f.size {
            progress(Progress::Downloading { file: f.name, done, total: f.size });
            last_report = done;
        }
    }
    out.sync_all()?;
    drop(out);
    let got = hex(&hasher.finalize());
    if done != f.size || got != f.sha256 {
        let _ = fs::remove_file(&part);
        bail!("{url}: checksum mismatch (got {done} bytes, sha256 {got}; expected {} bytes, {})", f.size, f.sha256);
    }
    fs::rename(&part, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_pinned_and_checksummed() {
        for spec in CATALOG {
            assert!(spec.base_url.starts_with("https://huggingface.co/"), "{}", spec.id);
            // Pinned to a commit, never a moving branch.
            assert!(!spec.base_url.contains("/resolve/main/"), "{}", spec.id);
            for f in spec.files {
                assert_eq!(f.sha256.len(), 64, "{} {}", spec.id, f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(f.size > 0);
            }
        }
        assert_eq!(find(DEFAULT_STT_MODEL).unwrap().kind, ModelKind::SpeechToText);
        assert_eq!(find(DEFAULT_TTS_MODEL).unwrap().kind, ModelKind::TextToSpeech);
        // About 230 MB in total.
        let total: u64 = CATALOG.iter().map(|s| s.total_size()).sum();
        assert!(total < 300_000_000);
    }

    #[test]
    fn a_setting_can_be_a_local_folder_or_a_catalog_id() {
        let tmp = std::env::temp_dir().join(format!("dh-models-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let p = resolve(tmp.to_str().unwrap(), ModelKind::TextToSpeech, &tmp, &mut |_| {}).unwrap();
        assert_eq!(p, tmp);
        assert!(resolve("no-such-model", ModelKind::TextToSpeech, &tmp, &mut |_| {}).is_err());
        assert!(resolve(DEFAULT_STT_MODEL, ModelKind::TextToSpeech, &tmp, &mut |_| {}).is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
