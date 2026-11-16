//! The per-app approval prompt, answered by a human in Pindot's own
//! window. One small always-on-top window per request (`approval-<id>`).
//! It is content-protected, so screenshots (ours included) never show it,
//! and Pindot itself can never be approved, so Claude cannot click it.
//! Closing the window counts as "Deny".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pindot_core::platform::{Accessibility, WindowInfo};
use pindot_core::server::approval::{ApprovalRequest, Approver};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

pub const LABEL_PREFIX: &str = "approval-";

struct Pending {
    /// Also shown inline in the chat window; either place can answer.
    req: ApprovalRequest,
    reply: oneshot::Sender<bool>,
    /// The window that was in front before the prompt took focus.
    restore: Option<WindowInfo>,
}

pub struct TauriApprover {
    app: AppHandle,
    ax: Arc<dyn Accessibility>,
    capturable: bool,
    pending: Mutex<HashMap<String, Pending>>,
}

impl TauriApprover {
    pub fn new(app: AppHandle, ax: Arc<dyn Accessibility>, capturable: bool) -> Self {
        Self { app, ax, capturable, pending: Mutex::new(HashMap::new()) }
    }

    fn label(id: &str) -> String {
        let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        format!("{LABEL_PREFIX}{safe}")
    }

    /// The user answered in window `label` (or closed it: `allow = false`).
    pub fn answer_from_window(&self, label: &str, allow: bool) {
        let Some(id) = label.strip_prefix(LABEL_PREFIX) else { return };
        let Some(p) = self.pending.lock().unwrap().remove(id) else { return };
        println!("approval {id}: user {}", if allow { "ALLOWED" } else { "denied" });
        let _ = p.reply.send(allow);
        let _ = self.app.emit("pindot:approval-done", serde_json::json!({ "id": p.req.id, "allowed": allow }));
        self.close(label);
        // Give focus back to the window the user was in.
        if let Some(w) = p.restore {
            let ax = self.ax.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let _ = ax.raise_window(&w);
            });
        }
    }

    /// Requests waiting for an answer, for the chat window.
    pub fn pending_requests(&self) -> Vec<ApprovalRequest> {
        self.pending.lock().unwrap().values().map(|p| p.req.clone()).collect()
    }

    fn close(&self, label: &str) {
        if let Some(w) = self.app.get_webview_window(label) {
            let _ = w.destroy();
        }
    }

    fn front_window(&self) -> Option<WindowInfo> {
        let pid = self.ax.frontmost_pid().ok().flatten()?;
        if pid == std::process::id() as i32 {
            return None;
        }
        self.ax.windows().ok()?.into_iter().find(|w| w.pid == pid && w.layer == 0)
    }
}

impl Approver for TauriApprover {
    fn ask(&self, req: ApprovalRequest, reply: oneshot::Sender<bool>) {
        let label = Self::label(&req.id);
        let id = label[LABEL_PREFIX.len()..].to_string();
        let restore = self.front_window();
        let json = serde_json::to_string(&req).unwrap_or_else(|_| "null".into());
        let _ = self.app.emit("pindot:approval", &req);
        self.pending.lock().unwrap().insert(id, Pending { req: req.clone(), reply, restore });
        let script = format!("window.__PINDOT_APPROVAL__ = {json};");
        let height = if req.login.is_some() { 330.0 } else { 230.0 + 64.0 * req.apps.len().min(6) as f64 };
        let title = if req.login.is_some() { "Pindot: let Claude sign in?" } else { "Pindot: allow Claude to control an app?" };
        let app = self.app.clone();
        let protect = !self.capturable;
        let _ = self.app.run_on_main_thread(move || {
            let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("approval.html".into()))
                .title(title)
                .inner_size(460.0, height)
                .resizable(false)
                .minimizable(false)
                .maximizable(false)
                .always_on_top(true)
                .visible_on_all_workspaces(true)
                .center()
                .focused(true)
                .content_protected(protect)
                .initialization_script(script)
                .build();
            match built {
                Ok(w) => {
                    let _ = w.set_focus();
                }
                Err(e) => tracing::error!("approval window failed: {e}"),
            }
        });
    }

    fn cancel(&self, id: &str) {
        let label = Self::label(id);
        // Dropping the sender counts as deny; the caller already timed out.
        if let Some(p) = self.pending.lock().unwrap().remove(&label[LABEL_PREFIX.len()..]) {
            let _ = self.app.emit("pindot:approval-done", serde_json::json!({ "id": p.req.id, "allowed": false }));
        }
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(w) = app.get_webview_window(&label) {
                let _ = w.destroy();
            }
        });
    }
}

/// Called by the approval page's buttons. The request id comes from the
/// calling window's label, so a page can only answer its own request.
#[tauri::command]
pub fn approval_answer(window: tauri::WebviewWindow, approver: tauri::State<'_, Arc<TauriApprover>>, allow: bool) {
    approver.answer_from_window(window.label(), allow);
}

/// The chat window's Allow / Deny. Same decision, same pending request, same
/// `answer_from_window` as the approval window's buttons: only the main
/// window may use it (a page can still only answer a request that is
/// pending, and Pindot's own windows can never be approved for control, so
/// Claude cannot click these buttons).
#[tauri::command]
pub fn approval_answer_inline(window: tauri::WebviewWindow, approver: tauri::State<'_, Arc<TauriApprover>>, id: String, allow: bool) {
    if window.label() != crate::ui::MAIN {
        return;
    }
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    approver.answer_from_window(&format!("{LABEL_PREFIX}{safe}"), allow);
}

#[tauri::command]
pub fn approvals_pending(window: tauri::WebviewWindow, approver: tauri::State<'_, Arc<TauriApprover>>) -> Vec<ApprovalRequest> {
    if window.label() != crate::ui::MAIN {
        return vec![];
    }
    approver.pending_requests()
}
