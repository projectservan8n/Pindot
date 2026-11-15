//! Hold-to-talk through a CGEventTap: Fn (the default) or any chord of
//! modifier keys (Right Option, Ctrl+Cmd, ...).
//!
//! An active (filtering) tap at the session level sees `flagsChanged`,
//! `keyDown` and `keyUp`. Each `flagsChanged` carries the full modifier
//! state, left and right separately (device bits), which feeds the pure
//! `ChordTracker` (exact match, cancel on any other key). Fn is tracked from
//! its own key events (keycode 0x3F, `MaskSecondaryFn`).
//!
//! Swallowing: only Fn. When the binding uses Fn we return NULL for the Fn
//! events, and for the Dictation (0xB0) and Globe (0xB3) keys some keyboards
//! send while Fn is held, so the Globe action does not fire. Other
//! modifiers pass through: holding them alone does nothing in apps,
//! swallowing a modifier does not remove it from the flags of later key
//! events anyway, and it would break that key for typing.
//!
//! Events Pindot itself posts (the `key` tool, stamped `INPUT_MARKER`) are
//! ignored, so Claude pressing a shortcut never counts as a hold.
//!
//! Limits (see CLAUDE.md): Fn only exists on Apple keyboards (a non-Apple
//! keyboard's Fn never reaches macOS, hence the other chords), and a physical
//! test is still needed to prove the Globe action is suppressed, because the
//! OS may act on it below the session tap.

use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use objc2_core_foundation::{CFMachPort, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::accessibility::ax_is_trusted;
use super::input::INPUT_MARKER;
use crate::platform::{
    ChordTracker, HeldMods, Hotkey, HotkeyBinding, HotkeyEvent, ListenerHandle, ModKey, Permission, PlatformError,
    Result, Side,
};

pub const KVK_FUNCTION: i64 = 0x3F;
pub const KVK_DICTATION: i64 = 0xB0;
pub const KVK_GLOBE: i64 = 0xB3;
pub const KVK_LEFT_CONTROL: u16 = 0x3B;
pub const KVK_RIGHT_CONTROL: u16 = 0x3E;
pub const KVK_LEFT_SHIFT: u16 = 0x38;
pub const KVK_RIGHT_SHIFT: u16 = 0x3C;
pub const KVK_LEFT_COMMAND: u16 = 0x37;
pub const KVK_RIGHT_COMMAND: u16 = 0x36;
pub const KVK_LEFT_OPTION: u16 = 0x3A;
pub const KVK_RIGHT_OPTION: u16 = 0x3D;

/// Marker in `kCGEventSourceUserData` for the synthetic hotkey events the
/// self-tests post, so the watcher can tell them apart from a real keyboard.
pub const SYNTHETIC_FN_MARKER: i64 = 0x0DE5_0F11;

/// The modifier a macOS virtual keycode belongs to.
pub fn mac_keycode_mod(keycode: i64) -> Option<(ModKey, Side)> {
    Some(match keycode as u16 {
        KVK_LEFT_CONTROL => (ModKey::Control, Side::Left),
        KVK_RIGHT_CONTROL => (ModKey::Control, Side::Right),
        KVK_LEFT_SHIFT => (ModKey::Shift, Side::Left),
        KVK_RIGHT_SHIFT => (ModKey::Shift, Side::Right),
        KVK_LEFT_COMMAND => (ModKey::Command, Side::Left),
        KVK_RIGHT_COMMAND => (ModKey::Command, Side::Right),
        KVK_LEFT_OPTION => (ModKey::Option, Side::Left),
        KVK_RIGHT_OPTION => (ModKey::Option, Side::Right),
        0x3F => (ModKey::Fn, Side::Either),
        _ => return None,
    })
}

/// The device-dependent flag bit (plus the generic bit) for one modifier key,
/// as a real keyboard sets them. Used to build synthetic test events.
pub fn mac_flags_for(key: ModKey, side: Side) -> u64 {
    match (key, side) {
        (ModKey::Control, Side::Right) => 0x40000 | 0x2000,
        (ModKey::Control, _) => 0x40000 | 0x1,
        (ModKey::Shift, Side::Right) => 0x20000 | 0x4,
        (ModKey::Shift, _) => 0x20000 | 0x2,
        (ModKey::Command, Side::Right) => 0x100000 | 0x10,
        (ModKey::Command, _) => 0x100000 | 0x8,
        (ModKey::Option, Side::Right) => 0x80000 | 0x40,
        (ModKey::Option, _) => 0x80000 | 0x20,
        (ModKey::Fn, _) => 0x800000,
    }
}

struct Chord {
    tracker: ChordTracker,
    fn_down: bool,
}

struct TapState {
    tx: mpsc::Sender<HotkeyEvent>,
    port: AtomicPtr<CFMachPort>,
    chord: Mutex<Chord>,
    /// Swallow Fn (and Globe/Dictation while Fn is down). Only when the
    /// binding uses Fn.
    swallow_fn: bool,
}

pub(super) fn mask_of(types: &[CGEventType]) -> CGEventMask {
    types.iter().fold(0, |m, t| m | (1u64 << t.0))
}

unsafe extern "C-unwind" fn chord_tap_callback(
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
    if CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData) == INPUT_MARKER {
        return event.as_ptr();
    }
    let keycode = CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode);
    let flags = CGEvent::flags(Some(ev));
    let has_fn = flags.contains(CGEventFlags::MaskSecondaryFn);
    let Ok(mut chord) = state.chord.lock() else {
        return event.as_ptr();
    };

    if etype == CGEventType::FlagsChanged {
        if keycode == KVK_FUNCTION {
            chord.fn_down = has_fn;
        }
        let mut held = HeldMods::from_mac_flags(flags.0);
        held.fn_key = chord.fn_down;
        if let Some(e) = chord.tracker.on_mods(held) {
            let _ = state.tx.send(e);
        }
        if keycode == KVK_FUNCTION && state.swallow_fn {
            return null_mut();
        }
    } else if (etype == CGEventType::KeyDown || etype == CGEventType::KeyUp)
        && (keycode == KVK_DICTATION || keycode == KVK_GLOBE)
    {
        if has_fn && state.swallow_fn {
            return null_mut();
        }
    } else if etype == CGEventType::KeyDown
        && let Some(e) = chord.tracker.on_other_key()
    {
        let _ = state.tx.send(e);
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
        let flags = CGEvent::flags(Some(ev)).0;
        let down = match mac_keycode_mod(keycode) {
            Some((key, side)) => {
                let want = mac_flags_for(key, side);
                flags & want == want
            }
            None => false,
        };
        let marker = CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData);
        let _ = state.tx.send(WatchedFn { keycode, down, synthetic: marker == SYNTHETIC_FN_MARKER });
    }
    event.as_ptr()
}

