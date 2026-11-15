//! The speech models: a small pinned catalog and the first-run downloader.
//!
//! Weights are never in git. They download into the per-user data dir
//! (`<data dir>/models/<model id>/`, or `DESKHAND_MODELS_DIR`) only when the
//! user presses Download on the setup screen, after the model's license is
//! shown. Every file is checked against its pinned size and SHA-256 before it
//! is used, on every load.
//!
//! Downloading: each file goes to `<name>.part` and is resumed with an HTTP
//! Range request after a dropped connection (a server that ignores Range
//! sends the whole file and we start over). A file whose checksum fails is
//! deleted and fetched again from scratch. After the retries, a model with a
//! fallback (Moonshine: Handy's mirror, `blob.handy.computer`, which serves an
//! older build of the same model as one tar.gz with its own pinned checksums)
//! is fetched from there instead.
//!
//! Which model the voice loop uses is a setting, not a constant: a catalog
//! id from here, or the path of a local folder laid out the way the runtime
//! expects (used as is, no download, no checksum).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ModelKind {
    SpeechToText,
    TextToSpeech,
}

#[derive(Debug, Clone, Copy, Serialize)]
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
    pub license_url: &'static str,
    /// The full license text, shown before the user downloads.
    pub license_text: &'static str,
    /// Where it comes from, for the NOTICE and the settings screen.
    pub source: &'static str,
    /// URL prefix pinned to one revision; a file's URL is `base_url + name`.
    pub base_url: &'static str,
    pub files: &'static [ModelFile],
    /// Used when `base_url` keeps failing.
    pub fallback: Option<&'static ArchiveSource>,
}

/// A fallback source: one `.tar.gz` holding the model files (in one top
/// folder), pinned by the archive's own size and SHA-256 and by each
/// extracted file's.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveSource {
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub note: &'static str,
    pub files: &'static [ModelFile],
}

pub const MIT_MOONSHINE: &str = include_str!("../../licenses/moonshine-MIT.txt");
pub const APACHE_2_0: &str = include_str!("../../licenses/apache-2.0.txt");
pub const CC_BY_4_0_PARAKEET: &str = concat!(
    "Parakeet TDT 0.6B v3 by NVIDIA, licensed under CC BY 4.0.\n",
    "ONNX export by istupakov (huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx).\n",
    "Deskhand downloads the int8 files unmodified.\n\n",
    include_str!("../../licenses/cc-by-4.0.txt")
);

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
    license_url: "https://github.com/moonshine-ai/moonshine/blob/main/LICENSE",
    license_text: MIT_MOONSHINE,
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
    fallback: Some(&HANDY_MOONSHINE_SMALL_STREAMING_EN),
};

