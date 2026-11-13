//! Where Deskhand keeps its runtime files, and the MCP bearer token.
//!
//! Everything secret lives in the per-user data directory with user-only
//! permissions (dir 0700, files 0600), never in the repo:
//! macOS `~/Library/Application Support/Deskhand`, Windows `%APPDATA%\Deskhand`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::RngCore;

pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().context("no per-user data directory on this system")?;
    let dir = base.join("Deskhand");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

pub fn token_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("mcp-token"))
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
    let path = token_path()?;
    if let Ok(t) = fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(t);
        }
    }
    regenerate_token()
}

/// Replace the bearer token (Settings > "Regenerate token").
pub fn regenerate_token() -> Result<String> {
    let t = new_token();
    write_private(&token_path()?, t.as_bytes())?;
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

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }
}
