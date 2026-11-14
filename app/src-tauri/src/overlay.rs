//! The AI cursor: a transparent, click-through, always-on-top,
//! non-activating panel per monitor (NSPanel through tauri-nspanel on macOS).
//! `TauriOverlay` implements the core `Overlay` trait by emitting events the
//! panels animate.

use std::sync::Mutex;
use std::time::Duration;

use deskhand_core::platform::{CursorAction, Overlay, Point};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// One monitor in global logical points (top-left of the primary = 0,0).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Clone, Serialize)]
struct CursorMove {
    from: Option<Point>,
    to: Point,
    ms: u64,
    label: Option<String>,
}

pub struct TauriOverlay {
    app: AppHandle,
    last: Mutex<Option<Point>>,
}

impl TauriOverlay {
    pub fn new(app: AppHandle) -> Self {
        Self { app, last: Mutex::new(None) }
    }
}

/// Quick for short hops, capped so long moves never feel slow.
fn animation_ms(distance: f64) -> u64 {
    (250.0 + distance * 0.45).clamp(250.0, 1100.0) as u64
}

impl Overlay for TauriOverlay {
    fn move_cursor(&self, to: Point, label: Option<String>) -> Duration {
        let mut last = self.last.lock().unwrap();
        let from = *last;
        let ms = from.map(|f| animation_ms(f.distance(to))).unwrap_or(300);
        *last = Some(to);
        if let Err(e) = self.app.emit("deskhand:cursor", CursorMove { from, to, ms, label }) {
            tracing::warn!("overlay emit failed: {e}");
        }
        // Small margin so the click lands after the last frame is drawn.
        Duration::from_millis(ms + 60)
    }

    fn set_acting(&self, acting: bool) {
        let _ = self.app.emit("deskhand:acting", acting);
    }

    fn action(&self, action: &CursorAction) {
        // A drag carries the cursor along, so the next glide starts at its end.
        if let CursorAction::Drag { to, .. } = action {
            *self.last.lock().unwrap() = Some(*to);
        }
        if let Err(e) = self.app.emit("deskhand:action", action) {
            tracing::warn!("overlay emit failed: {e}");
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use tauri::{AppHandle, LogicalPosition, LogicalSize, Position, Size, WebviewUrl};
    use tauri_nspanel::objc2_app_kit::NSWindowSharingType;
    use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask, tauri_panel};

    use super::MonitorRect;

    tauri_panel! {
        panel!(OverlayPanel {
            config: {
                can_become_key_window: false,
                can_become_main_window: false,
                is_floating_panel: true
            }
        })
    }

    pub fn create_panel(app: &AppHandle, index: usize, m: &MonitorRect, capturable: bool) -> tauri::Result<()> {
        let script = format!(
            "window.__DESKHAND_MONITOR__ = {{ index: {index}, x: {}, y: {}, width: {}, height: {} }};",
            m.x, m.y, m.width, m.height
        );
        let panel = PanelBuilder::<_, OverlayPanel>::new(app, format!("overlay-{index}"))
            .url(WebviewUrl::App("overlay.html".into()))
            .title("Deskhand cursor")
            .position(Position::Logical(LogicalPosition::new(m.x, m.y)))
            .size(Size::Logical(LogicalSize::new(m.width, m.height)))
            .level(PanelLevel::ScreenSaver)
            .transparent(true)
            .opaque(false)
            .has_shadow(false)
            .ignores_mouse_events(true)
            .hides_on_deactivate(false)
            .no_activate(true)
            .style_mask(StyleMask::empty().borderless().nonactivating_panel())
            .collection_behavior(
                CollectionBehavior::new().can_join_all_spaces().stationary().full_screen_auxiliary().ignores_cycle(),
            )
            .with_window(move |w| {
                w.transparent(true)
                    .decorations(false)
                    .shadow(false)
                    .skip_taskbar(true)
                    .focused(false)
                    .focusable(false)
                    .resizable(false)
                    .initialization_script(script)
            })
            .build()?;
        // Keep the AI cursor out of screen captures, including our own
        // screenshot tool, so the model never sees (or clicks) it.
        // `--capturable` turns this off so tests can photograph the cursor.
        if !capturable {
            panel.as_panel().setSharingType(NSWindowSharingType::None);
        }
        panel.show();
        Ok(())
    }

    /// Prints the live NSApp activation policy (Accessory = no Dock icon,
    /// never the active app unless a window asks for it).
    pub fn log_activation_policy() {
        use tauri_nspanel::objc2::MainThreadMarker;
        use tauri_nspanel::objc2_app_kit::NSApplication;
        if let Some(mtm) = MainThreadMarker::new() {
            let policy = NSApplication::sharedApplication(mtm).activationPolicy();
            println!("activation policy at launch: {policy:?}");
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{create_panel, log_activation_policy};

/// Windows: a plain transparent, always-on-top, click-through webview window.
/// Built but untested in Phase 0 (see docs/windows-test-checklist.md).
#[cfg(not(target_os = "macos"))]
pub fn create_panel(app: &AppHandle, index: usize, m: &MonitorRect, capturable: bool) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let script = format!(
        "window.__DESKHAND_MONITOR__ = {{ index: {index}, x: {}, y: {}, width: {}, height: {} }};",
        m.x, m.y, m.width, m.height
    );
    let w = WebviewWindowBuilder::new(app, format!("overlay-{index}"), WebviewUrl::App("overlay.html".into()))
        .position(m.x, m.y)
        .inner_size(m.width, m.height)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .resizable(false)
        .content_protected(!capturable)
        .initialization_script(script)
        .build()?;
    w.set_ignore_cursor_events(true)?;
    Ok(())
}
