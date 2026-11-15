//! Windows, the apps behind them, opening an app and bringing a window to
//! the front. What is inside a window (the element tree) is in `uia`.
//!
//! The window list comes from `EnumWindows`, which walks the top-level
//! windows front to back. What each window is to Pindot is decided in
//! `windows_logic::classify_window`: the desktop is left out, the taskbar is
//! listed as furniture that no app owns (pid -1, so it can never be
//! approved), and click-through overlays (ours, a game's, a recorder's) get
//! alpha 0 so the hit test and the screenshot mask look through them.
//!
//! An app is named the way the Start menu names it, whether it is running
//! or not, so one approval covers both: `windows_logic::start_name_for`,
//! then the executable's description, then its file name. Its `bundle_id`
//! is the executable's file name in lower case.
//!
//! The Start menu's list (`Get-StartApps`: shortcuts and store apps alike)
//! is read through PowerShell, once, and kept. It takes about a second.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint, MonitorFromWindow};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, ES_PASSWORD, EnumChildWindows, EnumWindows, GA_ROOT, GUITHREADINFO, GWL_EXSTYLE, GWL_STYLE,
    GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    WindowFromPoint,
};

use super::capture::{Monitor, dpi_aware, monitor_scale, monitors};
use super::uia;
use crate::platform::windows_logic::{
    RawWindow, StartApp, WindowKind, app_matches, classify_window, find_start_app, parse_start_apps, start_name_for,
    to_physical,
};
use crate::platform::{
    Accessibility, AppInfo, AxTree, DisplayInfo, ElementHandle, HitElement, PlatformError, Point, Rect, ResolvedApp,
    Result, TreeLimits, WindowInfo,
};

const NO_ACTION: &str = "that accessibility action on Windows (only a press)";
/// How long the Start menu's app list is kept.
const START_TTL: Duration = Duration::from_secs(600);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// What the taskbar is listed as. No process owns it for Pindot (pid -1).
const TASKBAR: &str = "Taskbar";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn window_text(h: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: a buffer and its length in characters.
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.clamp(0, buf.len() as i32) as usize])
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: a buffer and its length in characters.
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.clamp(0, buf.len() as i32) as usize])
}

fn owner_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: a plain query with a valid out-pointer.
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    pid
}

unsafe extern "system" fn collect_window(h: HWND, data: LPARAM) -> i32 {
    // SAFETY: `data` is the Vec passed to EnumWindows below, alive for the call.
    unsafe { (*(data as *mut Vec<HWND>)).push(h) };
    1
}

/// Every top-level window, front to back.
fn top_level() -> Vec<HWND> {
    let mut out: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `out`, which outlives the call.
    unsafe { EnumWindows(Some(collect_window), &mut out as *mut _ as LPARAM) };
    out
}

struct CoreSearch {
    frame_pid: u32,
    found: u32,
}

unsafe extern "system" fn find_core_window(h: HWND, data: LPARAM) -> i32 {
    // SAFETY: `data` is the CoreSearch passed to EnumChildWindows below.
    let search = unsafe { &mut *(data as *mut CoreSearch) };
    let pid = owner_pid(h);
    if pid != search.frame_pid && class_name(h) == "Windows.UI.Core.CoreWindow" {
        search.found = pid;
        return 0;
    }
    1
}

/// The process whose app a window shows. A store app's window is a frame
/// that belongs to ApplicationFrameHost; the app itself owns a child.
fn app_pid(h: HWND, class: &str) -> u32 {
    let pid = owner_pid(h);
    if class != "ApplicationFrameWindow" {
        return pid;
    }
    let mut search = CoreSearch { frame_pid: pid, found: 0 };
    // SAFETY: the callback only touches `search`, which outlives the call.
    unsafe { EnumChildWindows(h, Some(find_core_window), &mut search as *mut _ as LPARAM) };
    if search.found != 0 { search.found } else { pid }
}

fn is_cloaked(h: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: a u32 out-value and its size.
    let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, (&mut cloaked as *mut u32).cast(), 4) };
    hr == 0 && cloaked != 0
}