/// Handy's mirror of Moonshine v2 Small streaming EN (Feb 2026 build: the
/// same file names, different bytes). Checked with transcribe-rs 0.3.12 on
/// the test WAV (same transcript as the pinned build), 14 Nov 2026.
pub const HANDY_MOONSHINE_SMALL_STREAMING_EN: ArchiveSource = ArchiveSource {
    url: "https://blob.handy.computer/moonshine-small-streaming-en.tar.gz",
    size: 104_842_676,
    sha256: "dbb3e1c1832bd88a4ac712f7449a136cc2c9a18c5fe33a12ed1b7cb1cfe9cdd5",
    note: "Handy's mirror (blob.handy.computer), an older build of the same model",
    files: &[
        ModelFile { name: "frontend.ort", size: 30_984_200, sha256: "e086451043c1c8652a9614e4a4a81d5807221b611584a3cf31f73779d5900003" },
        ModelFile { name: "encoder.ort", size: 43_853_224, sha256: "3b21d02eff6aa5651524ada4271d37c1d7bba4eb3d256415074f2cfdbaeb526a" },
        ModelFile { name: "adapter.ort", size: 2_867_424, sha256: "d8493e0ac76a198b309a8be6f74b3101e235f773ffe5d6b378278cd7e4177992" },
        ModelFile { name: "cross_kv.ort", size: 5_298_736, sha256: "6e57d1361717e00d73336a0c3beafedae784b1e537905ad253dee33db4007466" },
        ModelFile { name: "decoder_kv.ort", size: 81_435_904, sha256: "d5adfcfaa6e582144791f1568bd0f683852c7bfbb8c79acad97499da05e4ffcf" },
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
    license_url: "https://www.apache.org/licenses/LICENSE-2.0",
    license_text: APACHE_2_0,
    source: "huggingface.co/gradium/phonon-31m-en@92741c5",
    base_url: "https://huggingface.co/gradium/phonon-31m-en/resolve/92741c5b07d085ff840e26fea8ce43110b87852f/",
    files: &[
        ModelFile { name: "config.json", size: 4_634, sha256: "b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85" },
        ModelFile { name: "model.q8.gguf", size: 64_512_352, sha256: "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae" },
        ModelFile { name: "tokenizer.json", size: 277_967, sha256: "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936" },
        ModelFile { name: "default-voice.safetensors", size: 256_136, sha256: "dc537478eaadc1a5257fcda2e3501ab81e14c111a5d95a5838f350db2afd0f1a" },
    ],
    // No second source is known for Phonon 31m (Nov 2026).
    fallback: None,
};

/// Optional, more accurate speech to text (about 4x Moonshine's size).
/// transcribe-rs loads it from these four files (`Quantization::Int8`).
pub const PARAKEET_TDT_06B_V3: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v3-int8",
    kind: ModelKind::SpeechToText,
    display: "Parakeet TDT 0.6B v3 (more accurate, larger)",
    license: "CC-BY-4.0",
    license_url: "https://creativecommons.org/licenses/by/4.0/",
    license_text: CC_BY_4_0_PARAKEET,
    source: "huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx@8f23f0c",
    base_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/",
    files: &[
        ModelFile { name: "vocab.txt", size: 93_939, sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d" },
        ModelFile { name: "nemo128.onnx", size: 139_764, sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f" },
        ModelFile { name: "decoder_joint-model.int8.onnx", size: 18_202_004, sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70" },
        ModelFile { name: "encoder-model.int8.onnx", size: 652_183_999, sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09" },
    ],
    fallback: None,
};

pub const CATALOG: &[&ModelSpec] = &[&MOONSHINE_SMALL_STREAMING_EN, &PARAKEET_TDT_06B_V3, &PHONON_31M_EN];

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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Progress {
    Checking { file: &'static str },
    /// `done`/`total` are for this file; `model_done`/`model_total` for the
    /// whole model.
    Downloading { file: &'static str, done: u64, total: u64, model_done: u64, model_total: u64 },
    /// A download resumed from `from` bytes (HTTP 206).
    Resumed { file: &'static str, from: u64 },
    Verified { file: &'static str },
    /// A checksum failed: the file was deleted and is fetched again.
    ChecksumFailed { file: &'static str },
    Retrying { file: &'static str, attempt: u32, error: String },
    /// Switching to the fallback source.
    Fallback { note: &'static str },
}

/// How hard the downloader tries.
#[derive(Clone)]
pub struct DownloadOptions {
    /// Extra attempts per file after the first.
    pub retries: u32,
    /// Wait before the first retry; doubles each time.
    pub backoff: Duration,
    pub use_fallback: bool,
    /// Set to stop a download; the `.part` file stays for a later resume.
    pub cancel: Arc<AtomicBool>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self { retries: 3, backoff: Duration::from_secs(2), use_fallback: true, cancel: Arc::new(AtomicBool::new(false)) }
    }
}

/// A model setting: a catalog id, or a local folder used as is. Never
/// downloads: a missing or damaged model is an error that points to the
/// setup screen.
pub fn resolve(setting: &str, kind: ModelKind, root: &Path) -> Result<PathBuf> {
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
    verify(spec, root)
}

/// The file set present in `root/<id>`: the pinned files, the fallback's,
/// or none (sizes only, no hashing).
pub fn present_files(spec: &ModelSpec, root: &Path) -> Option<&'static [ModelFile]> {
    let dir = root.join(spec.id);
    let all_there = |files: &[ModelFile]| {
        files.iter().all(|f| fs::metadata(dir.join(f.name)).map(|m| m.len() == f.size).unwrap_or(false))
    };
    if all_there(spec.files) {
        return Some(spec.files);
    }
    spec.fallback.filter(|fb| all_there(fb.files)).map(|fb| fb.files)
}

/// True when every file is present with the right size (no hashing): a
/// cheap "is it downloaded" check for the UI.
pub fn is_present(spec: &ModelSpec, root: &Path) -> bool {
    present_files(spec, root).is_some()
}

/// Check every file of an installed model (size and SHA-256). Never
/// downloads.
pub fn verify(spec: &ModelSpec, root: &Path) -> Result<PathBuf> {
    let dir = root.join(spec.id);
    let files = present_files(spec, root).ok_or_else(|| {
        anyhow!("{} is not downloaded yet: open Deskhand's Setup screen and press Download", spec.display)
    })?;
    for f in files {
        if !file_ok(&dir.join(f.name), f)? {
            bail!("{} in {} failed its checksum: download it again on the Setup screen", f.name, dir.display());
        }
    }
    Ok(dir)
}

/// Make sure every file of `spec` is present and intact under `root/<id>`,
/// downloading what is missing or damaged. Returns the model folder.
pub fn ensure(spec: &ModelSpec, root: &Path, progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    ensure_with(spec, root, &DownloadOptions::default(), progress)
}

pub fn ensure_with(
    spec: &ModelSpec,
    root: &Path,
    opts: &DownloadOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    let dir = root.join(spec.id);
    fs::create_dir_all(&dir)?;
    // A complete fallback install is fine as it is.
    if let Some(fb) = spec.fallback
        && present_files(spec, root).is_some_and(|f| std::ptr::eq(f, fb.files))
        && fb.files.iter().all(|f| file_ok(&dir.join(f.name), f).unwrap_or(false))
    {
        return Ok(dir);
    }
    let agent = http_agent();
    let model_total = spec.total_size();
    let mut model_done = 0u64;
    let mut failure = None;
    for f in spec.files {
        let path = dir.join(f.name);
        progress(Progress::Checking { file: f.name });
        if path.exists() {
            if file_ok(&path, f)? {
                model_done += f.size;
                progress(Progress::Verified { file: f.name });
                continue;
            }
            // Present but damaged (or another build's file): start clean.
            tracing::warn!("{} failed its checksum; downloading it again", path.display());
            progress(Progress::ChecksumFailed { file: f.name });
            fs::remove_file(&path)?;
        }
        let url = format!("{}{}", spec.base_url, f.name);
        match fetch_with_retries(&agent, &url, &path, f, opts, model_done, model_total, progress) {
            Ok(()) => {
                model_done += f.size;
                progress(Progress::Verified { file: f.name });
            }
            Err(e) => {
                failure = Some(e.context(format!("downloading {} of {}", f.name, spec.id)));
                break;
            }
        }
    }
    let Some(err) = failure else { return Ok(dir) };
    if opts.cancel.load(Ordering::Acquire) {
        return Err(err.context("download cancelled"));
    }
    match spec.fallback.filter(|_| opts.use_fallback) {
        Some(fb) => {
            tracing::warn!("{err:#}; trying {}", fb.note);
            progress(Progress::Fallback { note: fb.note });
            fetch_archive(&agent, spec, fb, root, opts, progress)
                .with_context(|| format!("the pinned source failed ({err:#}), and so did the fallback"))?;
            Ok(dir)
        }
        None => Err(err),
    }
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            // A stalled body read fails instead of hanging forever.
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build(),
    )
}

#[allow(clippy::too_many_arguments)]
fn fetch_with_retries(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    f: &ModelFile,
    opts: &DownloadOptions,
    model_done: u64,
    model_total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let part = part_path(dest);
    let mut wait = opts.backoff;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = fetch_resumable(agent, url, &part, f, opts, (model_done, model_total), progress).and_then(|()| {
            if sha256_file(&part)? == f.sha256 {
                Ok(())
            } else {
                // Clean retry: never resume onto bad bytes.
                let _ = fs::remove_file(&part);
                progress(Progress::ChecksumFailed { file: f.name });
                Err(anyhow!("{url}: SHA-256 does not match the pinned {}", f.sha256))
            }
        });
        match result {
            Ok(()) => {
                fs::rename(&part, dest)?;
                return Ok(());
            }
            Err(e) if attempt > opts.retries || opts.cancel.load(Ordering::Acquire) => return Err(e),
            Err(e) => {
                tracing::warn!("{} attempt {attempt} failed: {e:#}; retrying in {wait:?}", f.name);
                progress(Progress::Retrying { file: f.name, attempt, error: format!("{e:#}") });
                sleep_unless_cancelled(wait, &opts.cancel);
                wait = wait.saturating_mul(2);
            }
        }
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn sleep_unless_cancelled(d: Duration, cancel: &AtomicBool) {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end && !cancel.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Download `url` into `part`, resuming from what `part` already holds.
/// Leaves a partial `part` behind on a network error (for the next resume).
fn fetch_resumable(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    f: &ModelFile,
    opts: &DownloadOptions,
    (model_done, model_total): (u64, u64),
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let (name, size) = (f.name, f.size);
    let mut have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        fs::remove_file(part)?;
        have = 0;
    }
    if have == size {
        return Ok(());
    }
    let mut req = agent.get(url).header("User-Agent", concat!("Deskhand/", env!("CARGO_PKG_VERSION")));
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut resp = req.call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let mut out = match status {
        206 if have > 0 => {
            let range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).unwrap_or("");
            if !range.starts_with(&format!("bytes {have}-")) {
                fs::remove_file(part)?;
                bail!("{url}: the server resumed at the wrong place ({range:?}); starting over");
            }
            tracing::info!("resuming {name} from {have} bytes");
            progress(Progress::Resumed { file: name, from: have });
            fs::OpenOptions::new().append(true).open(part)?
        }
        200 => {
            have = 0;
            fs::File::create(part)?
        }
        416 => {
            // Range not satisfiable: what we have is useless, start over.
            fs::remove_file(part)?;
            bail!("{url}: HTTP 416 for a resume; starting over");
        }
        other => bail!("{url}: HTTP {other}"),
    };
    tracing::info!("downloading {url} ({size} bytes)");
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = have;
    let mut last_report = 0u64;
    loop {
        if opts.cancel.load(Ordering::Acquire) {
            out.sync_all()?;
            bail!("cancelled");
        }
        let n = reader.read(&mut buf).with_context(|| format!("reading {url}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > size {
            drop(out);
            fs::remove_file(part)?;
            bail!("{url} is larger than the pinned {size} bytes");
        }
        out.write_all(&buf[..n])?;
        if done - last_report >= 1 << 20 || done == size {
            progress(Progress::Downloading { file: name, done, total: size, model_done: model_done + done, model_total });
            last_report = done;
        }
    }
    out.sync_all()?;
    if done != size {
        bail!("{url}: the connection ended at {done} of {size} bytes");
    }
    Ok(())
}

/// Fetch the fallback archive, check it, and unpack the expected files
/// (flat, by name only: no paths from the archive are used).
fn fetch_archive(
    agent: &ureq::Agent,
    spec: &ModelSpec,
    fb: &ArchiveSource,
    root: &Path,
    opts: &DownloadOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let name: &'static str = "archive (fallback)";
    let archive = root.join(format!("{}.fallback.tar.gz", spec.id));
    let archive_spec = ModelFile { name, size: fb.size, sha256: fb.sha256 };
    fetch_with_retries(agent, fb.url, &archive, &archive_spec, opts, 0, fb.size, progress)?;
    let dir = root.join(spec.id);
    let result = (|| -> Result<()> {
        let gz = flate2::read::GzDecoder::new(fs::File::open(&archive)?);
        let mut tar = tar::Archive::new(gz);
        let mut got = Vec::new();
        for entry in tar.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path()?.into_owned();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(f) = fb.files.iter().find(|f| f.name == file_name) else { continue };
            let part = part_path(&dir.join(f.name));
            let mut out = fs::File::create(&part)?;
            let n = std::io::copy(&mut (&mut entry).take(f.size + 1), &mut out)?;
            out.sync_all()?;
            if n != f.size || sha256_file(&part)? != f.sha256 {
                let _ = fs::remove_file(&part);
                bail!("{} inside the fallback archive failed its checksum", f.name);
            }
            got.push(f);
        }
        if got.len() != fb.files.len() {
            bail!("the fallback archive is missing files ({} of {})", got.len(), fb.files.len());
        }
        // Swap the whole set in only once every file checked out.
        for f in got {
            let dest = dir.join(f.name);
            fs::rename(part_path(&dest), &dest)?;
            progress(Progress::Verified { file: f.name });
        }
        Ok(())
    })();
    let _ = fs::remove_file(&archive);
    result
}

fn file_ok(path: &Path, f: &ModelFile) -> Result<bool> {
    match fs::metadata(path) {
        Ok(m) if m.len() == f.size => Ok(sha256_file(path)? == f.sha256),
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
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
        // The defaults are about 230 MB; Parakeet (optional) about 670 MB.
        let total = find(DEFAULT_STT_MODEL).unwrap().total_size() + find(DEFAULT_TTS_MODEL).unwrap().total_size();
        assert!(total < 300_000_000);
        assert!(PARAKEET_TDT_06B_V3.total_size() < 700_000_000);
    }

    #[test]
    fn a_setting_can_be_a_local_folder_or_a_catalog_id() {
        let tmp = std::env::temp_dir().join(format!("dh-models-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let p = resolve(tmp.to_str().unwrap(), ModelKind::TextToSpeech, &tmp).unwrap();
        assert_eq!(p, tmp);
        assert!(resolve("no-such-model", ModelKind::TextToSpeech, &tmp).is_err());
        assert!(resolve(DEFAULT_STT_MODEL, ModelKind::TextToSpeech, &tmp).is_err());
        // A catalog model that is not downloaded is an error, never a download.
        let e = resolve(DEFAULT_TTS_MODEL, ModelKind::TextToSpeech, &tmp).unwrap_err();
        assert!(format!("{e}").contains("not downloaded"), "{e}");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ---------------------------------------------------- downloader tests
    //
    // A tiny HTTP/1.1 server on 127.0.0.1 that can drop the connection
    // mid-file, ignore Range, or serve wrong bytes.

    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::Mutex;

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        /// Close the connection after this many body bytes.
        cut_after: Option<usize>,
    }

    impl Reply {
        fn ok(body: &[u8]) -> Self {
            Self { status: 200, headers: vec![], body: body.to_vec(), cut_after: None }
        }
        fn not_found() -> Self {
            Self { status: 404, headers: vec![], body: b"nope".to_vec(), cut_after: None }
        }
    }

    /// (path, Range header) of every request.
    type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

    fn serve(handler: impl Fn(&str, Option<&str>, usize) -> Reply + Send + 'static) -> (String, Log) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log: Log = Arc::default();
        let log2 = log.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut reader = std::io::BufReader::new(conn.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':')
                        && k.eq_ignore_ascii_case("range")
                    {
                        range = Some(v.trim().to_string());
                    }
                }
                let n = {
                    let mut l = log2.lock().unwrap();
                    l.push((path.clone(), range.clone()));
                    l.iter().filter(|(p, _)| *p == path).count()
                };
                let r = handler(&path, range.as_deref(), n);
                let mut head = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
                for (k, v) in &r.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let body = &r.body[..r.cut_after.unwrap_or(r.body.len()).min(r.body.len())];
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(body);
                let _ = conn.flush();
            }
        });
        (base, log)
    }

    /// Serves `bytes` with Range support.
    fn ranged(bytes: &[u8], range: Option<&str>) -> Reply {
        match range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.strip_suffix('-')).and_then(|n| n.parse::<usize>().ok()) {
            Some(from) if from < bytes.len() => Reply {
                status: 206,
                headers: vec![("Content-Range".into(), format!("bytes {from}-{}/{}", bytes.len() - 1, bytes.len()))],
                body: bytes[from..].to_vec(),
                cut_after: None,
            },
            _ => Reply::ok(bytes),
        }
    }

    fn leak<T>(v: T) -> &'static T {
        Box::leak(Box::new(v))
    }

    fn sha(bytes: &[u8]) -> &'static str {
        Box::leak(hex(&Sha256::digest(bytes)).into_boxed_str())
    }

    fn content(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    fn test_spec(base: &str, files: &[(&'static str, &[u8])], fallback: Option<&'static ArchiveSource>) -> &'static ModelSpec {
        let files: Vec<ModelFile> = files.iter().map(|(n, b)| ModelFile { name: n, size: b.len() as u64, sha256: sha(b) }).collect();
        leak(ModelSpec {
            id: "test-model",
            kind: ModelKind::TextToSpeech,
            display: "Test model",
            license: "MIT",
            license_url: "",
            license_text: "",
            source: "test",
            base_url: Box::leak(format!("{base}/m/").into_boxed_str()),
            files: Box::leak(files.into_boxed_slice()),
            fallback,
        })
    }

    fn fast() -> DownloadOptions {
        DownloadOptions { retries: 2, backoff: Duration::from_millis(10), use_fallback: true, cancel: Arc::default() }
    }

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dh-dl-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".part")).collect())
            .unwrap_or_default()
    }

    #[test]
    fn downloads_verifies_and_reports_progress() {
        let a = content(1, 300_000);
        let b = content(2, 1_000);
        let (a2, b2) = (a.clone(), b.clone());
        let (base, _) = serve(move |p, _, _| match p {
            "/m/a.bin" => Reply::ok(&a2),
            "/m/b.bin" => Reply::ok(&b2),
            _ => Reply::not_found(),
        });
        let spec = test_spec(&base, &[("a.bin", &a), ("b.bin", &b)], None);
        let root = tmp_root("fresh");
        assert!(!is_present(spec, &root));
        let mut events = vec![];
        let dir = ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        assert!(is_present(spec, &root));
        verify(spec, &root).unwrap();
        let last_total = events.iter().rev().find_map(|e| match e {
            Progress::Downloading { model_done, model_total, .. } => Some((*model_done, *model_total)),
            _ => None,
        });
        assert_eq!(last_total, Some((301_000, 301_000)));
        assert_eq!(events.iter().filter(|e| matches!(e, Progress::Verified { .. })).count(), 2);
        assert!(leftovers(&dir).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dropped_connection_resumes_with_range() {
        let a = content(3, 2_000_000);
        let a2 = a.clone();
        let (base, log) = serve(move |_, range, n| {
            if n == 1 {
                // First try: the connection drops after 700 kB.
                Reply { cut_after: Some(700_000), ..Reply::ok(&a2) }
            } else {
                ranged(&a2, range)
            }
        });
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("resume");
        let mut events = vec![];
        let dir = ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        let log = log.lock().unwrap().clone();
        assert_eq!(log.len(), 2, "{log:?}");
        let from: u64 = log[1].1.as_deref().unwrap().trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
        assert!(from > 0 && from <= 700_000, "resumed from {from}");
        assert!(events.iter().any(|e| matches!(e, Progress::Resumed { from: f, .. } if *f == from)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_server_that_ignores_range_restarts_cleanly() {
        let a = content(4, 500_000);
        let a2 = a.clone();
        let (base, log) = serve(move |_, _, n| if n == 1 { Reply { cut_after: Some(100_000), ..Reply::ok(&a2) } } else { Reply::ok(&a2) });
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("norange");
        let dir = ensure_with(spec, &root, &fast(), &mut |_| {}).unwrap();
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        assert!(log.lock().unwrap()[1].1.is_some(), "asked to resume");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn wrong_bytes_fail_the_checksum_and_are_fetched_again() {
        let a = content(5, 50_000);
        let mut bad = a.clone();
        bad[25_000] ^= 0xFF;
        let (a2, bad2) = (a.clone(), bad.clone());
        let (base, _) = serve(move |_, _, n| if n == 1 { Reply::ok(&bad2) } else { Reply::ok(&a2) });
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("badsum");
        let mut events = vec![];
        let dir = ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        assert!(events.iter().any(|e| matches!(e, Progress::ChecksumFailed { .. })));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn always_wrong_bytes_end_in_an_error_and_leave_nothing() {
        let a = content(6, 10_000);
        let mut bad = a.clone();
        bad[0] ^= 1;
        let (base, log) = serve(move |_, _, _| Reply::ok(&bad));
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("allbad");
        let err = ensure_with(spec, &root, &fast(), &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("SHA-256"), "{err:#}");
        assert_eq!(log.lock().unwrap().len(), 3, "first try plus 2 retries");
        let dir = root.join(spec.id);
        assert!(!dir.join("a.bin").exists());
        assert!(leftovers(&dir).is_empty(), "a bad .part is never kept");
        assert!(verify(spec, &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_installed_file_is_replaced() {
        let a = content(7, 40_000);
        let a2 = a.clone();
        let (base, log) = serve(move |_, _, _| Reply::ok(&a2));
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("damaged");
        let dir = ensure_with(spec, &root, &fast(), &mut |_| {}).unwrap();
        // Same size, one byte flipped: only the checksum can tell.
        let mut bytes = fs::read(dir.join("a.bin")).unwrap();
        bytes[100] ^= 0x80;
        fs::write(dir.join("a.bin"), &bytes).unwrap();
        assert!(is_present(spec, &root), "sizes alone look fine");
        assert!(format!("{:#}", verify(spec, &root).unwrap_err()).contains("checksum"));
        let mut events = vec![];
        ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert!(events.iter().any(|e| matches!(e, Progress::ChecksumFailed { .. })));
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        assert_eq!(log.lock().unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_stops_and_keeps_the_part_for_a_resume() {
        let a = content(8, 3_000_000);
        let a2 = a.clone();
        let (base, _) = serve(move |_, range, _| ranged(&a2, range));
        let spec = test_spec(&base, &[("a.bin", &a)], None);
        let root = tmp_root("cancel");
        let opts = fast();
        let cancel = opts.cancel.clone();
        let err = ensure_with(spec, &root, &opts, &mut |p| {
            if let Progress::Downloading { done, .. } = p
                && done > 0
            {
                cancel.store(true, Ordering::Release);
            }
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("cancel"), "{err:#}");
        let dir = root.join(spec.id);
        assert_eq!(leftovers(&dir), vec!["a.bin.part".to_string()]);
        // The next run resumes from the .part.
        let mut events = vec![];
        ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert!(events.iter().any(|e| matches!(e, Progress::Resumed { .. })));
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), a);
        let _ = fs::remove_dir_all(&root);
    }

    fn tar_gz(top: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut t = tar::Builder::new(gz);
        for (name, bytes) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            t.append_data(&mut h, format!("{top}/{name}"), *bytes).unwrap();
        }
        t.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn the_fallback_archive_is_used_when_the_pinned_source_fails() {
        // The pinned files (never served) and the fallback build's files.
        let a = content(9, 20_000);
        let fa = content(10, 21_000);
        let fb_b = content(11, 500);
        let archive = tar_gz("model-x", &[("a.bin", &fa), ("b.bin", &fb_b), ("README", b"ignored")]);
        let archive2 = archive.clone();
        let (base, log) = serve(move |p, _, _| if p == "/fallback.tar.gz" { Reply::ok(&archive2) } else { Reply::not_found() });
        let fb = leak(ArchiveSource {
            url: Box::leak(format!("{base}/fallback.tar.gz").into_boxed_str()),
            size: archive.len() as u64,
            sha256: sha(&archive),
            note: "test mirror",
            files: Box::leak(
                vec![
                    ModelFile { name: "a.bin", size: fa.len() as u64, sha256: sha(&fa) },
                    ModelFile { name: "b.bin", size: fb_b.len() as u64, sha256: sha(&fb_b) },
                ]
                .into_boxed_slice(),
            ),
        });
        let spec = test_spec(&base, &[("a.bin", &a), ("b.bin", &content(12, 400))], Some(fb));
        let root = tmp_root("fallback");

        // Without the fallback: an error.
        let no_fb = DownloadOptions { use_fallback: false, ..fast() };
        assert!(ensure_with(spec, &root, &no_fb, &mut |_| {}).is_err());

        let mut events = vec![];
        let dir = ensure_with(spec, &root, &fast(), &mut |p| events.push(p)).unwrap();
        assert!(events.iter().any(|e| matches!(e, Progress::Fallback { .. })));
        assert_eq!(fs::read(dir.join("a.bin")).unwrap(), fa);
        assert_eq!(fs::read(dir.join("b.bin")).unwrap(), fb_b);
        assert!(!dir.join("README").exists(), "only the expected files are unpacked");
        assert!(!root.join("test-model.fallback.tar.gz").exists(), "the archive is removed");
        // The fallback build counts as installed and verifies.
        assert!(std::ptr::eq(present_files(spec, &root).unwrap(), fb.files));
        verify(spec, &root).unwrap();
        // A second ensure leaves it alone (no new requests).
        let before = log.lock().unwrap().len();
        ensure_with(spec, &root, &fast(), &mut |_| {}).unwrap();
        assert_eq!(log.lock().unwrap().len(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_fallback_archive_is_rejected() {
        let a = content(13, 1_000);
        let archive = tar_gz("x", &[("a.bin", &content(14, 1_000))]);
        let (base, _) = serve({
            let archive = archive.clone();
            move |p, _, _| if p == "/fb.tar.gz" { Reply::ok(&archive) } else { Reply::not_found() }
        });
        // The archive checks out, but a file inside does not match its pin.
        let fb = leak(ArchiveSource {
            url: Box::leak(format!("{base}/fb.tar.gz").into_boxed_str()),
            size: archive.len() as u64,
            sha256: sha(&archive),
            note: "test mirror",
            files: Box::leak(vec![ModelFile { name: "a.bin", size: 1_000, sha256: sha(&a) }].into_boxed_slice()),
        });
        let spec = test_spec(&base, &[("a.bin", &a)], Some(fb));
        let root = tmp_root("badfb");
        let err = ensure_with(spec, &root, &fast(), &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("fallback"), "{err:#}");
        assert!(!root.join(spec.id).join("a.bin").exists());
        assert!(leftovers(&root.join(spec.id)).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_handy_fallback_is_pinned() {
        let fb = MOONSHINE_SMALL_STREAMING_EN.fallback.unwrap();
        assert!(fb.url.starts_with("https://blob.handy.computer/"));
        assert_eq!(fb.sha256.len(), 64);
        let mut names: Vec<_> = fb.files.iter().map(|f| f.name).collect();
        let mut pinned: Vec<_> = MOONSHINE_SMALL_STREAMING_EN.files.iter().map(|f| f.name).collect();
        names.sort();
        pinned.sort();
        assert_eq!(names, pinned, "same layout, so the runtime loads either");
        assert!(PHONON_31M_EN.fallback.is_none());
        assert!(MOONSHINE_SMALL_STREAMING_EN.license_text.contains("Permission is hereby granted"));
        assert!(PHONON_31M_EN.license_text.contains("Apache License"));
    }
}
