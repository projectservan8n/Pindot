//! macOS accessibility: the AX element tree (AXUIElement), the window list
//! (CGWindowList), running apps (NSRunningApplication) and app launch.
//!
//! No Apple Events anywhere: launching goes through LaunchServices
//! (`/usr/bin/open -a`), which needs no Automation permission.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Arc, Once};
use std::time::Instant;

use objc2_app_kit::{NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_application_services::{AXCopyMultipleAttributeOptions, AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize, Type,
};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};

use crate::platform::{
    Accessibility, AppInfo, AxNode, AxTree, ElementHandle, HitElement, PlatformError, Point, Rect, ResolvedApp,
    Result, TreeLimits, WindowInfo,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    /// Private but long-stable (used by every macOS window manager): the
    /// CGWindowID behind an AX window element.
    fn _AXUIElementGetWindow(element: &AXUIElement, out: *mut u32) -> AXError;
}

/// True if this process (really: the responsible app) has the Accessibility
/// permission. Never prompts.
pub fn ax_is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// An AX element that can cross threads. AX calls are IPC to the target app
/// and the API is safe to call from any thread.
pub struct AxEl(pub CFRetained<AXUIElement>);
unsafe impl Send for AxEl {}
unsafe impl Sync for AxEl {}

fn handle_of(el: CFRetained<AXUIElement>) -> ElementHandle {
    Arc::new(AxEl(el))
}

fn el_of(h: &ElementHandle) -> Result<&AXUIElement> {
    h.downcast_ref::<AxEl>().map(|e| &*e.0).ok_or_else(|| PlatformError::Other("not a macOS AX element".into()))
}

static TIMEOUT: Once = Once::new();

/// Cap how long one AX call may block on an unresponsive app (process-wide).
fn set_global_timeout() {
    TIMEOUT.call_once(|| {
        let sys = unsafe { AXUIElement::new_system_wide() };
        unsafe { sys.set_messaging_timeout(0.5) };
    });
}

fn cfstr(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

/// Copy one attribute. `Err(Stale)` if the element is gone, `Ok(None)` if the
/// attribute is missing or empty.
fn attr(el: &AXUIElement, name: &str) -> Result<Option<CFRetained<CFType>>> {
    let key = cfstr(name);
    let mut out: *const CFType = std::ptr::null();
    let err = unsafe { el.copy_attribute_value(&key, NonNull::from(&mut out)) };
    if err == AXError::InvalidUIElement {
        return Err(PlatformError::Stale);
    }
    if err != AXError::Success {
        return Ok(None);
    }
    Ok(NonNull::new(out as *mut CFType).map(|p| unsafe { CFRetained::from_raw(p) }))
}

fn attr_opt(el: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    attr(el, name).ok().flatten()
}

fn as_string(v: &CFType) -> Option<String> {
    if let Some(s) = v.downcast_ref::<CFString>() {
        return Some(s.to_string());
    }
    if let Some(n) = v.downcast_ref::<CFNumber>() {
        if let Some(i) = n.as_i64() {
            return Some(i.to_string());
        }
        return n.as_f64().map(|f| format!("{f:.2}"));
    }
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return Some(b.as_bool().to_string());
    }
    None
}

fn as_bool(v: &CFType) -> Option<bool> {
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return Some(b.as_bool());
    }
    v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()).map(|i| i != 0)
}

fn as_i64(v: &CFType) -> Option<i64> {
    v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64())
}

fn as_point(v: &CFType) -> Option<CGPoint> {
    let v = v.downcast_ref::<AXValue>()?;
    let mut p = CGPoint::new(0.0, 0.0);
    let ok = unsafe { v.value(AXValueType::CGPoint, NonNull::from(&mut p).cast::<c_void>()) };
    ok.then_some(p)
}

fn as_size(v: &CFType) -> Option<CGSize> {
    let v = v.downcast_ref::<AXValue>()?;
    let mut s = CGSize::new(0.0, 0.0);
    let ok = unsafe { v.value(AXValueType::CGSize, NonNull::from(&mut s).cast::<c_void>()) };
    ok.then_some(s)
}

