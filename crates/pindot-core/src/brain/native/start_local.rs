//! Starting a local model server that is installed but not running.
//!
//! LM Studio's server is off unless its "start on launch" is set, so the
//! app can be open with the server down, and Ollama may simply not be
//! running: in both cases Setup said only "No model server is running"
//! (seen on Windows, 17 Nov 2026, with LM Studio open). Pindot now says
//! which one is installed and starts it when the user presses the button,
//! with the server's own command (`lms server start`, `ollama serve`).
//! Nothing here runs by itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::probe;
use super::providers;

/// A local server whose start program is on this computer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Installed {
    pub provider: String,
    pub name: String,
}

/// Where the program that starts `provider`'s server may be, most likely
/// first, besides the PATH. `local_app_data` is `%LOCALAPPDATA%` on Windows.
pub fn cli_places(provider: &str, home: &Path, local_app_data: Option<&Path>, windows: bool) -> Vec<PathBuf> {
    match provider {
        // LM Studio puts its `lms` CLI in its own folder (older versions in ~/.cache).
        "lmstudio" => {
            let lms = if windows { "lms.exe" } else { "lms" };
            vec![home.join(".lmstudio").join("bin").join(lms), home.join(".cache").join("lm-studio").join("bin").join(lms)]
        }
        "ollama" if windows => {
            local_app_data.map(|l| l.join("Programs").join("Ollama").join("ollama.exe")).into_iter().collect()
        }
        "ollama" => vec![
            PathBuf::from("/usr/local/bin/ollama"),
            PathBuf::from("/opt/homebrew/bin/ollama"),
            PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama"),
        ],
        _ => vec![],
    }
}

/// The program's name on the PATH.
pub fn cli_name(provider: &str, windows: bool) -> Option<String> {
    let base = match provider {
        "lmstudio" => "lms",
        "ollama" => "ollama",
        _ => return None,
    };
    Some(if windows { format!("{base}.exe") } else { base.to_string() })
}

/// The arguments that start the server, and whether the program stays
/// running as the server itself (`ollama serve`) rather than returning once
/// the server is up (`lms server start`).
pub fn start_args(provider: &str) -> Option<(&'static [&'static str], bool)> {
    match provider {
        "lmstudio" => Some((&["server", "start"], false)),
        "ollama" => Some((&["serve"], true)),
        _ => None,
    }
}

/// The start program for `provider` on this computer, if there is one.
pub fn find_cli(provider: &str) -> Option<PathBuf> {
    let windows = cfg!(windows);
    let name = cli_name(provider, windows)?;
    if let Some(path) = std::env::var_os("PATH")
        && let Some(found) = std::env::split_paths(&path).map(|d| d.join(&name)).find(|c| c.is_file())
    {
        return Some(found);
    }
    let home = dirs::home_dir()?;
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    cli_places(provider, &home, local.as_deref(), windows).into_iter().find(|c| c.is_file())
}

/// The local servers installed here (whether running or not).
pub fn installed() -> Vec<Installed> {
    providers::PROVIDERS
        .iter()
        .filter(|p| p.local && find_cli(p.id).is_some())
        .map(|p| Installed { provider: p.id.into(), name: p.name.into() })
        .collect()
}

/// Start `provider`'s server and wait until it answers (up to 20 s).
/// Errors are plain words for the Setup screen.
pub fn start(provider: &str) -> Result<(), String> {
    let p = providers::find(provider).filter(|p| p.local).ok_or("Pindot cannot start that server.")?;
    let (args, stays) = start_args(provider).ok_or("Pindot cannot start that server.")?;
    let cli = find_cli(provider).ok_or_else(|| format!("{} is not installed on this computer.", p.name))?;
    let mut cmd = Command::new(&cli);
    cmd.args(args).stdin(Stdio::null());
    crate::brain::hidden(&mut cmd);
    if stays {
        // The server itself: left running after this returns.
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        cmd.spawn().map_err(|e| format!("{} could not be started: {e}", p.name))?;
    } else {
        let out = cmd.output().map_err(|e| format!("{} could not be started: {e}", p.name))?;
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>();
            return Err(format!("{} did not start its server{}", p.name, if said.is_empty() { ".".into() } else { format!(": {said}") }));
        }
    }
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if probe::list_models(provider, p.base_url, None).is_ok() {
            tracing::info!("{} server started by the user's request", p.name);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    Err(format!("{} was started but its server did not answer at {} within 20 seconds.", p.name, p.base_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lm_studio_is_started_with_its_own_cli_from_its_home_folder() {
        let home = Path::new("/home/me");
        assert_eq!(
            cli_places("lmstudio", home, None, false),
            vec![home.join(".lmstudio/bin/lms"), home.join(".cache/lm-studio/bin/lms")]
        );
        let win = cli_places("lmstudio", Path::new(r"C:\Users\me"), Some(Path::new(r"C:\Users\me\AppData\Local")), true);
        assert_eq!(win[0], Path::new(r"C:\Users\me").join(".lmstudio").join("bin").join("lms.exe"));
        assert_eq!(cli_name("lmstudio", true).as_deref(), Some("lms.exe"));
        assert_eq!(cli_name("lmstudio", false).as_deref(), Some("lms"));
        assert_eq!(start_args("lmstudio"), Some((&["server", "start"][..], false)), "returns once the server is up");
    }

    #[test]
    fn ollama_is_started_with_ollama_serve_which_keeps_running() {
        let local = Path::new(r"C:\Users\me\AppData\Local");
        assert_eq!(
            cli_places("ollama", Path::new(r"C:\Users\me"), Some(local), true),
            vec![local.join("Programs").join("Ollama").join("ollama.exe")]
        );
        let mac = cli_places("ollama", Path::new("/Users/me"), None, false);
        assert!(mac.contains(&PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama")), "{mac:?}");
        assert!(mac.contains(&PathBuf::from("/opt/homebrew/bin/ollama")));
        assert_eq!(cli_name("ollama", true).as_deref(), Some("ollama.exe"));
        assert_eq!(start_args("ollama"), Some((&["serve"][..], true)), "the program is the server");
    }

    #[test]
    fn nothing_else_is_started() {
        for p in ["llamacpp", "openai", "anthropic", "custom", "nonsense"] {
            assert!(start_args(p).is_none(), "{p}");
            assert!(cli_places(p, Path::new("/home/me"), None, false).is_empty(), "{p}");
            assert!(cli_name(p, false).is_none(), "{p}");
        }
    }
}