/// A window's visible frame: physical pixels, and global logical points.
fn bounds(h: HWND) -> Option<(RECT, Rect)> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // The frame without the invisible resize border; else the plain rectangle.
    // SAFETY: a RECT out-value and its size.
    let ok = unsafe {
        DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut r as *mut RECT).cast(), size_of::<RECT>() as u32)
            == 0
            || GetWindowRect(h, &mut r) != 0
    };
    if !ok {
        return None;
    }
    // SAFETY: a plain query.
    let s = monitor_scale(unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) });
    let logical = Rect::new(
        f64::from(r.left) / s,
        f64::from(r.top) / s,
        f64::from(r.right - r.left) / s,
        f64::from(r.bottom - r.top) / s,
    );
    Some((r, logical))
}

/// A process's executable and, for a packaged app, its application id.
fn process_identity(pid: u32) -> Option<(PathBuf, Option<String>)> {
    // SAFETY: the handle is checked and closed; the buffers carry their sizes.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let path = (QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0)
            .then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
        let mut id = [0u16; 256];
        let mut id_len = id.len() as u32;
        let aumid = (GetApplicationUserModelId(h, &mut id_len, id.as_mut_ptr()) == 0 && id_len > 1)
            .then(|| String::from_utf16_lossy(&id[..id_len as usize - 1]));
        CloseHandle(h);
        path.map(|p| (p, aumid))
    }
}

/// The "File description" of an executable ("Google Chrome").
fn file_description(path: &Path) -> Option<String> {
    let file = wide(path.to_str()?);
    // SAFETY: the version block is read into a buffer of the size Windows
    // asked for; the pointers VerQueryValueW returns point into that buffer.
    unsafe {
        let size = GetFileVersionInfoSizeW(file.as_ptr(), null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(file.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut ptr: *mut c_void = null_mut();
        let mut len = 0u32;
        let translation = wide("\\VarFileInfo\\Translation");
        if VerQueryValueW(data.as_ptr().cast(), translation.as_ptr(), &mut ptr, &mut len) == 0 || len < 4 {
            return None;
        }
        let (lang, codepage) = (*(ptr as *const u16), *(ptr as *const u16).add(1));
        let key = wide(&format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
        if VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut ptr, &mut len) == 0 || len == 0 {
            return None;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len as usize));
        let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
        (!text.is_empty()).then_some(text)
    }
}

struct StartCache {
    apps: Vec<StartApp>,
    at: Instant,
}

static START: Mutex<Option<StartCache>> = Mutex::new(None);
/// Executable path -> the app's name (it never changes for one file).
static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn load_start_apps() -> Vec<StartApp> {
    let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Get-StartApps | Select-Object Name, AppID | ConvertTo-Json -Compress";
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match out {
        Ok(o) if o.status.success() => parse_start_apps(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            tracing::warn!("the Start menu's app list could not be read: {}", String::from_utf8_lossy(&o.stderr).trim());
            vec![]
        }
        Err(e) => {
            tracing::warn!("the Start menu's app list could not be read: {e}");
            vec![]
        }
    }
}

/// The Start menu's apps: read once, kept for `START_TTL`, or read again
/// now with `refresh` (an app may have been installed since).
fn start_apps(refresh: bool) -> Vec<StartApp> {
    let mut cache = START.lock().unwrap();
    let stale = cache.as_ref().is_none_or(|c| c.at.elapsed() > START_TTL || c.apps.is_empty());
    if refresh || stale {
        let apps = load_start_apps();
        if !apps.is_empty() || cache.is_none() {
            *cache = Some(StartCache { apps, at: Instant::now() });
        }
    }
    cache.as_ref().map(|c| c.apps.clone()).unwrap_or_default()
}

fn app_name(path: &Path, exe: &str, aumid: Option<&str>) -> String {
    let key = path.to_string_lossy().to_lowercase();
    if let Some(name) = NAMES.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()) {
        return name;
    }
    let name = if exe.eq_ignore_ascii_case("explorer.exe") {
        "File Explorer".to_string()
    } else {
        start_name_for(exe, aumid, &start_apps(false))
            .or_else(|| file_description(path))
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| exe.to_string()))
    };
    NAMES.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, name.clone());
    name
}

fn app_for_pid(pid: u32) -> Option<AppInfo> {
    if pid == std::process::id() {
        return Some(AppInfo { pid: pid as i32, name: "Pindot".into(), bundle_id: Some("dev.pindot.app".into()) });
    }
    let (path, aumid) = process_identity(pid)?;
    let exe = path.file_name()?.to_string_lossy().to_string();
    let name = app_name(&path, &exe, aumid.as_deref());
    Some(AppInfo { pid: pid as i32, name, bundle_id: Some(exe.to_lowercase()) })
}

