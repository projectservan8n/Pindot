//! The local activity log: one JSON line per tool call, approval and stop,
//! in `<data dir>/activity.jsonl` (user-only, 0600, rotated at 5 MB), plus
//! an in-memory ring and a broadcast for the app's activity view.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const RING: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivityEntry {
    /// UTC, ISO 8601.
    pub ts: String,
    /// Short id of the MCP session (one per `claude` process).
    pub session: String,
    /// Tool name, or "approval" / "stop" / "lock".
    pub tool: String,
    /// Short summary of the arguments (typed text is cut to 80 chars).
    pub args: String,
    pub outcome: String,
    pub ok: bool,
}

pub struct ActivityLog {
    path: Option<PathBuf>,
    ring: Mutex<VecDeque<ActivityEntry>>,
    tx: broadcast::Sender<ActivityEntry>,
}

impl ActivityLog {
    /// Log to `path` (created 0600) and memory.
    pub fn open(path: PathBuf) -> Self {
        Self { path: Some(path), ring: Mutex::new(VecDeque::new()), tx: broadcast::channel(256).0 }
    }

    /// Memory only (tests, headless runs without a data dir).
    pub fn memory() -> Self {
        Self { path: None, ring: Mutex::new(VecDeque::new()), tx: broadcast::channel(256).0 }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ActivityEntry> {
        self.tx.subscribe()
    }

    pub fn record(&self, session: &str, tool: &str, args: &str, outcome: &str, ok: bool) -> ActivityEntry {
        let e = ActivityEntry {
            ts: utc_now(),
            session: session.into(),
            tool: tool.into(),
            args: args.into(),
            outcome: truncate(outcome, 300),
            ok,
        };
        {
            let mut r = self.ring.lock().unwrap();
            if r.len() >= RING {
                r.pop_front();
            }
            r.push_back(e.clone());
        }
        if let Some(p) = &self.path {
            if let Err(err) = append(p, &e) {
                tracing::warn!("activity log write failed: {err}");
            }
        }
        let _ = self.tx.send(e.clone());
        e
    }

    /// The newest `n` entries, oldest first. Falls back to the file when the
    /// ring is empty (e.g. the viewer started after a restart).
    pub fn recent(&self, n: usize) -> Vec<ActivityEntry> {
        let r = self.ring.lock().unwrap();
        if !r.is_empty() {
            return r.iter().skip(r.len().saturating_sub(n)).cloned().collect();
        }
        drop(r);
        let Some(p) = &self.path else { return vec![] };
        let Ok(text) = fs::read_to_string(p) else { return vec![] };
        let all: Vec<ActivityEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        all[all.len().saturating_sub(n)..].to_vec()
    }
}

fn append(path: &Path, e: &ActivityEntry) -> std::io::Result<()> {
    if fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(path, path.with_extension("1.jsonl"));
    }
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    let mut line = serde_json::to_string(e).unwrap_or_default();
    line.push('\n');
    f.write_all(line.as_bytes())
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

/// Current time as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn utc_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    utc_iso(d.as_secs() as i64, d.subsec_millis())
}

/// Unix seconds to ISO 8601 UTC (Howard Hinnant's civil-from-days).
pub fn utc_iso(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(utc_iso(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc_iso(951_782_400, 5), "2000-02-29T00:00:00.005Z");
        assert_eq!(utc_iso(1_794_614_400, 0), "2026-11-14T00:00:00.000Z");
    }

    #[test]
    fn records_to_file_and_ring() {
        let dir = std::env::temp_dir().join(format!("deskhand-activity-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("activity.jsonl");
        let _ = fs::remove_file(&p);
        let log = ActivityLog::open(p.clone());
        log.record("s1", "click", "ref=e3", "clicked", true);
        log.record("s1", "type_text", "\"hello\"", "typed", true);
        assert_eq!(log.recent(10).len(), 2);
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A fresh log (new process) reads history back from the file.
        let again = ActivityLog::open(p.clone());
        assert_eq!(again.recent(1)[0].tool, "type_text");
        let _ = fs::remove_dir_all(&dir);
    }
}
