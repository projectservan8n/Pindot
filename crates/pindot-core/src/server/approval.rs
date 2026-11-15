//! Per-app approval. Claude calls `request_access(apps, reason)`; the user
//! answers in Pindot's own window ("Allow for this session" / "Deny").
//! Nothing works on an app until it is approved in the current session.
//!
//! The only way around the human is `PINDOT_TEST_AUTO_APPROVE`, a
//! test-only switch (off by default) that pre-approves the named apps and is
//! announced loudly in the log every time it is used.

use serde::Serialize;
use tokio::sync::oneshot;

pub const AUTO_APPROVE_ENV: &str = "PINDOT_TEST_AUTO_APPROVE";

/// One app in an approval request.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AppRequest {
    pub name: String,
    pub bundle_id: Option<String>,
    pub running: bool,
    /// Extra warning shown for high-risk apps.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    /// Short id of the MCP session asking.
    pub session: String,
    pub apps: Vec<AppRequest>,
    /// Claude's stated reason, shown to the user as-is (untrusted text).
    pub reason: String,
}

/// Shows approval requests to the user. `ask` must not block: it shows the
/// prompt and later sends `true` (allow) or `false` (deny) on `reply`.
/// Dropping `reply` counts as deny.
pub trait Approver: Send + Sync {
    fn ask(&self, req: ApprovalRequest, reply: oneshot::Sender<bool>);
    /// The request timed out or was withdrawn; close its prompt.
    fn cancel(&self, id: &str);
}

/// For headless runs with no UI: every request is denied.
pub struct NoApprover;

impl Approver for NoApprover {
    fn ask(&self, req: ApprovalRequest, reply: oneshot::Sender<bool>) {
        tracing::warn!(id = %req.id, "approval requested but there is no approval UI; denying");
        let _ = reply.send(false);
    }
    fn cancel(&self, _id: &str) {}
}

/// Category warnings, shown in the approval window and returned to Claude.
pub fn warning_for(name: &str, bundle_id: Option<&str>) -> Option<String> {
    let n = name.to_lowercase();
    let b = bundle_id.unwrap_or("").to_lowercase();
    let any = |list: &[&str]| list.iter().any(|x| n == *x || n.starts_with(&format!("{x} ")) || b.contains(x));
    const SHELL: &[&str] = &[
        "terminal", "iterm", "iterm2", "warp", "ghostty", "kitty", "alacritty", "wezterm", "hyper", "tabby",
        "visual studio code", "code", "vscode", "cursor", "windsurf", "xcode", "zed", "intellij idea", "pycharm",
        "webstorm", "clion", "goland", "rider", "rustrover", "android studio", "sublime text", "nova", "bbedit",
        "script editor", "automator", "shortcuts", "utm", "docker", "docker desktop",
        // Windows (names as in the Start menu, and executables).
        "windows powershell", "powershell", "pwsh", "command prompt", "cmd.exe", "windows terminal", "git bash",
        "mintty", "visual studio", "wsl", "ubuntu", "windows subsystem for linux",
    ];
    const FILES: &[&str] = &[
        "finder", "path finder", "forklift", "commander one", "transmit", "cyberduck",
        "file explorer", "windows explorer", "explorer.exe", "total commander", "winscp", "filezilla",
    ];
    const SETTINGS: &[&str] = &[
        "system settings", "system preferences", "disk utility", "activity monitor", "console", "migration assistant",
        "boot camp assistant", "directory utility", "systempreferences",
        "settings", "systemsettings", "control panel", "registry editor", "regedit", "task manager", "taskmgr",
        "device manager", "computer management", "services", "event viewer", "disk management", "windows security",
        "group policy", "mmc.exe",
    ];
    const PASSWORDS: &[&str] = &[
        "passwords", "keychain access", "1password", "1password 7", "bitwarden", "dashlane", "lastpass", "keepassxc",
        "enpass", "proton pass", "nordpass", "roboform", "keeper", "keychain", "credential manager", "keepass",
    ];
    if any(PASSWORDS) {
        Some("Password manager: Claude could read or change your saved passwords.".into())
    } else if any(SHELL) {
        Some("Equivalent to shell access: Claude could run any command on this computer.".into())
    } else if any(FILES) {
        Some("File manager: Claude could read, move or delete any of your files.".into())
    } else if any(SETTINGS) {
        Some("System settings: Claude could change how this computer is set up.".into())
    } else {
        None
    }
}