fn as_elements(v: &CFType) -> Vec<CFRetained<AXUIElement>> {
    let Some(arr) = v.downcast_ref::<CFArray>() else { return vec![] };
    let arr: &CFArray<CFType> = unsafe { arr.cast_unchecked() };
    arr.iter().filter_map(|x| x.downcast::<AXUIElement>().ok()).collect()
}

fn frame_of(pos: Option<&CFType>, size: Option<&CFType>) -> Option<Rect> {
    let p = as_point(pos?)?;
    let s = as_size(size?)?;
    Some(Rect::new(p.x, p.y, s.width, s.height))
}

fn element_pid(el: &AXUIElement) -> Option<i32> {
    let mut pid: libc_pid = 0;
    let err = unsafe { el.pid(NonNull::from(&mut pid)) };
    (err == AXError::Success).then_some(pid)
}

#[allow(non_camel_case_types)]
type libc_pid = i32;

/// Attributes read for every node in one IPC round trip.
const NODE_ATTRS: [&str; 14] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXFocused",
    "AXPlaceholderValue",
    "AXHelp",
    "AXChildren",
    "AXMenuItemCmdChar",
    "AXMenuItemCmdModifiers",
];

struct Attrs(Vec<Option<CFRetained<CFType>>>);

impl Attrs {
    fn get(&self, name: &str) -> Option<&CFType> {
        let i = NODE_ATTRS.iter().position(|a| *a == name)?;
        self.0.get(i).and_then(|v| v.as_deref())
    }
    fn string(&self, name: &str) -> Option<String> {
        self.get(name).and_then(as_string).filter(|s| !s.is_empty())
    }
}

struct Walker {
    names: CFRetained<CFArray<CFString>>,
    deadline: Instant,
    limits: TreeLimits,
    nodes: Vec<AxNode>,
    truncated: bool,
}

impl Walker {
    fn new(limits: &TreeLimits) -> Self {
        let owned: Vec<CFRetained<CFString>> = NODE_ATTRS.iter().map(|a| cfstr(a)).collect();
        let refs: Vec<&CFString> = owned.iter().map(|s| &**s).collect();
        Self {
            names: CFArray::from_objects(&refs),
            deadline: Instant::now() + limits.timeout,
            limits: *limits,
            nodes: Vec::new(),
            truncated: false,
        }
    }

    fn over_budget(&mut self) -> bool {
        if self.nodes.len() >= self.limits.max_nodes || Instant::now() > self.deadline {
            self.truncated = true;
        }
        self.truncated
    }

