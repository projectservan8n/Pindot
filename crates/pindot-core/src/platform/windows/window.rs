//! Window chores for Pindot's own windows.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, IsWindowVisible, SW_HIDE, SW_SHOWNA, SetWindowLongPtrW, ShowWindow,
};

use crate::platform::windows_logic::overlay_ex_style;

/// Keep an overlay window out of the Alt+Tab list and the taskbar: make it
/// a tool window. `hwnd` is the window handle as a number.
///
/// The shell reads the style when a window is shown, so a visible window
/// is hidden for the change and shown again without activating it.
pub fn hide_from_switcher(hwnd: isize) {
    let h = hwnd as HWND;
    // SAFETY: plain window calls; a handle that is gone just fails them.
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let want = overlay_ex_style(ex);
        if want == ex {
            return;
        }
        let visible = IsWindowVisible(h) != 0;
        if visible {
            ShowWindow(h, SW_HIDE);
        }
        SetWindowLongPtrW(h, GWL_EXSTYLE, want as isize);
        if visible {
            ShowWindow(h, SW_SHOWNA);
        }
    }
}
