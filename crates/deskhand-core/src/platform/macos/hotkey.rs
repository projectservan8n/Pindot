//! Hold-Fn push-to-talk through a CGEventTap.
//!
//! An active (filtering) tap at the session level sees `flagsChanged` events
//! for keycode 0x3F (kVK_Function). Fn down sets `MaskSecondaryFn`, Fn up
//! clears it. We report the transitions and, when swallowing, return NULL so
//! no app (and no later tap) receives the Fn press. We also swallow the
//! Dictation (0xB0) and Globe (0xB3) key events that some keyboards send
//! while Fn is held, so the Globe key's configured action does not fire.
//!
//! Limits (see CLAUDE.md): Fn only exists on Apple keyboards, and a physical
//! test is still needed to prove the Globe action is suppressed, because the
//! OS may act on it below the session tap.

use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use objc2_core_foundation::{CFMachPort, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::accessibility::ax_is_trusted;
use crate::platform::{
    Hotkey, HotkeyBinding, HotkeyEvent, ListenerHandle, Permission, PlatformError, Result,
};

pub const KVK_FUNCTION: i64 = 0x3F;
pub const KVK_DICTATION: i64 = 0xB0;
pub const KVK_GLOBE: i64 = 0xB3;

/// Marker in `kCGEventSourceUserData` for the synthetic Fn events the spike
/// posts, so the watcher can tell them apart from a real keyboard.
pub const SYNTHETIC_FN_MARKER: i64 = 0x0DE5_0F11;

struct TapState {
    tx: mpsc::Sender<HotkeyEvent>,
    port: AtomicPtr<CFMachPort>,
    fn_down: AtomicBool,
    swallow: bool,
}

fn mask_of(types: &[CGEventType]) -> CGEventMask {
    types.iter().fold(0, |m, t| m | (1u64 << t.0))
}

unsafe extern "C-unwind" fn fn_tap_callback(
    _proxy: CGEventTapProxy,
    etype: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user` is the Box<TapState> owned by the tap thread, which
    // outlives the tap (it is freed only after the tap is invalidated).
    let state = unsafe { &*(user as *const TapState) };

    if etype == CGEventType::TapDisabledByTimeout || etype == CGEventType::TapDisabledByUserInput {
        // macOS disables a tap whose callback is slow; turn it back on.
        let port = state.port.load(Ordering::Acquire);
        if !port.is_null() {
            CGEvent::tap_enable(unsafe { &*port }, true);
        }
        return event.as_ptr();
    }

    let ev = unsafe { event.as_ref() };
    let keycode = CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode);
    let has_fn = CGEvent::flags(Some(ev)).contains(CGEventFlags::MaskSecondaryFn);

    if etype == CGEventType::FlagsChanged && keycode == KVK_FUNCTION {
        let was_down = state.fn_down.swap(has_fn, Ordering::AcqRel);
        if was_down != has_fn {
            let _ = state.tx.send(if has_fn { HotkeyEvent::Down } else { HotkeyEvent::Up });
        }
        if state.swallow {
            return null_mut();
        }
    } else if (etype == CGEventType::KeyDown || etype == CGEventType::KeyUp)
        && (keycode == KVK_DICTATION || keycode == KVK_GLOBE)
        && has_fn
        && state.swallow
    {
        return null_mut();
    }
    event.as_ptr()
}

unsafe extern "C-unwind" fn watch_callback(
    _proxy: CGEventTapProxy,
    etype: CGEventType,
    event: NonNull<CGEvent>,
    user: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &*(user as *const WatchState) };
    if etype == CGEventType::FlagsChanged {
        let ev = unsafe { event.as_ref() };
        let keycode = CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode);
        if keycode == KVK_FUNCTION {
            let has_fn = CGEvent::flags(Some(ev)).contains(CGEventFlags::MaskSecondaryFn);
            let marker = CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData);
            let _ = state.tx.send(WatchedFn { down: has_fn, synthetic: marker == SYNTHETIC_FN_MARKER });
        }
    }
    event.as_ptr()
}

/// A Fn transition seen by the listen-only watcher tap.
#[derive(Debug, Clone, Copy)]
pub struct WatchedFn {
    pub down: bool,
    pub synthetic: bool,
}

struct WatchState {
    tx: mpsc::Sender<WatchedFn>,
}

/// Run a tap on its own thread with its own CFRunLoop. The returned handle
/// disables and invalidates the tap and joins the thread.
fn run_tap<S: Send + 'static>(
    location: CGEventTapLocation,
    options: CGEventTapOptions,
    mask: CGEventMask,
    callback: unsafe extern "C-unwind" fn(CGEventTapProxy, CGEventType, NonNull<CGEvent>, *mut c_void) -> *mut CGEvent,
    state: Box<S>,
    port_slot: impl Fn(&S) -> Option<&AtomicPtr<CFMachPort>> + Send + 'static,
    missing: Permission,
) -> Result<ListenerHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

    let join = thread::Builder::new()
        .name("deskhand-event-tap".into())
        .spawn(move || {
            let state_ptr = Box::into_raw(state);
            // SAFETY: callback matches CGEventTapCallBack; user_info stays valid
            // until after the tap is invalidated below.
            let port = unsafe {
                CGEvent::tap_create(
                    location,
                    CGEventTapPlacement::HeadInsertEventTap,
                    options,
                    mask,
                    Some(callback),
                    state_ptr as *mut c_void,
                )
            };
            let Some(port) = port else {
                let _ = ready_tx.send(Err(PlatformError::PermissionMissing(missing)));
                drop(unsafe { Box::from_raw(state_ptr) });
                return;
            };
            if let Some(slot) = port_slot(unsafe { &*state_ptr }) {
                slot.store(&*port as *const CFMachPort as *mut CFMachPort, Ordering::Release);
            }
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
                let _ = ready_tx.send(Err(PlatformError::Other("CFMachPortCreateRunLoopSource failed".into())));
                port.invalidate();
                drop(unsafe { Box::from_raw(state_ptr) });
                return;
            };
            let run_loop = CFRunLoop::current().expect("current run loop");
            let common = unsafe { kCFRunLoopCommonModes };
            run_loop.add_source(Some(&source), common);
            CGEvent::tap_enable(&port, true);
            let _ = ready_tx.send(Ok(()));

            let default_mode = unsafe { kCFRunLoopDefaultMode };
            while !stop_thread.load(Ordering::Acquire) {
                CFRunLoop::run_in_mode(default_mode, 0.2, false);
            }

            CGEvent::tap_enable(&port, false);
            run_loop.remove_source(Some(&source), common);
            port.invalidate();
            drop(source);
            drop(port);
            drop(unsafe { Box::from_raw(state_ptr) });
        })
        .map_err(|e| PlatformError::Other(e.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(ListenerHandle::new(move || {
            stop.store(true, Ordering::Release);
            let _ = join.join();
        })),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err(PlatformError::Other("event tap thread died".into())),
    }
}

pub struct MacHotkey;

impl MacHotkey {
    /// Start the Fn tap. `swallow = false` reports Fn but lets it through
    /// (used as the control run in the spike).
    pub fn start_fn(&self, tx: mpsc::Sender<HotkeyEvent>, swallow: bool) -> Result<ListenerHandle> {
        let state = Box::new(TapState { tx, port: AtomicPtr::new(null_mut()), fn_down: AtomicBool::new(false), swallow });
        // An active (filtering) keyboard tap needs Accessibility; a listen-only
        // one needs Input Monitoring. Report whichever is actually missing.
        let missing = if ax_is_trusted() { Permission::InputMonitoring } else { Permission::Accessibility };
        run_tap(
            CGEventTapLocation::SessionEventTap,
            if swallow { CGEventTapOptions::Default } else { CGEventTapOptions::ListenOnly },
            mask_of(&[CGEventType::FlagsChanged, CGEventType::KeyDown, CGEventType::KeyUp]),
            fn_tap_callback,
            state,
            |s: &TapState| Some(&s.port),
            missing,
        )
    }
}

impl Hotkey for MacHotkey {
    fn start(&self, binding: HotkeyBinding, events: mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        match binding {
            HotkeyBinding::Fn => self.start_fn(events, true),
            HotkeyBinding::CtrlWin => Err(PlatformError::Unsupported("Ctrl+Win is the Windows binding")),
        }
    }
}

/// Listen-only tap placed after the session taps (annotated session level).
/// If our filtering tap swallowed a Fn event, this one never sees it.
pub fn watch_fn(tx: mpsc::Sender<WatchedFn>) -> Result<ListenerHandle> {
    run_tap(
        CGEventTapLocation::AnnotatedSessionEventTap,
        CGEventTapOptions::ListenOnly,
        mask_of(&[CGEventType::FlagsChanged]),
        watch_callback,
        Box::new(WatchState { tx }),
        |_: &WatchState| None,
        Permission::InputMonitoring,
    )
}

/// Post one synthetic Fn transition at the HID level, marked with
/// `SYNTHETIC_FN_MARKER`. Uses a private event-source state so the system's
/// real modifier state is never left "stuck".
pub fn post_synthetic_fn(down: bool) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    let ev = CGEvent::new_keyboard_event(source.as_deref(), KVK_FUNCTION as u16, down)
        .ok_or_else(|| PlatformError::Other("CGEventCreateKeyboardEvent failed".into()))?;
    CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
    let flags = if down {
        CGEventFlags::MaskSecondaryFn | CGEventFlags::MaskNonCoalesced
    } else {
        CGEventFlags::MaskNonCoalesced
    };
    CGEvent::set_flags(Some(&ev), flags);
    CGEvent::set_integer_value_field(Some(&ev), CGEventField::EventSourceUserData, SYNTHETIC_FN_MARKER);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    Ok(())
}