    fn read(&self, el: &AXUIElement) -> Result<Attrs> {
        let mut out: *const CFArray = std::ptr::null();
        let err = unsafe {
            el.copy_multiple_attribute_values(
                self.names.as_opaque(),
                AXCopyMultipleAttributeOptions(0),
                NonNull::from(&mut out),
            )
        };
        if err == AXError::InvalidUIElement {
            return Err(PlatformError::Stale);
        }
        let Some(arr) = NonNull::new(out as *mut CFArray).map(|p| unsafe { CFRetained::from_raw(p) }) else {
            return Err(PlatformError::Other(format!("AX read failed ({})", err.0)));
        };
        let arr: &CFArray<CFType> = unsafe { arr.cast_unchecked() };
        let vals = (0..NODE_ATTRS.len())
            .map(|i| {
                arr.get(i).filter(|v| {
                    // Failed attributes come back as an AXValue holding an AXError.
                    v.downcast_ref::<AXValue>().map(|x| unsafe { x.r#type() } != AXValueType::AXError).unwrap_or(true)
                })
            })
            .collect();
        Ok(Attrs(vals))
    }

    fn node(&self, el: &AXUIElement, a: &Attrs, depth: u16) -> AxNode {
        let role = a.string("AXRole").unwrap_or_else(|| "AXUnknown".into());
        let value = a.get("AXValue").and_then(as_string);
        let mut name = a
            .string("AXTitle")
            .or_else(|| a.string("AXDescription"))
            .or_else(|| a.string("AXPlaceholderValue"))
            .or_else(|| a.string("AXHelp"))
            .unwrap_or_default();
        let mut value = value;
        if name.is_empty() && role == "AXStaticText" {
            name = value.take().unwrap_or_default();
        }
        let shortcut = a.string("AXMenuItemCmdChar").map(|c| {
            let m = a.get("AXMenuItemCmdModifiers").and_then(as_i64).unwrap_or(0);
            format_shortcut(&c, m)
        });
        AxNode {
            handle: handle_of(el.retain()),
            depth,
            subrole: a.string("AXSubrole"),
            name,
            value,
            bounds: frame_of(a.get("AXPosition"), a.get("AXSize")),
            enabled: a.get("AXEnabled").and_then(as_bool).unwrap_or(true),
            focused: a.get("AXFocused").and_then(as_bool).unwrap_or(false),
            shortcut,
            menu_path: None,
            menu_root: None,
            role,
        }
    }

    fn walk(&mut self, el: &AXUIElement, depth: u16) -> Result<()> {
        if self.over_budget() {
            return Ok(());
        }
        let a = match self.read(el) {
            Ok(a) => a,
            // The root must exist; a child that vanished mid-walk is skipped.
            Err(PlatformError::Stale) if depth > 0 => return Ok(()),
            Err(e) => return Err(e),
        };
        let node = self.node(el, &a, depth);
        let is_menu_bar = node.role == "AXMenuBar";
        self.nodes.push(node);
        if is_menu_bar {
            // The app's menu bar is listed separately (menu_depth).
            return Ok(());
        }
        if depth >= self.limits.max_depth {
            self.truncated = true;
            return Ok(());
        }
        let children = a.get("AXChildren").map(as_elements).unwrap_or_default();
        for c in children {
            if self.over_budget() {
                break;
            }
            self.walk(&c, depth + 1)?;
        }
        Ok(())
    }

    /// The app's menu bar: titles (depth 0) and, if `menu_depth >= 2`, the
    /// items under them. The Apple menu is left out on purpose: approving an
    /// app must not reach Shut Down, System Settings or Force Quit.
    fn menus(&mut self, app: &AXUIElement) {
        if self.limits.menu_depth == 0 {
            return;
        }
        let Some(bar) = attr_opt(app, "AXMenuBar").and_then(|v| v.downcast::<AXUIElement>().ok()) else {
            return;
        };
        let items = attr_opt(&bar, "AXChildren").map(|v| as_elements(&v)).unwrap_or_default();
        for item in items {
            if self.over_budget() {
                return;
            }
            let Ok(a) = self.read(&item) else { continue };
            let mut node = self.node(&item, &a, 0);
            if node.name.is_empty() || node.name == "Apple" {
                continue;
            }
            let root = self.nodes.len();
            let title = node.name.clone();
            node.menu_path = Some(title.clone());
            node.menu_root = Some(root);
            self.nodes.push(node);
            if self.limits.menu_depth >= 2 {
                for menu in a.get("AXChildren").map(as_elements).unwrap_or_default() {
                    self.menu_items(&menu, &title, root, 1, self.limits.menu_depth - 1);
                }
            }
        }
    }

    fn menu_items(&mut self, menu: &AXUIElement, path: &str, root: usize, depth: u16, levels: u16) {
        let items = attr_opt(menu, "AXChildren").map(|v| as_elements(&v)).unwrap_or_default();
        for item in items {
            if self.over_budget() {
                return;
            }
            let Ok(a) = self.read(&item) else { continue };
            let mut node = self.node(&item, &a, depth);
            if node.name.is_empty() {
                continue; // separators
            }
            let item_path = format!("{path} > {}", node.name);
            node.menu_path = Some(item_path.clone());
            node.menu_root = Some(root);
            // Frames of items in a closed menu are stale; menu items are
            // pressed through AX, not clicked at a position.
            node.bounds = None;
            self.nodes.push(node);
            if levels > 1 {
                for sub in a.get("AXChildren").map(as_elements).unwrap_or_default() {
                    self.menu_items(&sub, &item_path, root, depth + 1, levels - 1);
                }
            }
        }
    }
}

/// Menu shortcut text, e.g. "⇧⌘S". Modifier bits: 1 shift, 2 option,
/// 4 control, 8 = no Command.
fn format_shortcut(c: &str, m: i64) -> String {
    let mut s = String::new();
    if m & 4 != 0 {
        s.push('⌃');
    }
    if m & 2 != 0 {
        s.push('⌥');
    }
    if m & 1 != 0 {
        s.push('⇧');
    }
    if m & 8 == 0 {
        s.push('⌘');
    }
    s.push_str(c);
    s
}

/// The AX window element behind a CGWindowID (exact match through
/// `_AXUIElementGetWindow`, else by title and frame).
fn ax_window(window: &WindowInfo) -> Result<CFRetained<AXUIElement>> {
    let app = unsafe { AXUIElement::new_application(window.pid) };
    let wins = attr(&app, "AXWindows")?.map(|v| as_elements(&v)).unwrap_or_default();
    let mut fallback = None;
    let same = |f: Option<Rect>| {
        f.is_some_and(|f| {
            (f.x - window.bounds.x).abs() < 2.0
                && (f.y - window.bounds.y).abs() < 2.0
                && (f.width - window.bounds.width).abs() < 2.0
                && (f.height - window.bounds.height).abs() < 2.0
        })
    };
    for w in wins.iter().cloned() {
        let mut id: u32 = 0;
        if unsafe { _AXUIElementGetWindow(&w, &mut id) } == AXError::Success && id as u64 == window.id {
            return Ok(w);
        }
        let title = attr_opt(&w, "AXTitle").and_then(|v| as_string(&v)).unwrap_or_default();
        let frame = frame_of(attr_opt(&w, "AXPosition").as_deref(), attr_opt(&w, "AXSize").as_deref());
        if same(frame) && (title == window.title || fallback.is_none()) {
            fallback = Some(w);
        }
    }
    if fallback.is_none() {
        // A sheet (a Save sheet, say) has its own CGWindowID but no AX
        // window: it is a child of its document window, so read that.
        // While the sheet slides in its frame is not yet the CG one, so an
        // overlapping sheet counts when no frame matches exactly.
        let sheets = |w: &CFRetained<AXUIElement>| -> Vec<Option<Rect>> {
            attr_opt(w, "AXChildren")
                .map(|v| as_elements(&v))
                .unwrap_or_default()
                .iter()
                .filter(|c| attr_opt(c, "AXRole").and_then(|v| as_string(&v)).as_deref() == Some("AXSheet"))
                .map(|c| frame_of(attr_opt(c, "AXPosition").as_deref(), attr_opt(c, "AXSize").as_deref()))
                .collect()
        };
        let with_sheets: Vec<_> = wins
            .into_iter()
            .map(|w| {
                let s = sheets(&w);
                (w, s)
            })
            .collect();
        fallback = with_sheets
            .iter()
            .find(|(_, s)| s.iter().any(|f| same(*f)))
            .or_else(|| {
                with_sheets
                    .iter()
                    .find(|(_, s)| s.iter().any(|f| f.is_some_and(|f| f.intersect(&window.bounds).is_some())))
            })
            .map(|(w, _)| w.clone());
    }
    fallback.ok_or_else(|| {
        PlatformError::Other(format!(
            "no accessibility window found for window {} of {} (is it a panel from another process?)",
            window.id, window.app
        ))
    })
}

fn dict_get(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CFRetained<CFType>> {
    dict.get(key)
}

fn ns_app(pid: i32) -> Option<objc2::rc::Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}

fn info_of(app: &NSRunningApplication) -> AppInfo {
    AppInfo {
        pid: app.processIdentifier(),
        name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
        bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
    }
}

const APP_DIRS: [&str; 5] = [
    "/Applications",
    "/System/Applications",
    "/System/Applications/Utilities",
    "/Applications/Utilities",
    "/System/Library/CoreServices/Applications",
];

fn installed_app(name: &str) -> Option<PathBuf> {
    let file = format!("{name}.app");
    let mut dirs: Vec<PathBuf> = APP_DIRS.iter().map(PathBuf::from).collect();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
    }
    for d in dirs {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.eq_ignore_ascii_case(&file) {
                return Some(e.path());
            }
        }
    }
    None
}

