//! Per-app approval. Claude calls `request_access(apps, reason)`; the user
//! answers in Deskhand's own window ("Allow for this session" / "Deny").
//! Nothing works on an app until it is approved in the current session.
//!
//! The only way around the human is `DESKHAND_TEST_AUTO_APPROVE`, a
//! test-only switch (off by default) that pre-approves the named apps and is
//! announced loudly in the log every time it is used.

use serde::Serialize;
use tokio::sync::oneshot;

pub const AUTO_APPROVE_ENV: &str = "DESKHAND_TEST_AUTO_APPROVE";

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
    ];
    const FILES: &[&str] = &["finder", "path finder", "forklift", "commander one", "transmit", "cyberduck"];
    const SETTINGS: &[&str] = &[
        "system settings", "system preferences", "disk utility", "activity monitor", "console", "migration assistant",
        "boot camp assistant", "directory utility", "systempreferences",
    ];
    const PASSWORDS: &[&str] = &[
        "passwords", "keychain access", "1password", "1password 7", "bitwarden", "dashlane", "lastpass", "keepassxc",
        "enpass", "proton pass", "nordpass", "roboform", "keeper", "keychain",
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
/// app names). Empty unless the variable is set. Logs loudly when on.
pub fn auto_approve_from_env() -> Vec<String> {
    let Ok(v) = std::env::var(AUTO_APPROVE_ENV) else { return vec![] };
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

/// Is this app Deskhand itself? It can never be approved: Claude must not
/// be able to click its own approval prompt or stop button.
pub fn is_deskhand(name: &str, bundle_id: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("deskhand") || bundle_id.is_some_and(|b| b.to_lowercase().contains("deskhand"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn deskhand_is_never_approvable() {
        assert!(is_deskhand("Deskhand", None));
        assert!(is_deskhand("whatever", Some("dev.deskhand.app")));
        assert!(!is_deskhand("TextEdit", Some("com.apple.TextEdit")));
    }
}
