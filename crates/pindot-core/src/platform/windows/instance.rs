//! One Pindot per data folder. Starting it a second time (a double-click,
//! the Start-menu entry) would give two overlays, two tray icons and a
//! second copy without its port. macOS brings the running app forward by
//! itself; here the first instance holds a named lock, and a second one
//! finds it, signals a named event ("show your window") and exits.
//!
//! The names come from the data folder (`windows_logic::instance_name`), so
//! a test instance with its own folder runs beside the real one.

use std::path::Path;
use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};

use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

use crate::platform::windows_logic::instance_name;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Held by the first instance for as long as it runs.
pub struct InstanceGuard {
    // Handles as numbers: the guard is kept in app state shared by threads.
    lock: isize,
    show: isize,
}

pub enum Claim {
    First(InstanceGuard),
    /// Another Pindot with this data folder is running.
    Second,
}

/// Take the lock for `data_dir`, or learn that another instance has it.
pub fn claim(data_dir: &Path) -> Claim {
    let dir = data_dir.to_string_lossy();
    let (lock_name, show_name) = (wide(&instance_name(&dir, "lock")), wide(&instance_name(&dir, "show")));
    // SAFETY: NUL-terminated names; the handles are owned by the guard.
    unsafe {
        let lock = CreateMutexW(null(), 0, lock_name.as_ptr());
        if lock.is_null() {
            // Could not even create it: do not keep the app from starting.
            return Claim::First(InstanceGuard { lock: 0, show: 0 });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(lock);
            return Claim::Second;
        }
        // Auto-reset: one signal wakes the waiting thread once.
        let show = CreateEventW(null(), 0, 0, show_name.as_ptr());
        Claim::First(InstanceGuard { lock: lock as isize, show: show as isize })
    }
}

impl InstanceGuard {
    /// Call `show` each time a second launch asks for the window.
    pub fn on_show(&self, show: impl Fn() + Send + 'static) {
        let event = self.show;
        if event == 0 {
            return;
        }
        let spawned = std::thread::Builder::new().name("pindot-instance".into()).spawn(move || {
            // SAFETY: the event handle lives as long as the process.
            while unsafe { WaitForSingleObject(event as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
                show();
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("a second launch will not open the window: {e}");
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: closing handles this guard owns.
        unsafe {
            if self.show != 0 {
                CloseHandle(self.show as HANDLE);
            }
            if self.lock != 0 {
                CloseHandle(self.lock as HANDLE);
            }
        }
    }
}

/// From a second instance: ask the first to show its window. False if it
/// could not be reached.
pub fn ask_first_to_show(data_dir: &Path) -> bool {
    let name = wide(&instance_name(&data_dir.to_string_lossy(), "show"));
    // SAFETY: a NUL-terminated name; the handle is closed here.
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if event.is_null() {
            return false;
        }
        // The user just started this process, so it may take the foreground;
        // the first instance may not. Pass that right on, or its window
        // would only flash in the taskbar.
        AllowSetForegroundWindow(ASFW_ANY);
        let ok = SetEvent(event) != 0;
        CloseHandle(event);
        ok
    }
}
