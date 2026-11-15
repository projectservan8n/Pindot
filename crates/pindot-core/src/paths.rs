//! Where Pindot keeps its runtime files, and the MCP bearer token.
//!
//! Everything secret lives in the per-user data directory with user-only
//! permissions (dir 0700, files 0600), never in the repo:
//! macOS `~/Library/Application Support/Pindot`, Windows `%APPDATA%\Pindot`.
//! A `Deskhand` folder from before the rename is moved there on first use.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::RngCore;

/// The data folder's name under the per-user data directory.
pub const DIR_NAME: &str = "Pindot";
/// Its name before the rename (Pindot was called Deskhand until Nov 2026).
pub const OLD_DIR_NAME: &str = "Deskhand";

/// The per-user data directory (created 0700). `PINDOT_DATA_DIR` moves it,
/// for a clean first-run check or a second copy; it only changes where files
/// go, nothing else.
pub fn data_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("PINDOT_DATA_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => {
            let base = dirs::data_dir().context("no per-user data directory on this system")?;
            let dir = base.join(DIR_NAME);
            static MOVE_OLD: std::sync::Once = std::sync::Once::new();
            MOVE_OLD.call_once(|| {
                move_old_data_dir(&base.join(OLD_DIR_NAME), &dir);
            });
            dir
        }
    };
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Move the folder from before the rename (settings, token, models, the
/// activity log) to the new name, once: only when the new one doesn't exist
/// yet. A failed move leaves the old folder alone; the app then starts fresh.
fn move_old_data_dir(old: &Path, new: &Path) -> bool {
    if new.exists() || !old.is_dir() {
        return false;
    }
    match fs::rename(old, new) {
        Ok(()) => {
            tracing::info!("moved the data folder {} to {}", old.display(), new.display());
            true
        }
        Err(e) => {
            tracing::warn!("could not move {} to {}: {e}", old.display(), new.display());
            false
        }
    }
}

/// Create `dir` if needed and make it user-only (0700 on Unix).
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn token_path() -> Result<PathBuf> {
    Ok(token_path_in(&data_dir()?))
}

pub fn token_path_in(dir: &Path) -> PathBuf {
    dir.join("mcp-token")
}

/// The local activity log: one JSON line per tool call (0600, rotated at 5 MB).
pub fn activity_log_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("activity.jsonl"))
}

/// Write `contents` to `path` readable and writable by the user only.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Load the bearer token, creating it on first run.
pub fn load_or_create_token() -> Result<String> {
    load_or_create_token_in(&data_dir()?)
}

/// Replace the bearer token (Settings > "Regenerate token").
pub fn regenerate_token() -> Result<String> {
    regenerate_token_in(&data_dir()?)
}

pub fn load_or_create_token_in(dir: &Path) -> Result<String> {
    let path = token_path_in(dir);
    if let Ok(t) = fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            // Tighten a token file someone loosened by hand.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            }
            return Ok(t);
        }
    }
    regenerate_token_in(dir)
}

pub fn regenerate_token_in(dir: &Path) -> Result<String> {
    ensure_private_dir(dir)?;
    let t = new_token();
    write_private(&token_path_in(dir), t.as_bytes())?;
    Ok(t)
}

/// Constant-time string comparison for secrets.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape() {
        let t = new_token();
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(t, new_token());
    }

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pindot-paths-{tag}-{}", uuid::Uuid::new_v4()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn token_file_is_user_only_and_regenerates() {
        let dir = temp_dir("token");
        let t1 = load_or_create_token_in(&dir).unwrap();
        assert_eq!(t1.len(), 64);
        // Loading again returns the same token.
        assert_eq!(load_or_create_token_in(&dir).unwrap(), t1);
        #[cfg(unix)]
        {
            assert_eq!(mode(&dir), 0o700, "data dir must be 0700");
            assert_eq!(mode(&token_path_in(&dir)), 0o600, "token file must be 0600");
        }
        let t2 = regenerate_token_in(&dir).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(fs::read_to_string(token_path_in(&dir)).unwrap(), t2);
        #[cfg(unix)]
        assert_eq!(mode(&token_path_in(&dir)), 0o600, "regenerated token file must be 0600");
        // No temp file left behind by the atomic write.
        assert!(!token_path_in(&dir).with_extension("tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn loosened_token_file_is_tightened_and_bad_token_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("loose");
        let t = load_or_create_token_in(&dir).unwrap();
        let path = token_path_in(&dir);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create_token_in(&dir).unwrap(), t);
        assert_eq!(mode(&path), 0o600);
        fs::write(&path, "not-a-token").unwrap();
        let t2 = load_or_create_token_in(&dir).unwrap();
        assert_ne!(t2, "not-a-token");
        assert_eq!(t2.len(), 64);
        assert_eq!(mode(&path), 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_old_deskhand_folder_moves_to_the_new_name_once() {
        let base = temp_dir("rename");
        let old = base.join(OLD_DIR_NAME);
        let new = base.join(DIR_NAME);
        fs::create_dir_all(old.join("models")).unwrap();
        fs::write(old.join("settings.json"), "{}").unwrap();
        fs::write(old.join("models").join("weights.bin"), "w").unwrap();

        assert!(move_old_data_dir(&old, &new));
        assert!(!old.exists(), "the old folder is gone");
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), "{}");
        assert_eq!(fs::read_to_string(new.join("models").join("weights.bin")).unwrap(), "w");

        // Both present (an old copy came back): the new one wins, nothing moves.
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("settings.json"), "old").unwrap();
        assert!(!move_old_data_dir(&old, &new));
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), "{}");
        assert_eq!(fs::read_to_string(old.join("settings.json")).unwrap(), "old");

        // Neither present: nothing to do.
        fs::remove_dir_all(&base).unwrap();
        assert!(!move_old_data_dir(&old, &new));
        assert!(!new.exists());
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }
}
