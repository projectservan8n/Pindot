//! The element tree through UI Automation: what is in a window
//! (`read_window`, `find`), where an element is, pressing it without the
//! pointer, and whether keyboard focus is in a password field.
//!
//! A window is walked one level at a time. Each `FindAllBuildCache` call
//! fetches a node's children with every property we show already cached, so
//! a node costs one trip to the other process, and the walk can stop at the
//! limits (nodes, depth, time) between trips. Only the control view is
//! walked: what a user sees as controls, without layout scaffolding.
//!
//! Roles are translated to the macOS names `server::tree` is written in
//! (`windows_logic::uia_role`). A password's value is never read.
//!
//! Threads: UI Automation is used from worker threads in the multithreaded
//! apartment; each thread joins it before its first call. Element handles
//! are plain COM pointers that any of those threads may use.
//!
//! `Invoke` can block while the dialog a button opens is up (classic
//! controls), so a press runs on a helper thread and is given two seconds:
//! past that it counts as done. A failed press is reported and never retried
//! as a real click (it can fail after it worked).

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_Collapsed, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationCondition,
    IUIAutomationElement, IUIAutomationElement3, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, TreeScope_Children, UIA_AcceleratorKeyPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_ExpandCollapsePatternId, UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId,
    UIA_IsEnabledPropertyId, UIA_IsPasswordPropertyId, UIA_LegacyIAccessiblePatternId, UIA_NamePropertyId,
    UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};

use crate::platform::windows_logic::{
    Patterns, PressAction, UIA_MENU_BAR, click_presses, menu_shows_at, press_action, shows_menu_on_request,
    uia_has_value, uia_role, visible_text, web_tree_pending,
};
use crate::platform::{AxNode, AxTree, ElementHandle, FocusedPage, PlatformError, Rect, Result, TreeLimits};

/// How long a press may take before it counts as done.
const PRESS_WAIT: Duration = Duration::from_secs(2);
/// The longest value shown for a field.
const VALUE_MAX: usize = 400;
/// A browser builds a page's tree only once asked: how long to wait
/// between walks for it, and how many more walks to try.
const WEB_WAIT: Duration = Duration::from_millis(350);
const WEB_RETRIES: u32 = 4;
/// A browser window whose page never showed up is not waited for again
/// this long.
const WEB_GIVE_UP: Duration = Duration::from_secs(30);

/// A COM pointer that worker threads share (all in the multithreaded
/// apartment, see the module comment).
struct Shared<T>(T);
// SAFETY: UI Automation's client objects are free-threaded in the
// multithreaded apartment, which every thread that uses them has joined.
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

fn err(e: windows::core::Error) -> PlatformError {
    // UIA_E_ELEMENTNOTAVAILABLE: the element is gone.
    if e.code().0 as u32 == 0x8004_0201 { PlatformError::Stale } else { PlatformError::Other(e.message()) }
}

/// Join the multithreaded apartment on this thread, once.
fn join_apartment() {
    thread_local! {
        static JOINED: Cell<bool> = const { Cell::new(false) };
    }
    JOINED.with(|j| {
        if !j.replace(true) {
            // SAFETY: plain COM initialisation; a thread that is already in
            // another apartment keeps it (the call then fails harmlessly).
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        }
    });
}

fn automation() -> Result<&'static IUIAutomation> {
    static AUTOMATION: OnceLock<Option<Shared<IUIAutomation>>> = OnceLock::new();
    join_apartment();
    AUTOMATION
        // SAFETY: creating the system's UI Automation client object.
        .get_or_init(|| unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok().map(Shared))
        .as_ref()
        .map(|s| &s.0)
        .ok_or_else(|| PlatformError::Other("UI Automation is not available on this computer".into()))
}

fn handle_of(el: IUIAutomationElement) -> ElementHandle {
    Arc::new(Shared(el))
}

fn el_of(h: &ElementHandle) -> Result<&IUIAutomationElement> {
    join_apartment();
    h.downcast_ref::<Shared<IUIAutomationElement>>()
        .map(|s| &s.0)
        .ok_or_else(|| PlatformError::Other("not an element of this window".into()))
}

/// A rectangle in global points: on Windows a point is a pixel.
fn frame(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect::new(f64::from(r.left), f64::from(r.top), f64::from(r.right - r.left), f64::from(r.bottom - r.top))
}

struct Walk<'a> {
    cache: IUIAutomationCacheRequest,
    children: IUIAutomationCondition,
    limits: &'a TreeLimits,
    started: Instant,
    nodes: Vec<AxNode>,
    /// Control type and depth of each node, for `web_tree_pending`.
    kinds: Vec<(i32, u16)>,
    truncated: bool,
}

