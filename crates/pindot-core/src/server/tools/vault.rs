//! The two vault tools. The model names a saved login and a field; Pindot
//! checks the page, asks the user, and types the secret itself. The secret
//! never enters a tool result, the activity log, an error message or a
//! screenshot request: the only things that leave this file are the words
//! "typed" or the reason for a refusal.
//!
//! Every check here is code, not prompt:
//!  1. the browser's app is approved for the session AND the user approved
//!     this login for the session (never auto-approved);
//!  2. the frontmost app is a known browser and every web-area URL above the
//!     focused element exactly equals the saved origin;
//!  3. a password goes only into a field the system marks secure;
//!  4. the secret is not echoed anywhere (read_window masks secure fields);
//!  5. a rate limit and a lock; Esc cancels a fill in flight.

use zeroize::Zeroizing;

use super::*;
use crate::platform::{AppInfo, FocusedPage};
use crate::vault::origin;
use crate::vault::{Field, LoginSummary, UseEntry};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VaultFillArgs {
    /// The saved login's name or id, from vault_logins.
    pub login: String,
    /// "username", "password" or "code" (the current authenticator code).
    pub field: String,
}

/// Bundle ids of browsers whose page address Pindot can read.
const BROWSER_BUNDLES: &[&str] = &[
    "com.apple.Safari",
    "com.apple.SafariTechnologyPreview",
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "org.chromium.Chromium",
    "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta",
    "com.brave.Browser",
    "com.vivaldi.Vivaldi",
    "company.thebrowser.Browser",
    "com.operasoftware.Opera",
];

/// The same browsers on Windows, where an app's id is its executable's name
/// in lower case (`platform::windows::apps`), whatever the Start menu calls
/// the app. Chrome's channels and Chromium are all `chrome.exe`.
const BROWSER_EXES: &[&str] = &["msedge.exe", "chrome.exe", "brave.exe", "vivaldi.exe", "opera.exe", "arc.exe"];

/// Names, for an app that has no id at all. Exact, case-insensitive.
const BROWSER_NAMES: &[&str] = &[
    "safari", "google chrome", "chrome", "chromium", "microsoft edge", "msedge", "edge", "brave browser", "brave",
    "vivaldi", "arc", "opera",
];

pub fn is_browser(app: &AppInfo) -> bool {
    match app.bundle_id.as_deref() {
        Some(b) => BROWSER_BUNDLES.iter().chain(BROWSER_EXES).any(|k| k.eq_ignore_ascii_case(b)),
        None => BROWSER_NAMES.iter().any(|k| k.eq_ignore_ascii_case(app.name.trim().trim_end_matches(".exe"))),
    }
}

/// Is `page` a place where `login`'s `field` may be typed? `Err(why)` is
/// a plain-words refusal; nothing secret is in it.
pub fn page_allows(login: &LoginSummary, field: Field, page: &FocusedPage) -> Result<(), String> {
    if page.page_urls.is_empty() {
        return Err("Pindot cannot read which website this page is (no web page around the focused field).".into());
    }
    for url in &page.page_urls {
        if !origin::matches(&login.origin, url) {
            return Err(format!(
                "this page is not {} (exact address, including https and the port). Nothing was typed.",
                login.origin
            ));
        }
    }
    match field {
        Field::Password if !page.secure => {
            Err("the focused field is not a password field (the system does not mark it secure). Click the password box first. Nothing was typed.".into())
        }
        Field::Username if page.secure => Err("the focused field is a password field; click the username box. Nothing was typed.".into()),
        Field::Username if !page.text_input => Err("the focused element is not a text box. Click the username box first.".into()),
        Field::Code if !page.secure && !page.text_input => Err("the focused element is not a text box. Click the code box first.".into()),
        _ => Ok(()),
    }
}

impl DesktopServer {
    fn log_use(&self, l: Option<&LoginSummary>, field: &str, result: &str, detail: &str, ok: bool) {
        self.state.vault.append_use(&UseEntry {
            ts: super::super::activity::utc_now(),
            login_id: l.map(|l| l.id.clone()).unwrap_or_default(),
            login_name: l.map(|l| l.name.clone()).unwrap_or_default(),
            site: l.map(|l| l.origin.clone()).unwrap_or_default(),
            field: field.to_string(),
            result: result.to_string(),
            detail: truncate(detail, 200),
            session: self.session.id.clone(),
            ok,
        });
    }

    /// Record a refusal and build the tool error. `security` refusals (wrong
    /// site, not a browser, wrong field) count towards the lock.
    fn refuse(&self, l: Option<&LoginSummary>, field: &str, why: String, security: bool) -> String {
        let mut msg = format!("Refused: {why}");
        if security && self.session.fill_gate.note_refusal(Instant::now()) {
            msg.push_str(" The vault is now locked for this session after repeated refusals.");
        }
        self.log_use(l, field, "refused", &why, false);
        msg
    }

