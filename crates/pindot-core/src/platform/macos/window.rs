//! Small NSWindow helpers for Pindot's own windows.

use std::ffi::c_void;

use objc2::msg_send;
use objc2::runtime::AnyObject;

/// Put one of Pindot's own windows above other apps' windows without
/// activating Pindot, so the keyboard stays with the app the user is in
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

/// Bring Pindot forward with the keyboard on `ns_window`: only for a window
/// the user asked for (a tray menu item). Pindot runs with the Accessory
/// policy and never activates by itself, and a key window of an inactive
/// app stays behind the app the user is in, which is why the tray items
/// seemed to need a second click.
///
/// # Safety
/// As for [`order_front_without_focus`].
pub unsafe fn activate_and_focus(ns_window: *mut c_void) {
    let Some(w) = (unsafe { (ns_window as *mut AnyObject).as_ref() }) else { return };
    let app: *mut AnyObject = unsafe { msg_send![objc2::class!(NSApplication), sharedApplication] };
    if let Some(app) = unsafe { app.as_ref() } {
        // macOS 14+: `activate`; older: `activateIgnoringOtherApps:`.
        let modern: bool = unsafe { msg_send![app, respondsToSelector: objc2::sel!(activate)] };
        if modern {
            let _: () = unsafe { msg_send![app, activate] };
        } else {
            let _: () = unsafe { msg_send![app, activateIgnoringOtherApps: true] };
        }
    }
    let _: () = unsafe { msg_send![w, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()] };
}

/// Give the keyboard back to the app the user was in, once Pindot's last
/// normal window closes (the overlay panels never take it). Does not hide
/// the overlay, unlike `hide:`.
///
/// # Safety
/// Must run on the main thread.
pub unsafe fn deactivate_app() {
    let app: *mut AnyObject = unsafe { msg_send![objc2::class!(NSApplication), sharedApplication] };
    if let Some(app) = unsafe { app.as_ref() } {
        let active: bool = unsafe { msg_send![app, isActive] };
        if active {
            let _: () = unsafe { msg_send![app, deactivate] };
        }
    }
}
