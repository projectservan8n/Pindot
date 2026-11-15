//! Every piece of OS-specific code sits behind one of these traits.
//!
//! Each trait has a macOS implementation (`macos`), a Windows implementation
//! (`windows`, stubs so far) and a mock (`mock`) for tests. Nothing outside
//! this module should call an OS API directly.

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod hotkey;
pub mod keys;
pub mod mock;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;

pub use hotkey::{BindingError, ChordTracker, HeldMods, HotkeyBinding, ModKey, Os, Side};
pub use keys::{KeyCombo, KeyName};

/// A macOS / Windows privacy permission the app may need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum Permission {
    Accessibility,
    InputMonitoring,
    ScreenRecording,
    Microphone,
}

impl Permission {
    pub const ALL: [Permission; 4] =
        [Permission::Accessibility, Permission::InputMonitoring, Permission::ScreenRecording, Permission::Microphone];

    /// What Deskhand uses it for, for the setup screen.
    pub fn why(self) -> &'static str {
        match self {
            Permission::Accessibility => "Click, type and read app windows for Claude, and swallow the hold-to-talk key",
            Permission::InputMonitoring => "See the hold-to-talk key and the Esc stop key",
            Permission::ScreenRecording => "Take the screenshots Claude asks for",
            Permission::Microphone => "Hear you while you hold the hold-to-talk key",
        }
    }

    /// The System Settings pane (macOS). Opening it is the user's button.
    pub fn settings_url(self) -> &'static str {
        match self {
            Permission::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            Permission::InputMonitoring => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
            Permission::ScreenRecording => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            Permission::Microphone => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        }
    }
}

/// Finer than granted / not granted (only the microphone reports all of
/// these on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    NotGranted,
    /// Never asked yet (microphone).
    NotAsked,
    Denied,
    /// Blocked by a device profile.
    Restricted,
    /// This OS needs no grant for it.
    NotNeeded,
    Unknown,
}

impl PermissionState {
    pub fn ok(self) -> bool {
        matches!(self, PermissionState::Granted | PermissionState::NotNeeded)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Permission::Accessibility => "Accessibility",
            Permission::InputMonitoring => "Input Monitoring",
            Permission::ScreenRecording => "Screen Recording",
            Permission::Microphone => "Microphone",
        };
        f.write_str(s)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("the {0} permission is not granted to this app")]
    PermissionMissing(Permission),
    #[error("not supported on this OS yet: {0}")]
    Unsupported(&'static str),
    /// An accessibility element no longer exists (window closed, UI rebuilt).
    #[error("that element no longer exists")]
    Stale,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// A point in the global desktop coordinate space: logical points (not
/// physical pixels), origin at the top-left of the primary display, y down.
/// This is the space CGEvent and the AX API use on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(&self, other: Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// A rectangle in global logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
    /// Overlap of two rectangles, `None` if they don't overlap.
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.width).min(o.x + o.width);
        let y1 = (self.y + self.height).min(o.y + o.height);
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// One monitor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DisplayInfo {
    /// OS display id (CGDirectDisplayID on macOS, HMONITOR-derived on Windows).
    pub id: u32,
    pub name: String,
    /// Bounds in global logical points.
    pub bounds: Rect,
    /// Physical pixels per logical point (2.0 on Retina).
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// A captured display image, in physical pixels.
pub struct CapturedFrame {
    pub display: DisplayInfo,
    pub image: image::RgbaImage,
}

/// Screen capture.
pub trait Capture: Send + Sync {
    /// All displays, primary first.
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    /// Capture one display by its `DisplayInfo::id`. Windows that opted out
    /// of capture (our overlay, approval and activity windows) are absent.
    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Synthetic mouse and keyboard input. Every event carries Deskhand's marker
/// so our own Esc tap can tell it from the user's keyboard.
pub trait Input: Send + Sync {
    /// Move the pointer to `at` (global logical points) and click `count`
    /// times (2 = double-click).
    fn click(&self, at: Point, button: MouseButton, count: u32) -> Result<()>;
    /// Move the pointer without clicking.
    fn move_to(&self, at: Point) -> Result<()>;
    /// Type text into whatever has keyboard focus. `\n` presses Return and
    /// `\t` presses Tab.
    fn type_text(&self, text: &str) -> Result<()>;
    /// Press a key combination (modifiers held while the key is clicked).
    fn key(&self, combo: &KeyCombo) -> Result<()>;
    /// Scroll at `at` by whole lines: `dy > 0` scrolls down, `dx > 0` right.
    fn scroll(&self, at: Point, dx: i32, dy: i32) -> Result<()>;
    /// Press the left button at `from`, move to `to` in small steps, release.
    /// `should_stop` is checked between steps; the button is always released.
    fn drag(&self, from: Point, to: Point, should_stop: &(dyn Fn() -> bool + Sync)) -> Result<()>;
    /// Where the user's pointer is now (`None` where unknown). Read only.
    fn pointer(&self) -> Option<Point> {
        None
    }
}

/// What a hold-to-talk hook reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HotkeyEvent {
    Down,
    Up,
    /// Another key or modifier joined the hold: it was a shortcut, so the
    /// recording is dropped.
    Cancel,
}

