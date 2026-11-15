//! Synthetic mouse and keyboard input through `SendInput`.
//!
//! Every event carries `INPUT_MARKER` in `dwExtraInfo`, so Pindot's own
//! keyboard hooks ignore it: Claude pressing a shortcut never counts as
//! hold-to-talk, and Claude's `esc` is not the user's emergency stop.
//!
//! Points arrive in global logical points and are turned into physical
//! pixels with the scale of the display they are on. The pointer is moved
//! with absolute coordinates across the virtual desktop (all monitors).
//!
//! `cmd` in a shortcut is pressed as Ctrl (cmd+s on a Mac is ctrl+s here);
//! `win` is the Windows key. Text is typed as Unicode, so it does not depend
//! on the keyboard layout; a shortcut's letter does (`VkKeyScanW`).
//!
//! Limit: Windows drops input that a normal process sends to an elevated
//! (administrator) window, and `SendInput` then still reports success.

use std::thread;
use std::time::Duration;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VkKeyScanW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WHEEL_DELTA,
};

use super::capture::{Monitor, monitors};
use super::keyhook::INPUT_MARKER;
use crate::platform::windows_logic::{VK_LWIN, VK_MENU, VK_SHIFT, modifier_vks, mouse_abs, to_physical, vk_for_key};
use crate::platform::{DisplayInfo, Input, KeyCombo, KeyName, MouseButton, PlatformError, Point, Result, drag_steps};

const VK_RETURN: u16 = 0x0D;
const VK_TAB: u16 = 0x09;

fn mouse(flags: u32, data: i32, dx: i32, dy: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: INPUT_MARKER },
        },
    }
}

fn key(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INPUT_MARKER },
        },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: a valid array of INPUT structures and their size.
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(PlatformError::Other(format!(
            "Windows took only {sent} of {} input events: {}",
            inputs.len(),
            std::io::Error::last_os_error()
        )))
    }
}

fn move_abs(at: Point) -> Result<()> {
    let displays: Vec<DisplayInfo> = monitors().iter().map(Monitor::info).collect();
    let (x, y) = to_physical(&displays, at);
    // SAFETY: plain metric queries.
    let virt = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let (ax, ay) = mouse_abs(x, y, virt);
    send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, 0, ax, ay)])
}

fn button_flags(b: MouseButton) -> (u32, u32) {
    match b {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    }
}

/// Is `vk` one of the keys Windows wants flagged as "extended"?
fn key_flags(vk: u16, extended: bool, up: bool) -> u32 {
    let ext = extended || u32::from(vk) == VK_LWIN;
    (if ext { KEYEVENTF_EXTENDEDKEY } else { 0 }) | (if up { KEYEVENTF_KEYUP } else { 0 })
}

/// The virtual key that types `c` on the current keyboard layout. Adds
/// Shift to `mods` when the character needs it ('+' on a US keyboard).
fn char_vk(c: char, mods: &mut Vec<u16>) -> Result<u16> {
    let mut units = [0u16; 2];
    let encoded = c.encode_utf16(&mut units);
    if encoded.len() != 1 {
        return Err(PlatformError::Other(format!("{c:?} is not a key")));
    }
    // SAFETY: a plain lookup in the current keyboard layout.
    let found = unsafe { VkKeyScanW(encoded[0]) };
    if found == -1 {
        return Err(PlatformError::Other(format!("{c:?} is not a key on this keyboard layout")));
    }
    let (vk, state) = ((found & 0xFF) as u16, (found >> 8) & 0xFF);
    if state & 0b110 != 0 {
        return Err(PlatformError::Other(format!("{c:?} needs AltGr on this keyboard layout; type it with type_text instead")));
    }
    if state & 1 != 0 && !mods.contains(&(VK_SHIFT as u16)) {
        mods.push(VK_SHIFT as u16);
    }
    Ok(vk)
}

/// One tap of Alt, marked as ours. Windows lets a background program bring
/// a window to the front only right after input.
pub(super) fn tap_alt() {
    let _ = send(&[key(VK_MENU as u16, 0, 0), key(VK_MENU as u16, 0, KEYEVENTF_KEYUP)]);
}