    fn resolve_login(&self, which: &str) -> Result<LoginSummary, String> {
        let all = self.state.vault.list().map_err(|e| e.to_string())?;
        let w = which.trim();
        if let Some(l) = all.iter().find(|l| l.id == w) {
            return Ok(l.clone());
        }
        let hits: Vec<&LoginSummary> = all.iter().filter(|l| l.name.eq_ignore_ascii_case(w)).collect();
        match hits.len() {
            1 => Ok(hits[0].clone()),
            0 => Err(format!("no saved login called \"{}\". Call vault_logins for the saved logins on this site.", truncate(w, 60))),
            _ => Err(format!("more than one saved login is called \"{}\"; use its id from vault_logins.", truncate(w, 60))),
        }
    }

    async fn page_now(&self) -> Result<(AppInfo, Option<FocusedPage>), String> {
        let ax = self.state.accessibility.clone();
        blocking(move || {
            let pid = ax.frontmost_pid().map_err(perr)?.ok_or_else(|| "No app has keyboard focus.".to_string())?;
            let app = ax.app_info(pid).ok_or_else(|| "The frontmost app is unknown.".to_string())?;
            let page = ax.focused_page().map_err(perr)?;
            Ok::<_, String>((app, page))
        })
        .await?
    }

    /// The whole page check. `Err((reason, security))`.
    async fn check_fill_target(&self, l: &LoginSummary, field: Field) -> Result<(), (String, bool)> {
        self.require_approved_focus().await.map_err(|e| (e, false))?;
        let (app, page) = self.page_now().await.map_err(|e| (e, false))?;
        if !is_browser(&app) {
            return Err((
                format!("{} is not a browser Pindot can read the address of (Safari, Chrome, Edge, Brave, Vivaldi, Arc and Opera are). Nothing was typed.", app.name),
                true,
            ));
        }
        let Some(page) = page else {
            return Err(("Pindot cannot tell which web page has focus. Click into the page's form first.".into(), false));
        };
        let front = self.state.accessibility.frontmost_pid().ok().flatten();
        if front != Some(page.pid) {
            return Err(("the focused field does not belong to the frontmost app. Nothing was typed.".into(), true));
        }
        page_allows(l, field, &page).map_err(|e| (e, true))
    }

    pub(super) async fn do_vault_logins(&self) -> Out {
        self.require_approved_focus().await?;
        let (app, page) = self.page_now().await?;
        if !is_browser(&app) {
            return text(format!("{} is not a browser, so no saved login applies. The vault only signs in on websites.", app.name));
        }
        let Some(page) = page.filter(|p| !p.page_urls.is_empty()) else {
            return text("Pindot cannot read the website in front (click into the page first). No logins listed.");
        };
        let all = self.state.vault.list().map_err(|e| e.to_string())?;
        let mut lines = vec![];
        for l in all.iter().filter(|l| !l.locked && page.page_urls.iter().all(|u| origin::matches(&l.origin, u))) {
            lines.push(format!(
                "- {} (id {}): username {}{}",
                l.name,
                l.id,
                if l.username.is_empty() { "(none)" } else { l.username.as_str() },
                if l.has_2fa { ", has an authenticator code" } else { "" }
            ));
        }
        if lines.is_empty() {
            return text("No saved login for this website. Ask the user to add it in Pindot's Vault; never ask them to say or type a password to you.");
        }
        text(format!(
            "Saved logins for this website:\n{}\nUse vault_fill(login, field) with the keyboard focus in the matching box. You never see the secrets.",
            lines.join("\n")
        ))
    }

