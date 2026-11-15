//! The microphone on Windows. There is no per-app grant for a desktop app
//! and Windows never prompts: three switches on Settings > Privacy &
//! security > Microphone decide whether a desktop app hears anything. An app
//! only shows up in the list under "Let desktop apps access your microphone"
//! after it has used the microphone, so Pindot is not there before its first
//! recording.
//!
//! The switches live in the consent store in the registry. Reading them
//! never prompts and needs no rights. What they mean is decided in
//! `platform::windows_logic` (tested on every OS).

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, POINT};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::platform::windows_logic::{Consent, MicSwitches};
use crate::platform::{Permission, PermissionStatus, Point};

const MICROPHONE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The `Value` word ("Allow" / "Deny") of one consent key.
fn consent(hive: HKEY, key: &str) -> Consent {
    let (key, value) = (wide(key), wide("Value"));
    let mut buf = [0u16; 32];
    let mut bytes = size_of_val(&buf) as u32;
    // SAFETY: NUL-terminated names, and a buffer with its size in bytes.
    let status = unsafe {
        RegGetValueW(
            hive,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    match status {
        ERROR_SUCCESS => {
            let len = (bytes as usize / 2).min(buf.len());
            let text = String::from_utf16_lossy(&buf[..len]);
            Consent::from_value(Some(text.trim_end_matches('\0')))
        }
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Consent::from_value(None),
        _ => Consent::Unreadable,
    }
}

/// Read-only: the three microphone switches as they are now.
pub fn microphone_switches() -> MicSwitches {
    MicSwitches {
        device: consent(HKEY_LOCAL_MACHINE, MICROPHONE),
        user: consent(HKEY_CURRENT_USER, MICROPHONE),
        desktop_apps: consent(HKEY_CURRENT_USER, &format!(r"{MICROPHONE}\NonPackaged")),
    }
}

pub fn permission_report() -> Vec<PermissionStatus> {
    let switches = microphone_switches();
    let mut status = PermissionStatus::new(Permission::Microphone, switches.state());
    status.note = switches.off_switch().map(|name| {
        format!("\"{name}\" is switched off in Windows Settings (Privacy & security > Microphone). Switch it on.")
    });
    vec![status]
}

/// The user's pointer in global logical points: physical pixels divided by
/// the scale of the monitor it is on (how the overlay places each monitor).
pub fn pointer_location() -> Option<Point> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: a valid POINT to fill in.
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: plain queries; the monitor handle is checked before use.
    let scale = unsafe {
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        if !monitor.is_null() && GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == 0 {
            f64::from(dpi_x) / 96.0
        } else {
            1.0
        }
    };
    Some(Point::new(f64::from(p.x) / scale, f64::from(p.y) / scale))
}