pub struct WinInput;

impl Input for WinInput {
    fn pointer(&self) -> Option<Point> {
        super::permissions::pointer_location()
    }

    fn click(&self, at: Point, b: MouseButton, count: u32) -> Result<()> {
        move_abs(at)?;
        thread::sleep(Duration::from_millis(40));
        let (down, up) = button_flags(b);
        for i in 0..count.clamp(1, 3) {
            if i > 0 {
                thread::sleep(Duration::from_millis(30));
            }
            send(&[mouse(down, 0, 0, 0), mouse(up, 0, 0, 0)])?;
        }
        Ok(())
    }

    fn move_to(&self, at: Point) -> Result<()> {
        move_abs(at)
    }

    fn type_text(&self, text: &str) -> Result<()> {
        let mut batch: Vec<INPUT> = Vec::new();
        let mut prev = '\0';
        for c in text.chars() {
            match c {
                // "\r\n" is one line break.
                '\n' if prev == '\r' => {}
                '\n' | '\r' => batch.extend([key(VK_RETURN, 0, 0), key(VK_RETURN, 0, KEYEVENTF_KEYUP)]),
                '\t' => batch.extend([key(VK_TAB, 0, 0), key(VK_TAB, 0, KEYEVENTF_KEYUP)]),
                c => {
                    let mut units = [0u16; 2];
                    for &u in c.encode_utf16(&mut units).iter() {
                        batch.push(key(0, u, KEYEVENTF_UNICODE));
                        batch.push(key(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                    }
                }
            }
            prev = c;
            // Short bursts: some apps drop keys from one long burst.
            if batch.len() >= 40 {
                send(&batch)?;
                batch.clear();
                thread::sleep(Duration::from_millis(8));
            }
        }
        send(&batch)
    }

    fn key(&self, combo: &KeyCombo) -> Result<()> {
        let mut mods = modifier_vks(combo.mods);
        let (vk, extended) = match (vk_for_key(combo.key), combo.key) {
            (Some(named), _) => named,
            (None, KeyName::Char(c)) => (char_vk(c, &mut mods)?, false),
            (None, other) => return Err(PlatformError::Other(format!("no key for {other:?}"))),
        };
        let downs: Vec<INPUT> = mods.iter().map(|&m| key(m, 0, key_flags(m, false, false))).collect();
        send(&downs)?;
        let res = send(&[key(vk, 0, key_flags(vk, extended, false)), key(vk, 0, key_flags(vk, extended, true))]);
        // Always release the modifiers, even if the key failed.
        let ups: Vec<INPUT> = mods.iter().rev().map(|&m| key(m, 0, key_flags(m, false, true))).collect();
        let _ = send(&ups);
        res
    }

    fn scroll(&self, at: Point, dx: i32, dy: i32) -> Result<()> {
        move_abs(at)?;
        thread::sleep(Duration::from_millis(30));
        let notch = WHEEL_DELTA as i32;
        if dy != 0 {
            // A positive wheel value scrolls up; `dy > 0` means down.
            send(&[mouse(MOUSEEVENTF_WHEEL, -dy * notch, 0, 0)])?;
        }
        if dx != 0 {
            send(&[mouse(MOUSEEVENTF_HWHEEL, dx * notch, 0, 0)])?;
        }
        Ok(())
    }

    fn drag(&self, from: Point, to: Point, should_stop: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        move_abs(from)?;
        thread::sleep(Duration::from_millis(60));
        send(&[mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0)])?;
        thread::sleep(Duration::from_millis(80));
        let steps = drag_steps(from, to);
        let mut result = Ok(());
        for i in 1..=steps {
            if should_stop() {
                result = Err(PlatformError::Other("stopped by the user (Esc)".into()));
                break;
            }
            let t = i as f64 / steps as f64;
            let p = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            if let Err(e) = move_abs(p) {
                result = Err(e);
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        thread::sleep(Duration::from_millis(60));
        // Always let go of the button.
        send(&[mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0)])?;
        result
    }
}