/// Stops a running listener when dropped (or when `stop` is called).
pub struct ListenerHandle {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl ListenerHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self { stop: Some(Box::new(stop)) }
    }
    pub fn stop(mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

/// A low-level hold-to-talk hotkey for a chord of modifier keys. Must report
/// Down/Up/Cancel (see `hotkey::ChordTracker`). Fn is swallowed so the OS
/// does not run the Globe action; other modifiers pass through (holding them
/// alone does nothing in apps, and swallowing them would break typing).
pub trait Hotkey: Send + Sync {
    fn start(&self, binding: HotkeyBinding, events: std::sync::mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle>;
}

/// The global emergency stop: a system-wide Esc listener that is armed only
/// while Claude controls input. A real Esc press is consumed (no app sees
/// it, so a prompt-injected page can't use it either) and reported through
/// `on_press`. Deskhand's own synthetic Esc (the `key` tool) passes through.
pub trait StopKey: Send + Sync {
    fn start(&self, on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle>;
}

/// A top-level window, as listed by the window server.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowInfo {
    /// CGWindowID on macOS, HWND on Windows.
    pub id: u64,
    pub pid: i32,
    /// Owning app's name.
    pub app: String,
    pub title: String,
    pub bounds: Rect,
    /// 0 = normal app window; menus, the menu bar, the Dock and overlays sit
    /// on higher layers.
    pub layer: i32,
    pub alpha: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppInfo {
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
}

/// An app found on disk or running, for `request_access`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedApp {
    pub name: String,
    pub bundle_id: Option<String>,
    pub running: bool,
}

/// Opaque, platform-owned reference to one accessibility element.
pub type ElementHandle = Arc<dyn Any + Send + Sync>;

/// One element of an accessibility tree.
#[derive(Clone)]
pub struct AxNode {
    pub handle: ElementHandle,
    pub depth: u16,
    /// Platform role, e.g. "AXButton".
    pub role: String,
    pub subrole: Option<String>,
    pub name: String,
    pub value: Option<String>,
    /// Bounds in global points, if the element has a frame on screen.
    pub bounds: Option<Rect>,
    pub enabled: bool,
    pub focused: bool,
    /// Keyboard shortcut shown next to a menu item, e.g. "⌘N".
    pub shortcut: Option<String>,
    /// For menu items: "File > New". `None` outside the menu bar.
    pub menu_path: Option<String>,
    /// Index into the same tree of the top-level menu bar item this menu
    /// item belongs to (where the AI cursor goes while a closed menu item is
    /// pressed).
    pub menu_root: Option<usize>,
}

impl fmt::Debug for AxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxNode")
            .field("depth", &self.depth)
            .field("role", &self.role)
            .field("name", &self.name)
            .field("bounds", &self.bounds)
            .finish()
    }
}