/// A modifier transition seen by the listen-only watcher tap.
#[derive(Debug, Clone, Copy)]
pub struct WatchedFn {
    pub keycode: i64,
    pub down: bool,
    pub synthetic: bool,
}

struct WatchState {
    tx: mpsc::Sender<WatchedFn>,
}

/// Run a tap on its own thread with its own CFRunLoop. The returned handle
/// disables and invalidates the tap and joins the thread.
pub(super) fn run_tap<S: Send + 'static>(
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
        .name("pindot-event-tap".into())
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
        self.start_chord(HotkeyBinding::fn_key(), tx, swallow)
    }

    /// Start a tap for any modifier chord. `swallow_fn` applies only when the
    /// binding uses Fn.
    pub fn start_chord(
        &self,
        binding: HotkeyBinding,
        tx: mpsc::Sender<HotkeyEvent>,
        swallow_fn: bool,
    ) -> Result<ListenerHandle> {
        let swallow_fn = swallow_fn && binding.has_fn();
        let state = Box::new(TapState {
            tx,
            port: AtomicPtr::new(null_mut()),
            chord: Mutex::new(Chord { tracker: ChordTracker::new(binding), fn_down: false }),
            swallow_fn,
        });
        // An active (filtering) keyboard tap needs Accessibility; a listen-only
        // one needs Input Monitoring. Report whichever is actually missing.
        let missing = if ax_is_trusted() { Permission::InputMonitoring } else { Permission::Accessibility };
        run_tap(
            CGEventTapLocation::SessionEventTap,
            if swallow_fn { CGEventTapOptions::Default } else { CGEventTapOptions::ListenOnly },
            mask_of(&[CGEventType::FlagsChanged, CGEventType::KeyDown, CGEventType::KeyUp]),
            chord_tap_callback,
            state,
            |s: &TapState| Some(&s.port),
            missing,
        )
    }
}

impl Hotkey for MacHotkey {
    fn start(&self, binding: HotkeyBinding, events: mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        if let Err(e) = binding.check_for(crate::platform::Os::MacOs) {
            return Err(PlatformError::Other(e.to_string()));
        }
        self.start_chord(binding, events, true)
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
    post_synthetic_flags(KVK_FUNCTION as u16, if down { mac_flags_for(ModKey::Fn, Side::Either) } else { 0 })
}

/// Post one synthetic `flagsChanged` for `keycode` with the full modifier
/// state `flags` (generic + device bits, see [`mac_flags_for`]), marked with
/// `SYNTHETIC_FN_MARKER`, at the HID level from a private source state.
/// Callers must always post the matching release (flags 0) afterwards.
pub fn post_synthetic_flags(keycode: u16, flags: u64) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    let ev = CGEvent::new_keyboard_event(source.as_deref(), keycode, flags != 0)
        .ok_or_else(|| PlatformError::Other("CGEventCreateKeyboardEvent failed".into()))?;
    CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
    CGEvent::set_flags(Some(&ev), CGEventFlags(flags) | CGEventFlags::MaskNonCoalesced);
    CGEvent::set_integer_value_field(Some(&ev), CGEventField::EventSourceUserData, SYNTHETIC_FN_MARKER);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    Ok(())
}

/// Post one synthetic key down or up (e.g. F13) with modifier `flags`, marked
/// with `SYNTHETIC_FN_MARKER`. Only for self-tests with our own test window
/// frontmost: unlike Fn, the key reaches the frontmost app.
pub fn post_synthetic_key(keycode: u16, down: bool, flags: u64) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    let ev = CGEvent::new_keyboard_event(source.as_deref(), keycode, down)
        .ok_or_else(|| PlatformError::Other("CGEventCreateKeyboardEvent failed".into()))?;
    CGEvent::set_flags(Some(&ev), CGEventFlags(flags) | CGEventFlags::MaskNonCoalesced);
    CGEvent::set_integer_value_field(Some(&ev), CGEventField::EventSourceUserData, SYNTHETIC_FN_MARKER);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    Ok(())
}
