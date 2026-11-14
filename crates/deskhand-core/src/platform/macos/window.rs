//! Small NSWindow helpers for Deskhand's own windows.

use std::ffi::c_void;

use objc2::msg_send;
use objc2::runtime::AnyObject;

/// Put one of Deskhand's own windows above other apps' windows without
/// activating Deskhand, so the keyboard stays with the app the user is in
/// (used when Setup opens by itself at launch).
///
/// # Safety
/// `ns_window` must be a live `NSWindow` pointer (from Tauri's
/// `ns_window()`), and this must run on the main thread.
pub unsafe fn order_front_without_focus(ns_window: *mut c_void) {
    let Some(w) = (unsafe { (ns_window as *mut AnyObject).as_ref() }) else { return };
    let _: () = unsafe { msg_send![w, orderFrontRegardless] };
}

/// The window's CGWindowID (`screencapture -l` takes it), or 0.
///
/// # Safety
/// As for [`order_front_without_focus`].
pub unsafe fn window_number(ns_window: *mut c_void) -> isize {
    let Some(w) = (unsafe { (ns_window as *mut AnyObject).as_ref() }) else { return 0 };
    unsafe { msg_send![w, windowNumber] }
}