impl Walk<'_> {
    fn over_budget(&mut self) -> bool {
        if self.nodes.len() >= self.limits.max_nodes || self.started.elapsed() > self.limits.timeout {
            self.truncated = true;
        }
        self.truncated
    }

    /// One element, from the properties cached when it was fetched.
    fn node(&self, el: &IUIAutomationElement, depth: u16, in_menu_bar: bool) -> (AxNode, i32) {
        // SAFETY: reads of cached properties; a property that was not
        // cached or has no value comes back as an error and gets a default.
        unsafe {
            let control_type = el.CachedControlType().map_or(0, |c| c.0);
            let password = el.CachedIsPassword().is_ok_and(|b| b.as_bool());
            let (role, subrole) = uia_role(control_type, password, in_menu_bar);
            let value = uia_has_value(control_type, password)
                .then(|| el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok())
                .flatten()
                .and_then(|p| p.CurrentValue().ok())
                .map(|v| visible_text(&v.to_string()))
                .filter(|v| !v.is_empty())
                .map(|v| v.chars().take(VALUE_MAX).collect());
            let node = AxNode {
                handle: handle_of(el.clone()),
                depth,
                role: role.into(),
                subrole: subrole.map(String::from),
                name: el.CachedName().map(|n| visible_text(&n.to_string())).unwrap_or_default(),
                value,
                // An element with no place on screen reports an empty
                // rectangle; it keeps it, so the listing leaves it out.
                bounds: el.CachedBoundingRectangle().ok().map(frame),
                enabled: el.CachedIsEnabled().map_or(true, |b| b.as_bool()),
                focused: el.CachedHasKeyboardFocus().is_ok_and(|b| b.as_bool()),
                shortcut: el.CachedAcceleratorKey().ok().map(|k| k.to_string()).filter(|k| !k.is_empty()),
                menu_path: None,
                menu_root: None,
            };
            (node, control_type)
        }
    }

    fn walk(&mut self, el: &IUIAutomationElement, depth: u16, in_menu_bar: bool) {
        if self.over_budget() {
            return;
        }
        let (node, control_type) = self.node(el, depth, in_menu_bar);
        self.nodes.push(node);
        self.kinds.push((control_type, depth));
        if depth >= self.limits.max_depth {
            self.truncated = true;
            return;
        }
        // SAFETY: one call that fetches the children with their properties.
        // An element that vanished mid-walk simply has no children.
        let Ok(kids) = (unsafe { el.FindAllBuildCache(TreeScope_Children, &self.children, &self.cache) }) else { return };
        let count = unsafe { kids.Length() }.unwrap_or(0);
        for i in 0..count {
            if self.over_budget() {
                break;
            }
            if let Ok(kid) = unsafe { kids.GetElement(i) } {
                self.walk(&kid, depth + 1, control_type == UIA_MENU_BAR);
            }
        }
    }
}

/// Browser windows whose page did not show up after the retries, and when.
static NO_PAGE: Mutex<Option<HashMap<usize, Instant>>> = Mutex::new(None);

/// The element tree of the window `hwnd` (of window class `class`).
/// A browser window is walked again, a few times, until its page is in
/// the tree.
pub fn window_tree(hwnd: usize, class: &str, limits: &TreeLimits) -> Result<AxTree> {
    let gave_up = NO_PAGE.lock().unwrap().as_ref().and_then(|m| m.get(&hwnd).copied()).is_some_and(|t| t.elapsed() < WEB_GIVE_UP);
    let started = Instant::now();
    let mut tries = 0;
    loop {
        let (tree, kinds) = walk_window(hwnd, limits)?;
        if !web_tree_pending(class, &kinds) {
            if let Some(m) = NO_PAGE.lock().unwrap().as_mut() {
                m.remove(&hwnd);
            }
            return Ok(tree);
        }
        tries += 1;
        if gave_up || tries > WEB_RETRIES || started.elapsed() + WEB_WAIT > limits.timeout {
            NO_PAGE.lock().unwrap().get_or_insert_with(HashMap::new).insert(hwnd, Instant::now());
            return Ok(tree);
        }
        std::thread::sleep(WEB_WAIT);
    }
}

