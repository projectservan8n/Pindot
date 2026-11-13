//! Windows implementations. Phase 0 ships stubs only; see
//! `docs/windows-test-checklist.md` for what each must do and how to test it.
//!
//! Planned: hotkey = low-level keyboard hook (WH_KEYBOARD_LL) for Ctrl+Win,
//! swallowing the Win key-up so the Start menu does not open; capture = xcap
//! (Windows Graphics Capture); input = enigo (SendInput) in physical pixels
//! with per-monitor DPI awareness v2; accessibility = UI Automation.

use super::*;

pub struct WinCapture;
pub struct WinInput;
pub struct WinHotkey;
pub struct WinAccessibility;

impl Capture for WinCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Err(PlatformError::Unsupported("Windows capture (Phase 2)"))
    }
    fn capture_display(&self, _display_id: u32) -> Result<CapturedFrame> {
        Err(PlatformError::Unsupported("Windows capture (Phase 2)"))
    }
}

impl Input for WinInput {
    fn click(&self, _at: Point, _button: MouseButton, _count: u32) -> Result<()> {
        Err(PlatformError::Unsupported("Windows input (Phase 2)"))
    }
}

impl Hotkey for WinHotkey {
    fn start(&self, _binding: HotkeyBinding, _events: std::sync::mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        Err(PlatformError::Unsupported("Windows Ctrl+Win hotkey (Phase 2)"))
    }
}

impl Accessibility for WinAccessibility {
    fn is_trusted(&self) -> bool {
        // Windows has no per-app accessibility permission.
        true
    }
}

pub fn permission_report() -> Vec<PermissionStatus> {
    // Windows only prompts for the microphone; the rest needs no grant.
    vec![]
}
