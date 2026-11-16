//! The AI cursor: a transparent, click-through, always-on-top,
//! non-activating panel per monitor (NSPanel through tauri-nspanel on macOS).
//! `TauriOverlay` implements the core `Overlay` trait by emitting events the
//! panels animate.

use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// The longest the AI cursor waits on a spot the `point` tool showed.
pub const PARK_MAX: Duration = Duration::from_secs(120);
/// The user's pointer this close to the spot (in points) counts as there.
pub const PARK_NEAR: f64 = 40.0;

/// A spot the `point` tool showed. With the companion on, the AI cursor
/// stays on it until the user's pointer gets there (the user may take a
/// while to find it, or be listening to the explanation), instead of
/// flying back beside their pointer after a few seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parked {
    pub at: Point,
    pub until: Instant,
}

/// Where the AI cursor is waiting, if anywhere. Set by the `point` tool,
/// cleared by the next glide, Esc, a new spoken request, the user's pointer
/// reaching the spot, or `PARK_MAX`.
#[derive(Default)]
pub struct Park(Mutex<Option<Parked>>);

impl Park {
    pub fn set(&self, at: Point, now: Instant) {
        *self.0.lock().unwrap() = Some(Parked { at, until: now + PARK_MAX });
    }

    pub fn get(&self) -> Option<Parked> {
        *self.0.lock().unwrap()
    }

    /// Ends the wait; true if the cursor was waiting.
    pub fn clear(&self) -> bool {
        self.0.lock().unwrap().take().is_some()
    }

    /// With the user's pointer (if known): true, once, when the wait is
    /// over because the pointer reached the spot or the time ran out.
    pub fn check(&self, pointer: Option<Point>, now: Instant) -> bool {
        let mut g = self.0.lock().unwrap();
        let Some(p) = *g else { return false };
        let reached = pointer.is_some_and(|q| q.distance(p.at) <= PARK_NEAR);
        if reached || now >= p.until {
            *g = None;
            return true;
        }
        false
    }
}

pub struct TauriOverlay {
    app: AppHandle,
    last: Mutex<Option<Point>>,
    park: Park,
    /// Points per CSS pixel (see `MonitorRect::unit`), of the main monitor.
    unit: f64,
}

impl TauriOverlay {
    pub fn new(app: AppHandle, unit: f64) -> Self {
        Self { app, last: Mutex::new(None), park: Park::default(), unit }
    }

    /// Where the cursor is waiting after a `point`, if anywhere.
    pub fn parked(&self) -> Option<Parked> {
        self.park.get()
    }

    /// The companion feed calls this with the user's pointer; when the wait
    /// is over, the overlay is told to let the cursor come back.
    pub fn check_park(&self, pointer: Option<Point>, now: Instant) {
        if self.park.check(pointer, now) {
            let _ = self.app.emit("pindot:unpark", ());
        }
    }

    /// Ends a wait at once (Esc, or the user talking again).
    pub fn unpark(&self) {
        if self.park.clear() {
            let _ = self.app.emit("pindot:unpark", ());
        }
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
        // A new target: whatever the cursor was waiting on is done with.
        self.park.clear();
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
        // `point` glides to the spot first, so `last` is the spot.
        if let CursorAction::Point = action
            && let Some(at) = *self.last.lock().unwrap()
        {
            self.park.set(at, Instant::now());
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

    /// Streaming mode: let screen capture see (or not see) every overlay
    /// panel. `ReadOnly` is the normal readable value; `None` hides the panel
    /// from every capture. Runs on the main thread, where AppKit wants it.
    pub fn set_visible_to_capture(app: &AppHandle, on: bool) {
        use tauri::Manager;
        use tauri_nspanel::ManagerExt;
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            for (label, _) in app2.webview_windows() {
                if !label.starts_with("overlay-") {
                    continue;
                }
                if let Ok(panel) = app2.get_webview_panel(&label) {
                    let kind = if on { NSWindowSharingType::ReadOnly } else { NSWindowSharingType::None };
                    panel.as_panel().setSharingType(kind);
                }
            }
        });
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
pub use mac::{create_panel, log_activation_policy, set_visible_to_capture};

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

/// Streaming mode: let screen capture see (or not see) every overlay
/// window. Only the overlay: the main window and the approval popup stay
/// protected.
#[cfg(not(target_os = "macos"))]
pub fn set_visible_to_capture(app: &AppHandle, on: bool) {
    use tauri::Manager;
    for (label, w) in app.webview_windows() {
        if label.starts_with("overlay-") {
            if let Err(e) = w.set_content_protected(!on) {
                tracing::warn!("{label}: content protection did not change: {e}");
            }
        }
    }
}

/// Is streaming mode really in force? The setting must be on, this OS must
/// be able to do it (macOS 14+), and `--capturable` (everything visible, our
/// own screenshots too) takes over when given.
pub fn streaming_in_force(setting: bool, supported: bool, capturable: bool) -> bool {
    setting && supported && !capturable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_needs_the_setting_the_os_and_no_capturable_flag() {
        assert!(streaming_in_force(true, true, false));
        assert!(!streaming_in_force(false, true, false));
        // macOS 13: never switch the overlay to capturable.
        assert!(!streaming_in_force(true, false, false));
        assert!(!streaming_in_force(true, true, true));
    }

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

    fn pt(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn a_point_waits_until_the_users_pointer_gets_there() {
        let park = Park::default();
        let t0 = Instant::now();
        park.set(pt(500.0, 300.0), t0);
        // The user hasn't moved there yet: keep waiting, however long it takes them.
        assert!(!park.check(Some(pt(100.0, 100.0)), t0 + Duration::from_secs(30)));
        assert!(park.get().is_some());
        // Close, but not on it yet.
        assert!(!park.check(Some(pt(500.0 + PARK_NEAR + 5.0, 300.0)), t0 + Duration::from_secs(31)));
        // On it: the wait is over, once.
        assert!(park.check(Some(pt(510.0, 290.0)), t0 + Duration::from_secs(32)));
        assert!(park.get().is_none());
        assert!(!park.check(Some(pt(510.0, 290.0)), t0 + Duration::from_secs(33)));
    }

    #[test]
    fn a_point_gives_up_after_the_longest_wait() {
        let park = Park::default();
        let t0 = Instant::now();
        park.set(pt(500.0, 300.0), t0);
        assert!(!park.check(None, t0 + PARK_MAX - Duration::from_millis(1)));
        assert!(park.check(None, t0 + PARK_MAX));
        assert!(park.get().is_none());
    }

    #[test]
    fn clearing_a_park_says_whether_there_was_one() {
        let park = Park::default();
        assert!(!park.clear());
        park.set(pt(1.0, 2.0), Instant::now());
        assert_eq!(park.get().map(|p| p.at), Some(pt(1.0, 2.0)));
        assert!(park.clear());
        assert!(!park.clear());
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
