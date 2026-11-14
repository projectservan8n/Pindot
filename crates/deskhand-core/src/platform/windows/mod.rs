//! Windows implementations. Phases 0 and 1 ship stubs only; see
//! `docs/windows-test-checklist.md` for what each must do and how to test it.
//!
//! Planned: hotkey = low-level keyboard hook (WH_KEYBOARD_LL) for Ctrl+Win,
//! swallowing the Win key-up so the Start menu does not open; stop key = the
//! same hook on VK_ESCAPE, ignoring events whose dwExtraInfo carries our
//! marker; capture = xcap (Windows Graphics Capture); input = enigo
//! (SendInput) in physical pixels with per-monitor DPI awareness v2;
//! accessibility = UI Automation (IUIAutomation, TreeWalker over
//! ControlView, Invoke pattern for press, ElementFromPoint for hit tests,
//! IsPassword for secure fields); windows = EnumWindows + DWM cloaking
//! check; app launch = ShellExecute / the Start-menu AppsFolder.

use super::*;

pub struct WinCapture;
pub struct WinInput;
pub struct WinHotkey;
pub struct WinAccessibility;
pub struct WinStopKey;

const LATER: &str = "Windows support is a stub until Phase 2";

impl Capture for WinCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn capture_display(&self, _display_id: u32) -> Result<CapturedFrame> {
        Err(PlatformError::Unsupported(LATER))
    }
}

impl Input for WinInput {
    fn click(&self, _at: Point, _button: MouseButton, _count: u32) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn move_to(&self, _at: Point) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn type_text(&self, _text: &str) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn key(&self, _combo: &KeyCombo) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn scroll(&self, _at: Point, _dx: i32, _dy: i32) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn drag(&self, _from: Point, _to: Point, _should_stop: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
}

impl Hotkey for WinHotkey {
    fn start(&self, _binding: HotkeyBinding, _events: std::sync::mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        Err(PlatformError::Unsupported(LATER))
    }
}

impl StopKey for WinStopKey {
    fn start(&self, _on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle> {
        Err(PlatformError::Unsupported(LATER))
    }
}

impl Accessibility for WinAccessibility {
    fn is_trusted(&self) -> bool {
        // Windows has no per-app accessibility permission.
        true
    }
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn running_apps(&self) -> Result<Vec<AppInfo>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn app_info(&self, _pid: i32) -> Option<AppInfo> {
        None
    }
    fn frontmost_pid(&self) -> Result<Option<i32>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn resolve_app(&self, _name: &str) -> Option<ResolvedApp> {
        None
    }
    fn launch_app(&self, _name: &str) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn window_tree(&self, _window: &WindowInfo, _limits: &TreeLimits) -> Result<AxTree> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn element_frame(&self, _el: &ElementHandle) -> Result<Option<Rect>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn press(&self, _el: &ElementHandle) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn element_at(&self, _at: Point) -> Result<Option<HitElement>> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn raise_window(&self, _window: &WindowInfo) -> Result<()> {
        Err(PlatformError::Unsupported(LATER))
    }
    fn focused_is_secure(&self) -> Result<bool> {
        Err(PlatformError::Unsupported(LATER))
    }
}

pub fn permission_report() -> Vec<PermissionStatus> {
    // Windows only prompts for the microphone; the rest needs no grant.
    vec![]
}
