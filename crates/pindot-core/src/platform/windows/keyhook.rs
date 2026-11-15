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
//! Windows removes a low-level hook it finds too slow, without telling the
//! app, and a hook never sees keys typed on the lock screen, a UAC prompt or
//! into an elevated (administrator) window. So every `TICK_MS` the hook
//! thread corrects its state from the keys that are really down
//! (`GetAsyncKeyState`) and installs the hook again; it logs when Windows
//! had removed it. Hold-to-talk therefore still works with a dead hook,
//! half a second late, and heals. The handlers only update a little state
//! and send on a channel, so the hook itself stays fast.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, KillTimer, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT,
    WM_SYSKEYDOWN, WM_TIMER,
};

use crate::platform::windows_logic::{
    EscFilter, KeyEvent, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MASK, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN, WinChord,
};
use crate::platform::{
    HeldMods, Hotkey, HotkeyBinding, HotkeyEvent, ListenerHandle, Os, PlatformError, Result, StopKey,
};

/// `dwExtraInfo` of every key Pindot injects ("PIND").
pub const INPUT_MARKER: usize = 0x5049_4E44;

const HC_ACTION: i32 = 0;
/// How often the hook thread checks itself (see the module comment).
const TICK_MS: u32 = 500;
/// After a key, the real key state may not have settled yet: leave the
/// tracked state alone this long.
const KEY_QUIET: Duration = Duration::from_millis(150);

/// What the hook does with one key.
#[derive(Default)]
struct Verdict {
    swallow: bool,
    mask: bool,
}

/// What one listener does with keys, and on the timer.
trait HookLogic: Send {
    fn key(&mut self, e: KeyEvent) -> Verdict;
    /// Every `TICK_MS`: correct the state from the keys that are really down.
    fn tick(&mut self) -> Verdict {
        Verdict::default()
    }
    /// True while the hook must not be taken out and put back (a key-up
    /// could be missed in between).
    fn busy(&self) -> bool {
        false
    }
}

thread_local! {
    /// The logic of the hook this thread installed. A low-level hook has
    /// no user-data pointer, but it always runs on the installing thread.
    static HANDLER: RefCell<Option<Box<dyn HookLogic>>> = const { RefCell::new(None) };
}

/// The modifier keys that are down now, whatever the hook saw.
fn real_mods() -> HeldMods {
    // SAFETY: a plain query; the top bit is "down".
    let down = |vk: u32| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    HeldMods {
        l_ctrl: down(VK_LCONTROL),
        r_ctrl: down(VK_RCONTROL),
        l_opt: down(VK_LMENU),
        r_opt: down(VK_RMENU),
        l_shift: down(VK_LSHIFT),
        r_shift: down(VK_RSHIFT),
        l_cmd: down(VK_LWIN),
        r_cmd: down(VK_RWIN),
        fn_key: false,
    }
}

fn install_hook() -> HHOOK {
    // SAFETY: `hook_proc` matches HOOKPROC and lives as long as the process.
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(null()), 0) }
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
        let verdict = HANDLER.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.as_mut().map(|l| l.key(event))));
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
fn run_hook(what: &'static str, handler: Box<dyn HookLogic>) -> Result<ListenerHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<u32, String>>();
    let join = thread::Builder::new()
        .name("pindot-key-hook".into())
        .spawn(move || {
            // SAFETY: plain Win32 calls on this thread; `msg` is a valid MSG.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // Give this thread a message queue before anyone posts to it.
            unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE) };
            HANDLER.with(|h| *h.borrow_mut() = Some(handler));
            let mut hook = install_hook();
            if hook.is_null() {
                let err = std::io::Error::last_os_error();
                HANDLER.with(|h| *h.borrow_mut() = None);
                let _ = ready_tx.send(Err(format!("the keyboard hook could not be installed: {err}")));
                return;
            }
            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
            // A thread timer: WM_TIMER arrives in this thread's queue.
            let timer = unsafe { SetTimer(null_mut(), 0, TICK_MS, None) };
            // Windows calls `hook_proc` from inside GetMessageW. WM_QUIT
            // (from the handle) ends the loop; -1 is an error.
            while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
                if msg.message != WM_TIMER {
                    continue;
                }
                let (verdict, busy) = HANDLER.with(|h| match h.borrow_mut().as_mut() {
                    Some(l) => (l.tick(), l.busy()),
                    None => (Verdict::default(), true),
                });
                if verdict.mask {
                    press_mask_key();
                }
                if busy {
                    continue;
                }
                // Take the hook out and put it back. If taking it out fails,
                // Windows had already removed it: say so, it explains a
                // hold-to-talk that went quiet.
                let was_installed = unsafe { UnhookWindowsHookEx(hook) } != 0;
                let fresh = install_hook();
                if fresh.is_null() {
                    tracing::warn!("the {what} keyboard hook could not be reinstalled: {}", std::io::Error::last_os_error());
                    if was_installed {
                        continue; // `hook` is gone; try again on the next tick
                    }
                } else if !was_installed {
                    tracing::warn!("Windows had removed the {what} keyboard hook; it is installed again");
                }
                hook = fresh;
            }
            unsafe {
                KillTimer(null_mut(), timer);
                if !hook.is_null() {
                    UnhookWindowsHookEx(hook);
                }
            }
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

struct HotkeyLogic {
    chord: WinChord,
    events: mpsc::Sender<HotkeyEvent>,
    last_key: Instant,
}

impl HotkeyLogic {
    fn report(&self, out: crate::platform::windows_logic::ChordOutcome) -> Verdict {
        if let Some(event) = out.event {
            let _ = self.events.send(event);
        }
        Verdict { swallow: false, mask: out.mask }
    }
}

impl HookLogic for HotkeyLogic {
    fn key(&mut self, e: KeyEvent) -> Verdict {
        self.last_key = Instant::now();
        let out = self.chord.on_key(e);
        self.report(out)
    }

    fn tick(&mut self) -> Verdict {
        if self.last_key.elapsed() < KEY_QUIET {
            return Verdict::default();
        }
        let out = self.chord.resync(real_mods());
        if let Some(event) = out.event {
            // The hook did not report this change: it is dead or was blind.
            tracing::warn!("hold-to-talk {event:?} came from the key state, not from the keyboard hook");
        }
        self.report(out)
    }

    fn busy(&self) -> bool {
        self.chord.is_active() || self.last_key.elapsed() < KEY_QUIET
    }
}

pub struct WinHotkey;

impl Hotkey for WinHotkey {
    fn start(&self, binding: HotkeyBinding, events: mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        if let Err(e) = binding.check_for(Os::Windows) {
            return Err(PlatformError::Other(e.to_string()));
        }
        let logic = HotkeyLogic { chord: WinChord::new(binding), events, last_key: Instant::now() };
        run_hook("hold-to-talk", Box::new(logic))
    }
}

struct EscLogic {
    esc: EscFilter,
    on_press: Arc<dyn Fn() + Send + Sync>,
}

impl HookLogic for EscLogic {
    fn key(&mut self, e: KeyEvent) -> Verdict {
        let out = self.esc.on_key(e);
        if out.pressed {
            (self.on_press)();
        }
        Verdict { swallow: out.swallow, mask: false }
    }
}

pub struct WinStopKey;

impl StopKey for WinStopKey {
    /// `on_press` runs on the hook thread and must return fast. It must never
    /// drop the `ListenerHandle` itself: that joins this thread.
    fn start(&self, on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle> {
        run_hook("Esc stop", Box::new(EscLogic { esc: EscFilter::default(), on_press }))
    }
}