/// One listed window with what the list does not carry.
struct Listed {
    info: WindowInfo,
    kind: WindowKind,
}

fn list_windows() -> Vec<Listed> {
    dpi_aware();
    let mut out = Vec::new();
    for h in top_level() {
        // SAFETY: plain queries on a window handle (a closed window just fails them).
        if unsafe { IsWindowVisible(h) } == 0 {
            continue;
        }
        let class = class_name(h);
        let Some((px, logical)) = bounds(h) else { continue };
        let raw = RawWindow {
            class: &class,
            ex_style: unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32,
            cloaked: is_cloaked(h),
            minimised: unsafe { IsIconic(h) } != 0,
            width: px.right - px.left,
            height: px.bottom - px.top,
        };
        let kind = classify_window(&raw);
        let info = match kind {
            WindowKind::Skip => continue,
            WindowKind::Taskbar => WindowInfo {
                id: h as usize as u64,
                pid: -1,
                app: TASKBAR.into(),
                title: String::new(),
                bounds: logical,
                layer: 20,
                alpha: 1.0,
            },
            WindowKind::ClickThrough | WindowKind::Normal => {
                let pid = app_pid(h, &class);
                WindowInfo {
                    id: h as usize as u64,
                    pid: pid as i32,
                    // A process Pindot may not look into (an administrator's, a system one).
                    app: app_for_pid(pid).map_or_else(|| "an app Pindot cannot identify".to_string(), |a| a.name),
                    title: window_text(h),
                    bounds: logical,
                    layer: 0,
                    alpha: if kind == WindowKind::ClickThrough { 0.0 } else { 1.0 },
                }
            }
        };
        out.push(Listed { info, kind });
    }
    out
}

fn bring_to_front(h: HWND) {
    // SAFETY: plain window calls; every one tolerates a window that is gone.
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        let front = GetForegroundWindow();
        if front == h {
            return;
        }
        // Windows only lets the foreground thread hand over the foreground:
        // share its input state for the moment of the call.
        let me = GetCurrentThreadId();
        let front_thread = if front.is_null() { 0 } else { GetWindowThreadProcessId(front, null_mut()) };
        let attached = front_thread != 0 && front_thread != me && AttachThreadInput(me, front_thread, 1) != 0;
        BringWindowToTop(h);
        SetForegroundWindow(h);
        if attached {
            AttachThreadInput(me, front_thread, 0);
        }
        if GetForegroundWindow() != h {
            // Still refused: allowed right after input, so tap Alt.
            super::input::tap_alt();
            SetForegroundWindow(h);
        }
    }
}

pub struct WinAccessibility;

impl Accessibility for WinAccessibility {
    fn is_trusted(&self) -> bool {
        // Windows has no per-app accessibility permission.
        true
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(list_windows().into_iter().map(|w| w.info).collect())
    }

    fn running_apps(&self) -> Result<Vec<AppInfo>> {
        let mut apps: Vec<AppInfo> = Vec::new();
        for w in list_windows() {
            if w.kind != WindowKind::Normal || w.info.title.is_empty() || apps.iter().any(|a| a.pid == w.info.pid) {
                continue;
            }
            if let Some(a) = app_for_pid(w.info.pid as u32) {
                apps.push(a);
            }
        }
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(apps)
    }

    fn app_info(&self, pid: i32) -> Option<AppInfo> {
        u32::try_from(pid).ok().and_then(app_for_pid)
    }

    fn frontmost_pid(&self) -> Result<Option<i32>> {
        // SAFETY: a plain query.
        let h = unsafe { GetForegroundWindow() };
        if h.is_null() {
            return Ok(None);
        }
        Ok(Some(app_pid(h, &class_name(h)) as i32))
    }

    fn resolve_app(&self, name: &str) -> Option<ResolvedApp> {
        let query = name.trim();
        if let Ok(apps) = self.running_apps()
            && let Some(a) = apps.iter().find(|a| app_matches(query, &a.name, a.bundle_id.as_deref().unwrap_or_default()))
        {
            return Some(ResolvedApp { name: a.name.clone(), bundle_id: a.bundle_id.clone(), running: true });
        }
        let found = find_start_app(&start_apps(false), query).cloned().or_else(|| {
            // Installed since the list was read?
            find_start_app(&start_apps(true), query).cloned()
        })?;
        Some(ResolvedApp { name: found.name, bundle_id: None, running: false })
    }

