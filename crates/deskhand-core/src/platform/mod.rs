//! Every piece of OS-specific code sits behind one of these traits.
//!
//! Each trait has a macOS implementation (`macos`), a Windows implementation
//! (`windows`, stubs in Phase 0) and a mock (`mock`) for tests. Nothing outside
//! this module should call an OS API directly.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod mock;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;

/// A macOS / Windows privacy permission the app may need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Permission {
    Accessibility,
    InputMonitoring,
    ScreenRecording,
    Microphone,
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
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// A point in the global desktop coordinate space: logical points (not
/// physical pixels), origin at the top-left of the primary display, y down.
/// This is the space CGEvent uses on macOS.
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
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
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
    /// Capture one display by its `DisplayInfo::id`.
    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Synthetic mouse and keyboard input.
pub trait Input: Send + Sync {
    /// Move the pointer to `at` (global logical points) and click.
    fn click(&self, at: Point, button: MouseButton, count: u32) -> Result<()>;
}

/// The push-to-talk hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotkeyBinding {
    /// Hold Fn / Globe (macOS default).
    Fn,
    /// Hold Ctrl+Win (Windows default).
    CtrlWin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HotkeyEvent {
    Down,
    Up,
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

/// A low-level hold-to-talk hotkey. Must report down/up for modifier-only
/// keys and swallow the key so the OS does not act on it.
pub trait Hotkey: Send + Sync {
    fn start(
        &self,
        binding: HotkeyBinding,
        events: std::sync::mpsc::Sender<HotkeyEvent>,
    ) -> Result<ListenerHandle>;
}

/// Accessibility (AX on macOS, UI Automation on Windows). Phase 0 only needs
/// the trust check; Phase 1 adds the tree reader behind this same trait.
pub trait Accessibility: Send + Sync {
    fn is_trusted(&self) -> bool;
    fn read_window(&self, _window_id: Option<u64>) -> Result<String> {
        Err(PlatformError::Unsupported("read_window arrives in Phase 1"))
    }
}

/// The on-screen AI cursor overlay.
pub trait Overlay: Send + Sync {
    /// Start animating the AI cursor to `to` (global logical points) and
    /// return how long the animation takes. Callers wait that long before
    /// acting, so the cursor always arrives before the click.
    fn move_cursor(&self, to: Point, label: Option<String>) -> Duration;
    /// Show or hide the "Claude is controlling input" indicator.
    fn set_acting(&self, acting: bool);
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
}

/// The real implementations for the OS this was built for.
#[derive(Clone)]
pub struct NativePlatform {
    pub capture: Arc<dyn Capture>,
    pub input: Arc<dyn Input>,
    pub hotkey: Arc<dyn Hotkey>,
    pub accessibility: Arc<dyn Accessibility>,
}

#[cfg(target_os = "macos")]
pub fn native() -> NativePlatform {
    NativePlatform {
        capture: Arc::new(macos::MacCapture),
        input: Arc::new(macos::MacInput::default()),
        hotkey: Arc::new(macos::MacHotkey),
        accessibility: Arc::new(macos::MacAccessibility),
    }
}

#[cfg(windows)]
pub fn native() -> NativePlatform {
    NativePlatform {
        capture: Arc::new(windows::WinCapture),
        input: Arc::new(windows::WinInput),
        hotkey: Arc::new(windows::WinHotkey),
        accessibility: Arc::new(windows::WinAccessibility),
    }
}

#[cfg(target_os = "macos")]
pub fn permission_report() -> Vec<PermissionStatus> {
    macos::permission_report()
}

#[cfg(windows)]
pub fn permission_report() -> Vec<PermissionStatus> {
    windows::permission_report()
}