fn walk_window(hwnd: usize, limits: &TreeLimits) -> Result<(AxTree, Vec<(i32, u16)>)> {
    let ui = automation()?;
    // SAFETY: COM calls on valid interface pointers; errors are returned.
    unsafe {
        let cache = ui.CreateCacheRequest().map_err(err)?;
        for property in [
            UIA_NamePropertyId,
            UIA_ControlTypePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_IsEnabledPropertyId,
            UIA_HasKeyboardFocusPropertyId,
            UIA_IsPasswordPropertyId,
            UIA_AcceleratorKeyPropertyId,
        ] {
            cache.AddProperty(property).map_err(err)?;
        }
        let children = ui.ControlViewCondition().map_err(err)?;
        let root = ui.ElementFromHandleBuildCache(HWND(hwnd as *mut c_void), &cache).map_err(err)?;
        let mut walk = Walk {
            cache,
            children,
            limits,
            started: Instant::now(),
            nodes: Vec::new(),
            kinds: Vec::new(),
            truncated: false,
        };
        walk.walk(&root, 0, false);
        Ok((AxTree { nodes: walk.nodes, truncated: walk.truncated }, walk.kinds))
    }
}

/// Where an element is now. `Err(Stale)` if it is gone, `Ok(None)` if it
/// has no place on screen.
pub fn element_frame(h: &ElementHandle) -> Result<Option<Rect>> {
    let el = el_of(h)?;
    // SAFETY: a COM call on a valid interface pointer.
    let rect = unsafe { el.CurrentBoundingRectangle() }.map_err(err)?;
    // An empty rectangle: the element has no place on screen.
    let empty = rect.right <= rect.left && rect.bottom <= rect.top;
    Ok((!empty).then(|| frame(rect)))
}

/// The element's kind and what it offers. An element that is gone fails
/// the first query.
fn offers(el: &IUIAutomationElement) -> Result<(i32, Patterns)> {
    // SAFETY: COM calls on a valid interface pointer.
    unsafe {
        let control_type = el.CurrentControlType().map_err(err)?.0;
        let can = Patterns {
            invoke: el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).is_ok(),
            toggle: el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId).is_ok(),
            select: el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).is_ok(),
            expand: el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).is_ok(),
            legacy: el
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId)
                .is_ok(),
        };
        Ok((control_type, can))
    }
}

/// Does the element offer an action that amounts to a press?
pub fn can_press(h: &ElementHandle) -> Result<bool> {
    let (control_type, can) = offers(el_of(h)?)?;
    // The old default action is a last resort of `press`, not a promise.
    Ok(press_action(control_type, Patterns { legacy: false, ..can }).is_some())
}

/// What one click would do, without the pointer: select a row, a tab or a
/// radio button; invoke a button or a link; toggle; open or close.
fn press_now(el: &IUIAutomationElement) -> Result<()> {
    let (control_type, can) = offers(el)?;
    let Some(action) = press_action(control_type, can) else {
        return Err(PlatformError::Other("that element cannot be pressed; click it at its position instead".into()));
    };
    // SAFETY: COM calls on a valid interface pointer.
    unsafe {
        match action {
            PressAction::Invoke => el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).and_then(|p| p.Invoke()),
            PressAction::Toggle => el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId).and_then(|p| p.Toggle()),
            PressAction::Select => el
                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
                .and_then(|p| p.Select()),
            PressAction::ExpandCollapse => {
                el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).and_then(|p| {
                    let collapsed = p.CurrentExpandCollapseState().is_ok_and(|s| s == ExpandCollapseState_Collapsed);
                    if collapsed { p.Expand() } else { p.Collapse() }
                })
            }
            PressAction::DefaultAction => el
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId)
                .and_then(|p| p.DoDefaultAction()),
        }
        .map_err(err)
    }
}

/// Press an element without the pointer (see the module comment on time).
pub fn press(h: &ElementHandle) -> Result<()> {
    let el = Shared(el_of(h)?.clone());
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pindot-uia-press".into())
        .spawn(move || {
            join_apartment();
            let el = el;
            let _ = tx.send(press_now(&el.0));
        })
        .map_err(|e| PlatformError::Other(e.to_string()))?;
    match rx.recv_timeout(PRESS_WAIT) {
        Ok(result) => result,
        // Still inside the press: a dialog it opened is up. It was pressed.
        Err(_) => Ok(()),
    }
}