    pub(super) async fn do_vault_fill(&self, args: VaultFillArgs) -> Out {
        let Some(field) = Field::parse(&args.field) else {
            return Err("field must be \"username\", \"password\" or \"code\".".into());
        };
        let fname = field.label();
        let login = match self.resolve_login(&args.login) {
            Ok(l) => l,
            Err(e) => return Err(self.refuse(None, fname, e, false)),
        };
        let l = &login;
        if l.locked {
            return Err(self.refuse(Some(l), fname, "this login can't be opened with the current vault key.".into(), false));
        }
        match self.state.vault.has_field(&l.id, field) {
            Ok(true) => {}
            Ok(false) => return Err(self.refuse(Some(l), fname, format!("this login has no saved {fname}."), false)),
            Err(e) => return Err(self.refuse(Some(l), fname, e.to_string(), false)),
        }
        if let Err(why) = self.session.fill_gate.check(Instant::now()) {
            self.log_use(Some(l), fname, "refused", &why, false);
            return Err(format!("Refused: {why}"));
        }
        let guard = self.begin().inspect_err(|e| self.log_use(Some(l), fname, "refused", e, false))?;

        if let Err((why, sec)) = self.check_fill_target(l, field).await {
            return Err(self.refuse(Some(l), fname, why, sec));
        }

        // The user's say-so for this login, once per session. Never automatic.
        if !self.session.is_login_approved(&l.id) {
            if self.state.control.approval_pending() {
                return Err(self.refuse(Some(l), fname, "an approval prompt is already open; wait for the user's answer.".into(), false));
            }
            let req = ApprovalRequest {
                id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
                session: self.session.id.clone(),
                apps: vec![],
                reason: String::new(),
                login: Some(super::super::approval::LoginRequest {
                    name: l.name.clone(),
                    site: l.origin.clone(),
                    username: l.username.clone(),
                    field: fname.to_string(),
                }),
            };
            let id = req.id.clone();
            let (tx, rx) = oneshot::channel();
            self.state.control.set_approval_pending(true);
            self.state.approver.ask(req, tx);
            let answer = tokio::time::timeout(APPROVAL_TIMEOUT, rx).await;
            self.state.control.set_approval_pending(false);
            match answer {
                Ok(Ok(true)) => {
                    self.session.approve_login(&l.id);
                    self.state.activity.record(&self.session.id, "approval", &format!("login {}", l.name), "user allowed this login for the session", true);
                }
                Ok(_) => {
                    self.state.activity.record(&self.session.id, "approval", &format!("login {}", l.name), "user denied", false);
                    return Err(self.refuse(Some(l), fname, "the user did not allow this login. Do not try to work around it; ask the user.".into(), false));
                }
                Err(_) => {
                    self.state.approver.cancel(&id);
                    self.state.activity.record(&self.session.id, "approval", &format!("login {}", l.name), "timed out: denied", false);
                    return Err(self.refuse(Some(l), fname, "the user did not answer in time, so the login was not allowed.".into(), false));
                }
            }
            // Two minutes may have passed: look again at where the focus is.
            if let Err((why, sec)) = self.check_fill_target(l, field).await {
                return Err(self.refuse(Some(l), fname, why, sec));
            }
        }

        let secret = match self.state.vault.secret(&l.id, field) {
            Ok(s) => s,
            Err(e) => return Err(self.refuse(Some(l), fname, e.to_string(), false)),
        };
        let chars: Vec<char> = secret.chars().collect();
        self.state.overlay.action(&CursorAction::Type { chars: chars.len() });
        let mut typed = 0usize;
        for chunk in chars.chunks(24) {
            if guard.is_cancelled() {
                self.log_use(Some(l), fname, "refused", "stopped by Esc while typing", false);
                return Err(format!("{STOPPED} (Part of the {fname} may have been typed; the user should clear the field.)"));
            }
            if typed > 0 {
                if let Err((why, sec)) = self.check_fill_target(l, field).await {
                    return Err(self.refuse(Some(l), fname, format!("{why} Part of the {fname} may have been typed."), sec));
                }
            }
            let part: Zeroizing<String> = Zeroizing::new(chunk.iter().collect());
            let input = self.state.input.clone();
            let res = blocking(move || input.type_text(&part)).await?;
            if let Err(e) = res {
                self.log_use(Some(l), fname, "refused", "the keyboard input failed", false);
                return Err(format!("Typing failed: {}", perr(e)));
            }
            typed += chunk.len();
        }
        drop(chars);
        self.session.fill_gate.note_fill(Instant::now());
        self.log_use(Some(l), fname, "typed", "", true);
        text("typed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, id: Option<&str>) -> AppInfo {
        AppInfo { pid: 1, name: name.into(), bundle_id: id.map(Into::into) }
    }

    /// Windows has no bundle ids: there an app's id is its executable's
    /// name in lower case, whatever the Start menu calls the app.
    #[test]
    fn a_windows_browser_is_known_by_its_executable() {
        for (name, exe) in [
            ("Microsoft Edge", "msedge.exe"),
            ("Google Chrome", "chrome.exe"),
            ("Brave", "brave.exe"),
            ("Vivaldi", "vivaldi.exe"),
            ("Opera", "opera.exe"),
            ("Arc", "arc.exe"),
        ] {
            assert!(is_browser(&app(name, Some(exe))), "{exe}");
        }
    }

    #[test]
    fn other_windows_programs_are_not_browsers_whatever_they_call_themselves() {
        assert!(!is_browser(&app("Notepad", Some("notepad.exe"))));
        assert!(!is_browser(&app("Firefox", Some("firefox.exe"))), "Firefox does not give the page's address");
        assert!(!is_browser(&app("Google Chrome", Some("updater.exe"))), "the name alone does not count");
        assert!(!is_browser(&app("File Explorer", Some("explorer.exe"))));
    }

    #[test]
    fn mac_browsers_are_still_known_by_their_bundle_id() {
        assert!(is_browser(&app("Safari", Some("com.apple.Safari"))));
        assert!(is_browser(&app("Google Chrome", Some("com.google.Chrome"))));
        assert!(!is_browser(&app("Safari", Some("com.example.NotSafari"))));
        assert!(is_browser(&app("Safari", None)), "an app without an id goes by its name");
    }
}
