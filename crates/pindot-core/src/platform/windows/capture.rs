//! Screen capture with GDI: one `BitBlt` of a monitor's rectangle from the
//! screen. Windows that opted out of capture (`WDA_EXCLUDEFROMCAPTURE`: our
//! overlay, main and approval windows) are left out by the compositor.
//!
//! Coordinates. Windows speaks physical pixels; Pindot speaks global
//! logical points. Here a logical point is a physical pixel divided by the
//! scale of the monitor it is on, the same rule the overlay uses to place
//! each monitor (`windows_logic::to_physical` is the way back).

use std::ptr::null_mut;
use std::sync::Once;

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowDisplayAffinity, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowVisible,
    MONITORINFOF_PRIMARY, SetWindowDisplayAffinity,
};

use crate::platform::windows_logic::{OwnWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, display_info, windows_to_exclude};
use crate::platform::{Capture, CapturedFrame, DisplayInfo, PlatformError, Result};

/// See real pixels on every monitor. The app's manifest already says so;
/// this covers the command-line tools (the call fails harmlessly when the
/// awareness is already set).
pub(super) fn dpi_aware() {
    static ONCE: Once = Once::new();
    // SAFETY: a process-wide setting, made once.
    ONCE.call_once(|| unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
}

/// One monitor, in physical pixels.
pub(super) struct Monitor {
    pub handle: HMONITOR,
    pub rect: RECT,
    pub primary: bool,
    pub name: String,
}

impl Monitor {
    /// The display as the tools see it: a point is a pixel, on every
    /// monitor, whatever Windows scales it by (`windows_logic::display_info`).
    pub fn info(&self) -> DisplayInfo {
        let r = self.rect;
        display_info(self.handle as usize as u32, self.name.clone(), (r.left, r.top, r.right, r.bottom), self.primary)
    }
}

unsafe extern "system" fn collect_monitor(h: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> i32 {
    // SAFETY: `data` is the Vec passed to EnumDisplayMonitors below, alive for the call.
    unsafe { (*(data as *mut Vec<HMONITOR>)).push(h) };
    1
}

/// Every monitor, primary first.
pub(super) fn monitors() -> Vec<Monitor> {
    dpi_aware();
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    unsafe { EnumDisplayMonitors(null_mut(), std::ptr::null(), Some(collect_monitor), &mut handles as *mut _ as LPARAM) };
    let mut out: Vec<Monitor> = handles
        .into_iter()
        .filter_map(|handle| {
            // SAFETY: zeroed is a valid MONITORINFOEXW; cbSize tells Windows which struct it is.
            let mut mi: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(handle, &mut mi as *mut _ as *mut MONITORINFO) } == 0 {
                return None;
            }
            let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            Some(Monitor {
                handle,
                rect: mi.monitorInfo.rcMonitor,
                primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                name: String::from_utf16_lossy(&mi.szDevice[..len]),
            })
        })
        .collect();
    out.sort_by_key(|m| !m.primary);
    out
}

unsafe extern "system" fn collect_own(h: HWND, data: LPARAM) -> i32 {
    let mut pid = 0u32;
    // SAFETY: `h` is a live handle from EnumWindows; `data` is the Vec passed below.
    unsafe {
        GetWindowThreadProcessId(h, &mut pid);
        if pid == GetCurrentProcessId() {
            let mut affinity = 0u32;
            GetWindowDisplayAffinity(h, &mut affinity);
            let ex_style = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            (*(data as *mut Vec<OwnWindow>)).push((h as isize, IsWindowVisible(h) != 0, affinity, ex_style));
        }
    }
    1
}

/// Only one capture at a time may borrow the windows' affinity: a second one
/// would see them already excluded and the first one's restore would then
/// expose them in the middle of the second's copy.
static EXCLUDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Streaming mode: while this lives, the overlay windows are excluded from
/// capture, so the screenshot Pindot takes leaves them out although OBS sees
/// them the rest of the time. Dropping it puts them back, on every path,
/// errors included. No other window of ours is ever touched.
struct OwnWindowsExcluded {
    set: Vec<isize>,
    _one_at_a_time: std::sync::MutexGuard<'static, ()>,
}

impl OwnWindowsExcluded {
    fn begin() -> Self {
        let one = EXCLUDING.lock().unwrap_or_else(|p| p.into_inner());
        let mut own: Vec<OwnWindow> = Vec::new();
        // SAFETY: the callback only pushes into `own`, which outlives the call.
        unsafe { EnumWindows(Some(collect_own), &mut own as *mut _ as LPARAM) };
        let mut set = Vec::new();
        for h in windows_to_exclude(&own) {
            // SAFETY: a window handle of our own process; failure just leaves it as it was.
            if unsafe { SetWindowDisplayAffinity(h as HWND, WDA_EXCLUDEFROMCAPTURE) } != 0 {
                set.push(h);
            }
        }
        if !set.is_empty() {
            // Let the compositor drop them from what it hands to capture.
            // SAFETY: no arguments.
            unsafe {
                DwmFlush();
                DwmFlush();
            }
        }
        Self { set, _one_at_a_time: one }
    }
}

impl Drop for OwnWindowsExcluded {
    fn drop(&mut self) {
        // Streaming mode was switched off during the copy: the overlay is
        // meant to stay hidden from now on, so it is not put back.
        if !crate::platform::streaming_capture() {
            return;
        }
        for &h in &self.set {
            // SAFETY: as in `begin`. A window that closed meanwhile makes this fail harmlessly.
            unsafe { SetWindowDisplayAffinity(h as HWND, WDA_NONE) };
        }
        // Switched off between the check and the restore: hide it again. (The
        // switch itself hides the overlay once more after it flips the flag,
        // so whichever of the two runs last leaves the overlay hidden.)
        if !crate::platform::streaming_capture() {
            for &h in &self.set {
                // SAFETY: as above.
                unsafe { SetWindowDisplayAffinity(h as HWND, WDA_EXCLUDEFROMCAPTURE) };
            }
        }
    }
}

pub struct WinCapture;

impl Capture for WinCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let all = monitors();
        if all.is_empty() {
            return Err(PlatformError::Other("no display found".into()));
        }
        Ok(all.iter().map(Monitor::info).collect())
    }

    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame> {
        let all = monitors();
        let m = all
            .iter()
            .find(|m| m.handle as usize as u32 == display_id)
            .ok_or_else(|| PlatformError::Other(format!("display {display_id} not found")))?;
        let (w, h) = (m.rect.right - m.rect.left, m.rect.bottom - m.rect.top);
        if w <= 0 || h <= 0 {
            return Err(PlatformError::Other("the display has no size".into()));
        }
        // Streaming mode: hide our own overlay from the compositor for the
        // length of the copy only.
        let excluded = crate::platform::streaming_capture().then(OwnWindowsExcluded::begin);
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: GDI objects are created, used and released here; `pixels`
        // is exactly w*h 32-bit pixels, which is what GetDIBits is asked for.
        let copied = unsafe {
            let screen = GetDC(null_mut());
            let mem = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bitmap);
            let blt = BitBlt(mem, 0, 0, w, h, screen, m.rect.left, m.rect.top, SRCCOPY | CAPTUREBLT);
            // The bitmap must not be selected into a DC while GetDIBits reads it.
            SelectObject(mem, old);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let rows = GetDIBits(mem, bitmap, 0, h as u32, pixels.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS);
            DeleteObject(bitmap);
            DeleteDC(mem);
            ReleaseDC(null_mut(), screen);
            blt != 0 && rows == h
        };
        drop(excluded);
        if !copied {
            return Err(PlatformError::Other(format!("the screen could not be copied: {}", std::io::Error::last_os_error())));
        }
        // GDI gives BGRX; the image wants RGBA.
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        let image = image::RgbaImage::from_raw(w as u32, h as u32, pixels)
            .ok_or_else(|| PlatformError::Other("the captured image has the wrong size".into()))?;
        Ok(CapturedFrame { display: m.info(), image })
    }
}