pub struct MacAccessibility;

impl Accessibility for MacAccessibility {
    fn is_trusted(&self) -> bool {
        ax_is_trusted()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let opts = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
        let Some(list) = CGWindowListCopyWindowInfo(opts, 0) else {
            return Err(PlatformError::Other("CGWindowListCopyWindowInfo failed".into()));
        };
        let list: &CFArray<CFDictionary> = unsafe { list.cast_unchecked() };
        let (k_num, k_layer, k_owner, k_name, k_pid, k_bounds, k_alpha) = unsafe {
            (kCGWindowNumber, kCGWindowLayer, kCGWindowOwnerName, kCGWindowName, kCGWindowOwnerPID, kCGWindowBounds, kCGWindowAlpha)
        };
        let mut out = Vec::new();
        for d in list.iter() {
            let d: &CFDictionary<CFString, CFType> = unsafe { d.cast_unchecked() };
            let num = |k: &CFString| dict_get(d, k).as_deref().and_then(as_i64);
            let text = |k: &CFString| dict_get(d, k).as_deref().and_then(as_string).unwrap_or_default();
            let Some(id) = num(k_num) else { continue };
            let mut rect = CGRect::default();
            let bounds = dict_get(d, k_bounds)
                .and_then(|b| b.downcast::<CFDictionary>().ok())
                .filter(|b| unsafe { CGRectMakeWithDictionaryRepresentation(Some(b), &mut rect) })
                .map(|_| Rect::new(rect.origin.x, rect.origin.y, rect.size.width, rect.size.height));
            let Some(bounds) = bounds else { continue };
            out.push(WindowInfo {
                id: id as u64,
                pid: num(k_pid).unwrap_or(-1) as i32,
                app: text(k_owner),
                title: text(k_name),
                bounds,
                layer: num(k_layer).unwrap_or(0) as i32,
                alpha: dict_get(d, k_alpha)
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_f64())
                    .unwrap_or(1.0),
            });
        }
        Ok(out)
    }

    fn running_apps(&self) -> Result<Vec<AppInfo>> {
        let mut apps: Vec<AppInfo> = NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular && !a.isTerminated())
            .map(|a| info_of(&a))
            .collect();
        // NSWorkspace only refreshes on the main run loop; the window list is
        // always current, so add apps that own a normal window.
        for w in self.windows()?.into_iter().filter(|w| w.layer == 0) {
            if !apps.iter().any(|a| a.pid == w.pid) {
                if let Some(a) = self.app_info(w.pid) {
                    apps.push(a);
                }
            }
        }
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        apps.dedup_by_key(|a| a.pid);
        Ok(apps)
    }

    fn app_info(&self, pid: i32) -> Option<AppInfo> {
        ns_app(pid).filter(|a| !a.isTerminated()).map(|a| info_of(&a))
    }

    fn frontmost_pid(&self) -> Result<Option<i32>> {
        set_global_timeout();
        let sys = unsafe { AXUIElement::new_system_wide() };
        if let Some(app) = attr_opt(&sys, "AXFocusedApplication").and_then(|v| v.downcast::<AXUIElement>().ok()) {
            if let Some(pid) = element_pid(&app) {
                return Ok(Some(pid));
            }
        }
        Ok(NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier()))
    }

    fn resolve_app(&self, name: &str) -> Option<ResolvedApp> {
        let name = name.trim().trim_end_matches(".app");
        if let Ok(apps) = self.running_apps() {
            if let Some(a) = apps.iter().find(|a| {
                a.name.eq_ignore_ascii_case(name) || a.bundle_id.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(name))
            }) {
                return Some(ResolvedApp { name: a.name.clone(), bundle_id: a.bundle_id.clone(), running: true });
            }
        }
        let path = installed_app(name)?;
        let display = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.into());
        Some(ResolvedApp { name: display, bundle_id: None, running: false })
    }

    fn launch_app(&self, name: &str) -> Result<()> {
        // LaunchServices through `open`; no Apple Events, no Automation prompt.
        let out = std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(name)
            .output()
            .map_err(|e| PlatformError::Other(format!("open -a failed: {e}")))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(PlatformError::Other(format!(
                "could not open {name}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    fn window_tree(&self, window: &WindowInfo, limits: &TreeLimits) -> Result<AxTree> {
        if !ax_is_trusted() {
            return Err(PlatformError::PermissionMissing(crate::platform::Permission::Accessibility));
        }
        set_global_timeout();
        // A window that just appeared (a sheet after cmd+w) can take a
        // moment to show up in the accessibility tree.
        let mut tries = 0;
        let win = loop {
            match ax_window(window) {
                Ok(w) => break w,
                Err(_) if tries < 3 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                Err(e) => return Err(e),
            }
        };
        let mut w = Walker::new(limits);
        w.walk(&win, 0)?;
        let app = unsafe { AXUIElement::new_application(window.pid) };
        w.menus(&app);
        Ok(AxTree { nodes: w.nodes, truncated: w.truncated })
    }

    fn element_frame(&self, el: &ElementHandle) -> Result<Option<Rect>> {
        let el = el_of(el)?;
        // Role first: it fails with InvalidUIElement once the element is gone.
        if attr(el, "AXRole")?.is_none() {
            return Err(PlatformError::Stale);
        }
        let pos = attr(el, "AXPosition")?;
        let size = attr(el, "AXSize")?;
        Ok(frame_of(pos.as_deref(), size.as_deref()))
    }

    fn press(&self, el: &ElementHandle) -> Result<()> {
        let el = el_of(el)?;
        let err = unsafe { el.perform_action(&cfstr("AXPress")) };
        match err {
            AXError::Success => Ok(()),
            AXError::InvalidUIElement => Err(PlatformError::Stale),
            e => Err(PlatformError::Other(format!("AXPress failed (AXError {})", e.0))),
        }
    }

    fn element_at(&self, at: Point) -> Result<Option<HitElement>> {
        set_global_timeout();
        let sys = unsafe { AXUIElement::new_system_wide() };
        let mut out: *const AXUIElement = std::ptr::null();
        let err = unsafe { sys.copy_element_at_position(at.x as f32, at.y as f32, NonNull::from(&mut out)) };
        if err != AXError::Success {
            return Ok(None);
        }
        let Some(el) = NonNull::new(out as *mut AXUIElement).map(|p| unsafe { CFRetained::from_raw(p) }) else {
            return Ok(None);
        };
        let Some(pid) = element_pid(&el) else { return Ok(None) };
        let role = attr_opt(&el, "AXRole").and_then(|v| as_string(&v)).unwrap_or_default();
        let name = attr_opt(&el, "AXTitle")
            .and_then(|v| as_string(&v))
            .or_else(|| attr_opt(&el, "AXDescription").and_then(|v| as_string(&v)))
            .unwrap_or_default();
        Ok(Some(HitElement { pid, role, name }))
    }

    fn raise_window(&self, window: &WindowInfo) -> Result<()> {
        let app = unsafe { AXUIElement::new_application(window.pid) };
        let yes: &CFType = CFBoolean::new(true);
        unsafe {
            app.set_attribute_value(&cfstr("AXFrontmost"), yes);
        }
        if let Ok(win) = ax_window(window) {
            unsafe {
                win.set_attribute_value(&cfstr("AXMain"), yes);
                win.perform_action(&cfstr("AXRaise"));
            }
        }
        if let Some(a) = ns_app(window.pid) {
            a.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        }
        Ok(())
    }

    fn focused_is_secure(&self) -> Result<bool> {
        let sys = unsafe { AXUIElement::new_system_wide() };
        let Some(el) = attr_opt(&sys, "AXFocusedUIElement").and_then(|v| v.downcast::<AXUIElement>().ok()) else {
            return Ok(false);
        };
        let role = attr_opt(&el, "AXRole").and_then(|v| as_string(&v)).unwrap_or_default();
        let sub = attr_opt(&el, "AXSubrole").and_then(|v| as_string(&v)).unwrap_or_default();
        Ok(role == "AXSecureTextField" || sub == "AXSecureTextField")
    }
}

#[cfg(test)]
mod tests {
    use super::format_shortcut;

    #[test]
    fn shortcut_text() {
        assert_eq!(format_shortcut("N", 0), "⌘N");
        assert_eq!(format_shortcut("S", 1), "⇧⌘S");
        assert_eq!(format_shortcut("F", 4 | 8), "⌃F");
    }
}