/// Show the element's context menu without the pointer: what a
/// right-click on it, or the menu key, brings up. On a helper thread, as a
/// press is: the call can stay inside the menu while it is open.
pub fn show_menu(h: &ElementHandle) -> Result<()> {
    let el = Shared(el_of(h)?.clone());
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pindot-uia-menu".into())
        .spawn(move || {
            join_apartment();
            let el = el;
            // SAFETY: COM calls on a valid interface pointer.
            let shown = unsafe { el.0.cast::<IUIAutomationElement3>().and_then(|e| e.ShowContextMenu()) };
            let _ = tx.send(shown.map_err(err));
        })
        .map_err(|e| PlatformError::Other(e.to_string()))?;
    match rx.recv_timeout(PRESS_WAIT) {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

/// Does the element show its menu when asked? One the keyboard can reach
/// (the menu key is what the request stands in for), in a UI framework
/// that was seen to answer (`windows_logic::shows_menu_on_request`).
fn answers_menu_request(el: &IUIAutomationElement) -> bool {
    // SAFETY: COM calls on a valid interface pointer.
    unsafe {
        el.CurrentIsEnabled().is_ok_and(|b| b.as_bool())
            && el.CurrentIsKeyboardFocusable().is_ok_and(|b| b.as_bool())
            && el.CurrentFrameworkId().is_ok_and(|f| shows_menu_on_request(&f.to_string()))
            && el.cast::<IUIAutomationElement3>().is_ok()
    }
}

/// Can the element be asked for its context menu?
pub fn can_show_menu(h: &ElementHandle) -> Result<bool> {
    let el = el_of(h)?;
    // SAFETY: a COM call on a valid interface pointer. An element that is
    // gone fails it.
    unsafe { el.CurrentControlType() }.map_err(err)?;
    Ok(answers_menu_request(el))
}

/// The control at a screen pixel that takes a click without the pointer,
/// with its role and name: the element there or one of its three nearest
/// ancestors. `menu` false: one a plain click presses
/// (`windows_logic::click_presses`); true: one a right-click can ask for
/// its context menu (`menu_shows_at`). `None`: click for real.
pub fn target_at(x: i32, y: i32, menu: bool) -> Result<Option<(ElementHandle, String, String)>> {
    let ui = automation()?;
    // SAFETY: COM calls on valid interface pointers. An element that
    // vanishes on the way simply ends the search.
    unsafe {
        let Ok(mut el) = ui.ElementFromPoint(windows::Win32::Foundation::POINT { x, y }) else { return Ok(None) };
        let walker = ui.ControlViewWalker().map_err(err)?;
        for _ in 0..4 {
            let Ok((control_type, can)) = offers(&el) else { return Ok(None) };
            let enabled = el.CurrentIsEnabled().is_ok_and(|b| b.as_bool());
            let covers =
                el.CurrentBoundingRectangle().is_ok_and(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom);
            let fits = if menu {
                menu_shows_at(control_type, true, enabled) && answers_menu_request(&el)
            } else {
                click_presses(control_type, can, enabled)
            };
            if covers && fits {
                let (role, _) = uia_role(control_type, false, false);
                let name = el.CurrentName().map(|n| visible_text(&n.to_string())).unwrap_or_default();
                return Ok(Some((handle_of(el), role.to_string(), name)));
            }
            match walker.GetParentElement(&el) {
                Ok(parent) => el = parent,
                Err(_) => break,
            }
        }
    }
    Ok(None)
}

/// Is keyboard focus in a password field (of any kind of app)?
pub fn focused_is_password() -> Result<bool> {
    let ui = automation()?;
    // SAFETY: COM calls on valid interface pointers.
    unsafe {
        let focused = ui.GetFocusedElement().map_err(err)?;
        Ok(focused.CurrentIsPassword().map_err(err)?.as_bool())
    }
}

/// The focused field and the address of every web document above it.
/// Browsers expose a page's address as the value of its Document element
/// (frames are Documents of their own). A document whose address can't be
/// read is kept as "" so the caller's exact-match check fails on it.
pub fn focused_page() -> Result<Option<FocusedPage>> {
    let ui = automation()?;
    // SAFETY: COM calls on valid interface pointers; an element that vanishes
    // on the way simply ends the walk.
    unsafe {
        let Ok(focused) = ui.GetFocusedElement() else { return Ok(None) };
        let pid = focused.CurrentProcessId().map_err(err)?;
        let secure = focused.CurrentIsPassword().map_err(err)?.as_bool();
        let kind = focused.CurrentControlType().map_err(err)?;
        let text_input = secure || kind == UIA_EditControlTypeId;
        let walker = ui.ControlViewWalker().map_err(err)?;
        let mut urls: Vec<String> = Vec::new();
        let mut cur = Some(focused);
        for _ in 0..200 {
            let Some(el) = cur else { break };
            if el.CurrentControlType().is_ok_and(|k| k == UIA_DocumentControlTypeId) {
                let url = el
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .ok()
                    .and_then(|p| p.CurrentValue().ok())
                    .map(|b| b.to_string())
                    .unwrap_or_default();
                urls.push(url);
            }
            cur = walker.GetParentElement(&el).ok();
        }
        urls.reverse();
        Ok(Some(FocusedPage { pid: pid as i32, secure, text_input, page_urls: urls }))
    }
}
