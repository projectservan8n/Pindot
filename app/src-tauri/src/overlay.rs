//! The AI cursor: a transparent, click-through, always-on-top,
//! non-activating panel per monitor (NSPanel through tauri-nspanel on macOS).
//! `TauriOverlay` implements the core `Overlay` trait by emitting events the
//! panels animate.

use std::sync::Mutex;
use std::time::Duration;

use pindot_core::platform::{CursorAction, Overlay, Point};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// One monitor in global points (top-left of the primary = 0,0), the
/// points the tools and the cursor events use: logical points on macOS,
/// physical pixels on Windows (`windows_logic::display_info` says why).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    /// Points per CSS pixel of the monitor's overlay page: 1 on macOS, the
    /// monitor's scale on Windows.
    pub unit: f64,
}

impl MonitorRect {
    /// From the monitor's physical position, size and scale.
    /// `points_are_pixels`: the Windows rule.
    pub fn from_physical(position: (i32, i32), size: (u32, u32), scale: f64, points_are_pixels: bool) -> Self {
        // Points per physical pixel, and per CSS pixel of the page.
        let (per_pixel, unit) = if points_are_pixels { (1.0, scale) } else { (1.0 / scale, 1.0) };
        Self {
            x: f64::from(position.0) * per_pixel,
            y: f64::from(position.1) * per_pixel,
            width: f64::from(size.0) * per_pixel,
            height: f64::from(size.1) * per_pixel,
            scale,
            unit,
        }
    }

    /// The size of the monitor's overlay page in CSS pixels.
    pub fn page_size(&self) -> (f64, f64) {
        (self.width / self.unit, self.height / self.unit)
    }
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
    /// Points per CSS pixel (see `MonitorRect::unit`), of the main monitor.
    unit: f64,
}

impl TauriOverlay {
    pub fn new(app: AppHandle, unit: f64) -> Self {
        Self { app, last: Mutex::new(None), unit }
    }
}

/// Quick for short hops, capped so long moves never feel slow. `distance`
/// is in points, `unit` points make one pixel on the page: a glide across
/// the same stretch of screen takes the same time at any scale.
fn animation_ms(distance: f64, unit: f64) -> u64 {
    (250.0 + distance / unit * 0.45).clamp(250.0, 1100.0) as u64
}

impl Overlay for TauriOverlay {
    fn move_cursor(&self, to: Point, label: Option<String>) -> Duration {
        let mut last = self.last.lock().unwrap();
        let from = *last;
        let ms = from.map(|f| animation_ms(f.distance(to), self.unit)).unwrap_or(300);
        *last = Some(to);
        if let Err(e) = self.app.emit("pindot:cursor", CursorMove { from, to, ms, label }) {
            tracing::warn!("overlay emit failed: {e}");
        }
        // Small margin so the click lands after the last frame is drawn.
        Duration::from_millis(ms + 60)
    }

    fn set_acting(&self, acting: bool) {
        let _ = self.app.emit("pindot:acting", acting);
    }

    fn action(&self, action: &CursorAction) {
        // A drag carries the cursor along, so the next glide starts at its end.
        if let CursorAction::Drag { to, .. } = action {
            *self.last.lock().unwrap() = Some(*to);
        }
        if let Err(e) = self.app.emit("pindot:action", action) {
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
            "window.__PINDOT_MONITOR__ = {{ index: {index}, x: {}, y: {}, width: {}, height: {} }};",
            m.x, m.y, m.width, m.height
        );
        let panel = PanelBuilder::<_, OverlayPanel>::new(app, format!("overlay-{index}"))
            .url(WebviewUrl::App("overlay.html".into()))
            .title("Pindot cursor")
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
    // The page lays itself out in CSS pixels; `unit` points make one.
    let (page_w, page_h) = m.page_size();
    let script = format!(
        "window.__PINDOT_MONITOR__ = {{ index: {index}, x: {}, y: {}, width: {page_w}, height: {page_h}, unit: {} }};",
        m.x, m.y, m.unit
    );
    let w = WebviewWindowBuilder::new(app, format!("overlay-{index}"), WebviewUrl::App("overlay.html".into()))
        .title("Pindot cursor")
        .position(m.x / m.unit, m.y / m.unit)
        .inner_size(page_w, page_h)
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
    // Exactly over its monitor, in pixels: a logical position is ambiguous
    // once two monitors have different scales.
    #[cfg(windows)]
    {
        w.set_position(tauri::PhysicalPosition::new(m.x.round() as i32, m.y.round() as i32))?;
        w.set_size(tauri::PhysicalSize::new(m.width.round() as u32, m.height.round() as u32))?;
    }
    // A tool window: not in the Alt+Tab list or the taskbar (the window
    // library makes it an "app window", which is forced into both).
    #[cfg(windows)]
    if let Ok(h) = w.hwnd() {
        pindot_core::platform::windows::window::hide_from_switcher(h.0 as isize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_macos_a_point_is_a_css_pixel() {
        let m = MonitorRect::from_physical((0, 0), (2880, 1800), 2.0, false);
        assert_eq!((m.x, m.y, m.width, m.height, m.unit), (0.0, 0.0, 1440.0, 900.0, 1.0));
        assert_eq!(m.page_size(), (1440.0, 900.0));
    }

    #[test]
    fn on_windows_a_point_is_a_pixel_and_the_page_is_scaled() {
        // A 150% monitor of 2560x1440 pixels to the right of a 1920 wide one.
        let m = MonitorRect::from_physical((1920, 0), (2560, 1440), 1.5, true);
        assert_eq!((m.x, m.y, m.width, m.height, m.unit), (1920.0, 0.0, 2560.0, 1440.0, 1.5));
        let (w, h) = m.page_size();
        assert!((w - 1706.667).abs() < 0.01, "{w}");
        assert_eq!(h, 960.0);
        // At 100% nothing changes.
        let plain = MonitorRect::from_physical((-1920, 0), (1920, 1080), 1.0, true);
        assert_eq!((plain.x, plain.width, plain.unit, plain.page_size()), (-1920.0, 1920.0, 1.0, (1920.0, 1080.0)));
    }

    #[test]
    fn a_glide_takes_as_long_at_any_scale() {
        // 600 pixels on the page: 600 points on macOS, 900 on a 150% Windows monitor.
        assert_eq!(animation_ms(600.0, 1.0), 520);
        assert_eq!(animation_ms(900.0, 1.5), 520);
        assert_eq!(animation_ms(0.0, 1.0), 250);
        assert_eq!(animation_ms(100_000.0, 2.0), 1100);
    }
}
