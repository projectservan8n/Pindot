//! Hold-to-talk and the Esc emergency stop through a low-level keyboard hook
//! (`WH_KEYBOARD_LL`).
//!
//! Each listener runs its own hook on its own thread, which Windows calls
//! from that thread's message loop. What to do with a key is decided by the
//! pure logic in `platform::windows_logic`; this file only installs the
//! hook, hands it each key and carries out the verdict.
//!
//! Hold-to-talk swallows nothing. When a chord with Win or Alt starts, it
//! presses an unassigned key (`VK_MASK`) once, so Windows sees a shortcut
//! and letting go opens neither the Start menu nor a window's menu bar.
//! The Esc stop swallows a real Esc (the hook returns 1, no app sees it).
//!
//! Keys Pindot injects carry `INPUT_MARKER` in `dwExtraInfo` and are
//! ignored by both.
//!
//! Limits: a hook in a normal process does not see keys typed into an
//! elevated (administrator) window, and Windows drops a hook whose callback
//! takes longer than the system's hook timeout, so the handlers only update
//! a little state and send on a channel.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::{Arc, mpsc};
use std::thread;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::platform::windows_logic::{EscFilter, KeyEvent, VK_MASK, WinChord};
use crate::platform::{Hotkey, HotkeyBinding, HotkeyEvent, ListenerHandle, Os, PlatformError, Result, StopKey};

/// `dwExtraInfo` of every key Pindot injects ("PIND").
pub const INPUT_MARKER: usize = 0x5049_4E44;

const HC_ACTION: i32 = 0;

/// What the hook does with one key.
struct Verdict {
    swallow: bool,
    mask: bool,
}

type Handler = Box<dyn FnMut(KeyEvent) -> Verdict + Send>;

thread_local! {
    /// The handler of the hook this thread installed. A low-level hook has
    /// no user-data pointer, but it always runs on the installing thread.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT that is
        // valid for the duration of this call.
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let event = KeyEvent {
            vk: k.vkCode,
            scan: k.scanCode,
            down: matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN),
            ours: k.flags & LLKHF_INJECTED != 0 && k.dwExtraInfo == INPUT_MARKER,
        };
        // The borrow ends before the mask key is sent: SendInput calls this
        // hook again, on this thread, before it returns.
        let verdict = HANDLER.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.as_mut().map(|f| f(event))));
        if let Some(v) = verdict {
            if v.mask {
                press_mask_key();
            }
            if v.swallow {
                return 1;
            }
        }
    }
    // SAFETY: passing the call on unchanged, as the hook contract requires.
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

fn key_input(vk: u32, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as u16,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INPUT_MARKER,
            },
        },
    }
}

/// One press and release of the unassigned mask key, marked as ours.
fn press_mask_key() {
    let inputs = [key_input(VK_MASK, false), key_input(VK_MASK, true)];
    // SAFETY: a valid array of two INPUT structures and their size.
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        tracing::warn!("the mask key could not be sent: {}", std::io::Error::last_os_error());
    }
}

/// Install a low-level keyboard hook on a new thread and pump its messages.
/// The returned handle removes the hook and joins the thread.
fn run_hook(handler: Handler) -> Result<ListenerHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<u32, String>>();
    let join = thread::Builder::new()
        .name("pindot-key-hook".into())
        .spawn(move || {
            // SAFETY: plain Win32 calls on this thread; `msg` is a valid MSG.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // Give this thread a message queue before anyone posts to it.
            unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE) };
            HANDLER.with(|h| *h.borrow_mut() = Some(handler));
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(null()), 0) };
            if hook.is_null() {
                let err = std::io::Error::last_os_error();
                HANDLER.with(|h| *h.borrow_mut() = None);
                let _ = ready_tx.send(Err(format!("the keyboard hook could not be installed: {err}")));
                return;
            }
            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
            // Windows calls `hook_proc` from inside GetMessageW. WM_QUIT
            // (from the handle) ends the loop; -1 is an error.
            while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {}
            unsafe { UnhookWindowsHookEx(hook) };
            HANDLER.with(|h| *h.borrow_mut() = None);
        })
        .map_err(|e| PlatformError::Other(e.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok(ListenerHandle::new(move || {
            // SAFETY: posting a thread message; a thread that is gone just fails.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
            let _ = join.join();
        })),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(PlatformError::Other(e))
        }
        Err(_) => Err(PlatformError::Other("the keyboard hook thread died".into())),
    }
}

pub struct WinHotkey;

impl Hotkey for WinHotkey {
    fn start(&self, binding: HotkeyBinding, events: mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        if let Err(e) = binding.check_for(Os::Windows) {
            return Err(PlatformError::Other(e.to_string()));
        }
        let mut chord = WinChord::new(binding);
        run_hook(Box::new(move |key| {
            let out = chord.on_key(key);
            if let Some(event) = out.event {
                let _ = events.send(event);
            }
            Verdict { swallow: false, mask: out.mask }
        }))
    }
}

pub struct WinStopKey;

impl StopKey for WinStopKey {
    /// `on_press` runs on the hook thread and must return fast. It must never
    /// drop the `ListenerHandle` itself: that joins this thread.
    fn start(&self, on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle> {
        let mut esc = EscFilter::default();
        run_hook(Box::new(move |key| {
            let out = esc.on_key(key);
            if out.pressed {
                on_press();
            }
            Verdict { swallow: out.swallow, mask: false }
        }))
    }
}
