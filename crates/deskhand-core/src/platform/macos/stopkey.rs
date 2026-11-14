//! The global Esc emergency stop: an active session-level event tap on
//! Escape key-down/up. Armed only while Claude controls input. A real Esc is
//! consumed (returned NULL), so no app sees it; Deskhand's own synthetic Esc
//! (the `key` tool, stamped with `INPUT_MARKER`) passes through untouched.
//!
//! `on_press` runs on the tap thread and must return fast. It must never drop
//! the `ListenerHandle` itself: that joins the tap thread and would deadlock.

use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};

use objc2_core_foundation::CFMachPort;
use objc2_core_graphics::{CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapProxy, CGEventType};

use super::accessibility::ax_is_trusted;
use super::hotkey::{mask_of, run_tap};
use super::input::INPUT_MARKER;
use crate::platform::{ListenerHandle, Permission, Result, StopKey};

pub const KVK_ESCAPE: i64 = 0x35;

struct EscState {
    on_press: Arc<dyn Fn() + Send + Sync>,
    port: AtomicPtr<CFMachPort>,
}

unsafe extern "C-unwind" fn esc_callback(
    _proxy: CGEventTapProxy,
    etype: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user` is the Box<EscState> owned by the tap thread (see run_tap).
    let state = unsafe { &*(user as *const EscState) };
    if etype == CGEventType::TapDisabledByTimeout || etype == CGEventType::TapDisabledByUserInput {
        let port = state.port.load(Ordering::Acquire);
        if !port.is_null() {
            CGEvent::tap_enable(unsafe { &*port }, true);
        }
        return event.as_ptr();
    }
    let ev = unsafe { event.as_ref() };
    if CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode) != KVK_ESCAPE {
        return event.as_ptr();
    }
    if CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData) == INPUT_MARKER {
        return event.as_ptr(); // our own `key esc`
    }
    if etype == CGEventType::KeyDown
        && CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventAutorepeat) == 0
    {
        (state.on_press)();
    }
    null_mut()
}

pub struct MacStopKey;

impl StopKey for MacStopKey {
    fn start(&self, on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle> {
        let missing = if ax_is_trusted() { Permission::InputMonitoring } else { Permission::Accessibility };
        run_tap(
            CGEventTapLocation::SessionEventTap,
            CGEventTapOptions::Default,
            mask_of(&[CGEventType::KeyDown, CGEventType::KeyUp]),
            esc_callback,
            Box::new(EscState { on_press, port: AtomicPtr::new(null_mut()) }),
            |s: &EscState| Some(&s.port),
            missing,
        )
    }
}

struct WatchEsc {
    tx: std::sync::mpsc::Sender<bool>,
}

unsafe extern "C-unwind" fn watch_esc_callback(
    _proxy: CGEventTapProxy,
    etype: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &*(user as *const WatchEsc) };
    let ev = unsafe { event.as_ref() };
    if (etype == CGEventType::KeyDown || etype == CGEventType::KeyUp)
        && CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode) == KVK_ESCAPE
    {
        let _ = state.tx.send(etype == CGEventType::KeyDown);
    }
    event.as_ptr()
}

/// Self-test helper: a listen-only tap *after* the session taps (where apps
/// get their events). Sends `true`/`false` for every Esc down/up that got
/// past the stop key. While the stop key is armed it must see nothing.
pub fn watch_escape(tx: std::sync::mpsc::Sender<bool>) -> Result<ListenerHandle> {
    run_tap(
        CGEventTapLocation::AnnotatedSessionEventTap,
        CGEventTapOptions::ListenOnly,
        mask_of(&[CGEventType::KeyDown, CGEventType::KeyUp]),
        watch_esc_callback,
        Box::new(WatchEsc { tx }),
        |_: &WatchEsc| None,
        Permission::InputMonitoring,
    )
}

/// Post a real-looking (unmarked) Esc key press at the HID level, for the
/// self-test of the stop key. Uses a private event source.
pub fn post_test_escape() -> Result<()> {
    use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(source.as_deref(), KVK_ESCAPE as u16, down)
            .ok_or_else(|| crate::platform::PlatformError::Other("CGEventCreateKeyboardEvent failed".into()))?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(())
}