/// The test-only auto-approve list from the environment (comma-separated
/// app names). Always empty unless this build has the `test-switches` cargo
/// feature AND the variable is set; logs loudly either way.
pub fn auto_approve_from_env() -> Vec<String> {
    auto_approve_list(std::env::var(AUTO_APPROVE_ENV).ok(), cfg!(feature = "test-switches"))
}

fn auto_approve_list(value: Option<String>, switches_built_in: bool) -> Vec<String> {
    let Some(v) = value else { return vec![] };
    if !switches_built_in {
        let msg = format!(
            "{AUTO_APPROVE_ENV} is set but this build has no test switches (cargo feature `test-switches`); ignored, every app still asks"
        );
        tracing::warn!("{msg}");
        eprintln!("{msg}");
        return vec![];
    }
    let list: Vec<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if !list.is_empty() {
        let msg = format!(
            "!!! TEST-ONLY: {AUTO_APPROVE_ENV} is set; these apps are approved WITHOUT asking the user: {}. Never use this outside automated tests. !!!",
            list.join(", ")
        );
        tracing::warn!("{msg}");
        eprintln!("{msg}");
    }
    list
}

/// Is this app Pindot itself? It can never be approved: Claude must not
/// be able to click its own approval prompt or stop button.
pub fn is_pindot(name: &str, bundle_id: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("pindot") || bundle_id.is_some_and(|b| b.to_lowercase().contains("pindot"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_needs_the_test_switches_feature() {
        assert!(auto_approve_list(Some("TextEdit".into()), false).is_empty());
        assert!(auto_approve_list(None, true).is_empty());
        assert_eq!(auto_approve_list(Some(" TextEdit, test-target ,".into()), true), ["TextEdit", "test-target"]);
    }

    #[test]
    fn warnings_by_category() {
        assert!(warning_for("Terminal", Some("com.apple.Terminal")).unwrap().contains("shell"));
        assert!(warning_for("Visual Studio Code", None).unwrap().contains("shell"));
        assert!(warning_for("Finder", Some("com.apple.finder")).unwrap().contains("files"));
        assert!(warning_for("System Settings", None).unwrap().contains("set up"));
        assert!(warning_for("1Password 7", None).unwrap().contains("passwords"));
        assert!(warning_for("Passwords", Some("com.apple.Passwords")).unwrap().contains("passwords"));
        assert_eq!(warning_for("TextEdit", Some("com.apple.TextEdit")), None);
        assert_eq!(warning_for("Calculator", None), None);
    }

    #[test]
    fn warnings_for_windows_apps() {
        // Names as the Start menu shows them; the id is the executable.
        assert!(warning_for("Windows PowerShell", Some("powershell.exe")).unwrap().contains("shell"));
        assert!(warning_for("PowerShell 7 (x64)", Some("pwsh.exe")).unwrap().contains("shell"));
        assert!(warning_for("Command Prompt", Some("cmd.exe")).unwrap().contains("shell"));
        assert!(warning_for("Terminal", Some("windowsterminal.exe")).unwrap().contains("shell"));
        assert!(warning_for("Git Bash", Some("mintty.exe")).unwrap().contains("shell"));
        assert!(warning_for("File Explorer", Some("explorer.exe")).unwrap().contains("files"));
        assert!(warning_for("Settings", Some("systemsettings.exe")).unwrap().contains("set up"));
        assert!(warning_for("Control Panel", None).unwrap().contains("set up"));
        assert!(warning_for("Registry Editor", Some("regedit.exe")).unwrap().contains("set up"));
        assert!(warning_for("Task Manager", Some("taskmgr.exe")).unwrap().contains("set up"));
        assert!(warning_for("Credential Manager", None).unwrap().contains("passwords"));
        assert_eq!(warning_for("Notepad", Some("notepad.exe")), None);
        assert_eq!(warning_for("Google Chrome", Some("chrome.exe")), None);
        assert_eq!(warning_for("Paint", Some("mspaint.exe")), None);
    }

    #[test]
    fn pindot_is_never_approvable() {
        assert!(is_pindot("Pindot", None));
        assert!(is_pindot("whatever", Some("dev.pindot.app")));
        assert!(!is_pindot("TextEdit", Some("com.apple.TextEdit")));
    }
}