/// Limits for one tree walk, so a huge or hung app can't stall a tool call.
#[derive(Debug, Clone, Copy)]
pub struct TreeLimits {
    pub max_depth: u16,
    pub max_nodes: usize,
    pub timeout: Duration,
    /// 0 = no menu bar, 1 = menu bar titles only, 2+ = menu items too, to
    /// that depth (submenus count as one more level).
    pub menu_depth: u16,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self { max_depth: 40, max_nodes: 4000, timeout: Duration::from_secs(4), menu_depth: 1 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AxTree {
    pub nodes: Vec<AxNode>,
    /// The walk stopped at a limit; some elements are missing.
    pub truncated: bool,
}

/// What sits under a screen point, from the accessibility hit test.
/// An element found under a point that offers an AX action (see
/// `Accessibility::actionable_at`).
#[derive(Clone)]
pub struct ActionTarget {
    pub handle: ElementHandle,
    pub pid: i32,
    pub role: String,
    pub name: String,
}

/// Roles whose AXPress is a click. Pressing a list, row or window is not.
pub const PRESS_ROLES: &[&str] = &[
    "AXButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXMenuButton",
    "AXPopUpButton",
    "AXLink",
    "AXDisclosureTriangle",
];

#[derive(Debug, Clone)]
pub struct HitElement {
    pub pid: i32,
    pub role: String,
    pub name: String,
}

/// Windows, apps and accessibility trees (AX on macOS, UI Automation on
/// Windows).
pub trait Accessibility: Send + Sync {
    fn is_trusted(&self) -> bool;
    /// On-screen windows of every app, front to back.
    fn windows(&self) -> Result<Vec<WindowInfo>>;
    /// Running apps that have a user interface.
    fn running_apps(&self) -> Result<Vec<AppInfo>>;
    fn app_info(&self, pid: i32) -> Option<AppInfo>;
    /// The app that has keyboard focus.
    fn frontmost_pid(&self) -> Result<Option<i32>>;
    /// Find an app by name, running or installed.
    fn resolve_app(&self, name: &str) -> Option<ResolvedApp>;
    /// Launch an app by name (or bring it forward if it runs).
    fn launch_app(&self, name: &str) -> Result<()>;
    /// The window's element tree, then (per `limits.menu_depth`) its app's
    /// menu bar.
    fn window_tree(&self, window: &WindowInfo, limits: &TreeLimits) -> Result<AxTree>;
    /// Current frame of an element. `Err(Stale)` if it no longer exists,
    /// `Ok(None)` if it has no frame.
    fn element_frame(&self, el: &ElementHandle) -> Result<Option<Rect>>;
    /// Perform the element's default action (AXPress).
    fn press(&self, el: &ElementHandle) -> Result<()>;
    /// True if the element offers AXPress, so it can be clicked without
    /// moving the user's pointer. `Err(Stale)` if it no longer exists.
    fn can_press(&self, el: &ElementHandle) -> Result<bool>;
    /// The element under `at`, or one of its three nearest ancestors, that
    /// offers the AX `action` ("AXPress", "AXShowMenu") and whose frame
    /// covers the point. For "AXPress" only `PRESS_ROLES` count.
    fn actionable_at(&self, _at: Point, _action: &str) -> Result<Option<ActionTarget>> {
        Ok(None)
    }
    /// True if the element offers the AX `action`.
    fn has_action(&self, _el: &ElementHandle, _action: &str) -> Result<bool> {
        Ok(false)
    }
    /// Perform an AX action on the element (no pointer involved).
    fn perform(&self, _el: &ElementHandle, _action: &str) -> Result<()> {
        Err(PlatformError::Unsupported("accessibility actions"))
    }
    /// What is under a screen point.
    fn element_at(&self, at: Point) -> Result<Option<HitElement>>;
    /// Bring a window and its app to the front.
    fn raise_window(&self, window: &WindowInfo) -> Result<()>;
    /// True if keyboard focus is in a password field.
    fn focused_is_secure(&self) -> Result<bool>;
}

/// The on-screen AI cursor overlay.
pub trait Overlay: Send + Sync {
    /// Start animating the AI cursor to `to` (global logical points) and
    /// return how long the animation takes. Callers wait that long before
    /// acting, so the cursor always arrives before the click.
    fn move_cursor(&self, to: Point, label: Option<String>) -> Duration;
    /// Show or hide the "Claude is controlling input" indicator.
    fn set_acting(&self, acting: bool);
    /// Tell the overlay what the AI cursor just did (or, for a drag or
    /// typing, is doing), so it can show it.
    fn action(&self, _action: &CursorAction) {}
}

/// Steps of a synthetic drag from `from` to `to`, one every 16 ms.
pub fn drag_steps(from: Point, to: Point) -> usize {
    ((from.distance(to) / 12.0).ceil() as usize).clamp(8, 60)
}

/// How long a drag's travel takes, so the overlay can follow it.
pub fn drag_travel_ms(from: Point, to: Point) -> u64 {
    16 * drag_steps(from, to) as u64
}

/// What the AI cursor did, for the overlay's feedback.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CursorAction {
    /// A real mouse click: the user's pointer went there.
    Click { button: MouseButton, count: u32 },
    /// An accessibility press: the user's pointer did not move.
    Press,
    Scroll { dx: i32, dy: i32 },
    /// A drag that is starting: after a short grab (about 140 ms) the
    /// pointer travels to `to` in about `ms` (`drag_travel_ms`).
    Drag { to: Point, ms: u64 },
    /// A shortcut, as key-cap labels (`KeyCombo::caps`).
    Key { keys: Vec<String>, repeat: u32 },
    /// Typing that is starting: only the number of characters, never the text.
    Type { chars: usize },
    /// The `point` tool: show the user a spot (nothing is clicked).
    Point,
}

/// An overlay that draws nothing (headless runs).
#[derive(Default)]
pub struct NoOverlay;

impl Overlay for NoOverlay {
    fn move_cursor(&self, _to: Point, _label: Option<String>) -> Duration {
        Duration::ZERO
    }
    fn set_acting(&self, _acting: bool) {}
}

/// Permission status for the onboarding screen.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionStatus {
    pub permission: Permission,
    pub granted: bool,
    pub state: PermissionState,
    pub name: String,
    pub why: &'static str,
    pub settings_url: &'static str,
}