    fn launch_app(&self, name: &str) -> Result<()> {
        let query = name.trim();
        // Already running: bring its front window forward.
        let running = list_windows().into_iter().find(|w| {
            w.kind == WindowKind::Normal
                && !w.info.title.is_empty()
                && app_for_pid(w.info.pid as u32)
                    .is_some_and(|a| app_matches(query, &a.name, a.bundle_id.as_deref().unwrap_or_default()))
        });
        if let Some(w) = running {
            bring_to_front(w.info.id as usize as HWND);
            return Ok(());
        }
        let apps = start_apps(false);
        let app = find_start_app(&apps, query)
            .ok_or_else(|| PlatformError::Other(format!("No app called \"{query}\" is in the Start menu.")))?;
        std::process::Command::new("explorer.exe")
            .arg(format!("shell:AppsFolder\\{}", app.app_id))
            .spawn()
            .map(|_| ())
            .map_err(|e| PlatformError::Other(format!("could not open {}: {e}", app.name)))
    }

    fn window_tree(&self, window: &WindowInfo, limits: &TreeLimits) -> Result<AxTree> {
        let h = window.id as usize as HWND;
        // SAFETY: plain queries.
        if unsafe { IsWindow(h) } == 0 {
            return Err(PlatformError::Stale);
        }
        let scale = monitor_scale(unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) });
        uia::window_tree(window.id as usize, scale, limits)
    }

    fn element_frame(&self, el: &ElementHandle) -> Result<Option<Rect>> {
        // In physical pixels first: the scale is that of the monitor it is on.
        let Some(px) = uia::element_frame(el, 1.0)? else { return Ok(None) };
        let c = px.center();
        // SAFETY: a plain query.
        let s = monitor_scale(unsafe { MonitorFromPoint(POINT { x: c.x as i32, y: c.y as i32 }, MONITOR_DEFAULTTONEAREST) });
        Ok(Some(Rect::new(px.x / s, px.y / s, px.width / s, px.height / s)))
    }

    fn press(&self, el: &ElementHandle) -> Result<()> {
        uia::press(el)
    }

    fn can_press(&self, el: &ElementHandle) -> Result<bool> {
        uia::can_press(el)
    }

    fn has_action(&self, el: &ElementHandle, action: &str) -> Result<bool> {
        // UI Automation has a press; a context menu is a real right-click.
        Ok(action == "AXPress" && uia::can_press(el)?)
    }

    fn perform(&self, el: &ElementHandle, action: &str) -> Result<()> {
        if action == "AXPress" { uia::press(el) } else { Err(PlatformError::Unsupported(NO_ACTION)) }
    }

    fn element_at(&self, at: Point) -> Result<Option<HitElement>> {
        let displays: Vec<DisplayInfo> = monitors().iter().map(Monitor::info).collect();
        let (x, y) = to_physical(&displays, at);
        // SAFETY: plain queries.
        let root = unsafe {
            let h = WindowFromPoint(POINT { x, y });
            if h.is_null() {
                return Ok(None);
            }
            let root = GetAncestor(h, GA_ROOT);
            if root.is_null() { h } else { root }
        };
        let class = class_name(root);
        Ok(Some(HitElement { pid: app_pid(root, &class) as i32, role: class, name: window_text(root) }))
    }

    fn raise_window(&self, window: &WindowInfo) -> Result<()> {
        let h = window.id as usize as HWND;
        // SAFETY: a plain query.
        if unsafe { IsWindow(h) } == 0 {
            return Err(PlatformError::Stale);
        }
        bring_to_front(h);
        Ok(())
    }

    /// Two checks, either is enough: UI Automation's `IsPassword` on the
    /// focused element (browsers, modern apps, classic ones), and a classic
    /// Edit control with `ES_PASSWORD` in case UI Automation fails.
    fn focused_is_secure(&self) -> Result<bool> {
        if uia::focused_is_password().unwrap_or(false) {
            return Ok(true);
        }
        // SAFETY: a zeroed GUITHREADINFO with its size is what the call wants.
        unsafe {
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(0, &mut info) == 0 || info.hwndFocus.is_null() {
                return Ok(false);
            }
            let class = class_name(info.hwndFocus).to_ascii_lowercase();
            let style = GetWindowLongPtrW(info.hwndFocus, GWL_STYLE) as i32;
            Ok(class.contains("edit") && style & ES_PASSWORD != 0)
        }
    }
}