impl PermissionStatus {
    pub fn new(permission: Permission, state: PermissionState) -> Self {
        Self {
            permission,
            granted: state.ok(),
            state,
            name: permission.to_string(),
            why: permission.why(),
            settings_url: permission.settings_url(),
        }
    }
}

/// The real implementations for the OS this was built for.
#[derive(Clone)]
pub struct NativePlatform {
    pub capture: Arc<dyn Capture>,
    pub input: Arc<dyn Input>,
    pub hotkey: Arc<dyn Hotkey>,
    pub accessibility: Arc<dyn Accessibility>,
    pub stop_key: Arc<dyn StopKey>,
}

#[cfg(target_os = "macos")]
pub fn native() -> NativePlatform {
    NativePlatform {
        capture: Arc::new(macos::MacCapture),
        input: Arc::new(macos::MacInput),
        hotkey: Arc::new(macos::MacHotkey),
        accessibility: Arc::new(macos::MacAccessibility),
        stop_key: Arc::new(macos::MacStopKey),
    }
}

#[cfg(windows)]
pub fn native() -> NativePlatform {
    NativePlatform {
        capture: Arc::new(windows::WinCapture),
        input: Arc::new(windows::WinInput),
        hotkey: Arc::new(windows::WinHotkey),
        accessibility: Arc::new(windows::WinAccessibility),
        stop_key: Arc::new(windows::WinStopKey),
    }
}

/// The user's pointer in global points (for the companion cursor); `None`
/// where unsupported.
#[cfg(target_os = "macos")]
pub fn pointer_location() -> Option<Point> {
    macos::pointer_location()
}

#[cfg(windows)]
pub fn pointer_location() -> Option<Point> {
    None
}

#[cfg(target_os = "macos")]
pub fn permission_report() -> Vec<PermissionStatus> {
    macos::permission_report()
}

#[cfg(windows)]
pub fn permission_report() -> Vec<PermissionStatus> {
    windows::permission_report()
}

/// FOR THE USER'S BUTTON ONLY: may show a system prompt (macOS).
#[cfg(target_os = "macos")]
pub fn request_permission(p: Permission) -> PermissionState {
    macos::permissions::request(p)
}

#[cfg(windows)]
pub fn request_permission(_p: Permission) -> PermissionState {
    PermissionState::NotNeeded
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn every_permission_has_a_settings_pane() {
        let anchors = ["Privacy_Accessibility", "Privacy_ListenEvent", "Privacy_ScreenCapture", "Privacy_Microphone"];
        for (p, a) in Permission::ALL.iter().zip(anchors) {
            let url = p.settings_url();
            assert!(url.starts_with("x-apple.systempreferences:com.apple.preference.security?"), "{url}");
            assert!(url.ends_with(a), "{url}");
            assert!(!p.why().is_empty());
        }
        let s = PermissionStatus::new(Permission::Microphone, PermissionState::NotAsked);
        assert!(!s.granted);
        assert_eq!(s.name, "Microphone");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "not_asked");
        assert!(PermissionStatus::new(Permission::Microphone, PermissionState::NotNeeded).granted);
    }
}
