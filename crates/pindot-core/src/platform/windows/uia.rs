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

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_Collapsed, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationCondition,
    IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, TreeScope_Children, UIA_AcceleratorKeyPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId, UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId,
    UIA_IsEnabledPropertyId, UIA_IsPasswordPropertyId, UIA_LegacyIAccessiblePatternId, UIA_NamePropertyId,
    UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};

use crate::platform::windows_logic::{
    Patterns, PressAction, UIA_MENU_BAR, press_action, uia_has_value, uia_role, visible_text, web_tree_pending,
};
use crate::platform::{AxNode, AxTree, ElementHandle, PlatformError, Rect, Result, TreeLimits};

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

fn logical(r: windows::Win32::Foundation::RECT, scale: f64) -> Rect {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    Rect::new(f64::from(r.left) / scale, f64::from(r.top) / scale, f64::from(w) / scale, f64::from(h) / scale)
}

struct Walk<'a> {
    cache: IUIAutomationCacheRequest,
    children: IUIAutomationCondition,
    limits: &'a TreeLimits,
    scale: f64,
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
                bounds: el.CachedBoundingRectangle().ok().map(|r| logical(r, self.scale)),
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
/// `scale` is its monitor's scale. A browser window is walked again, a
/// few times, until its page is in the tree.
pub fn window_tree(hwnd: usize, class: &str, scale: f64, limits: &TreeLimits) -> Result<AxTree> {
    let gave_up = NO_PAGE.lock().unwrap().as_ref().and_then(|m| m.get(&hwnd).copied()).is_some_and(|t| t.elapsed() < WEB_GIVE_UP);
    let started = Instant::now();
    let mut tries = 0;
    loop {
        let (tree, kinds) = walk_window(hwnd, scale, limits)?;
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

fn walk_window(hwnd: usize, scale: f64, limits: &TreeLimits) -> Result<(AxTree, Vec<(i32, u16)>)> {
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
            scale,
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
pub fn element_frame(h: &ElementHandle, scale: f64) -> Result<Option<Rect>> {
    let el = el_of(h)?;
    // SAFETY: a COM call on a valid interface pointer.
    let rect = unsafe { el.CurrentBoundingRectangle() }.map_err(err)?;
    // An empty rectangle: the element has no place on screen.
    let empty = rect.right <= rect.left && rect.bottom <= rect.top;
    Ok((!empty).then(|| logical(rect, scale)))
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

/// Is keyboard focus in a password field (of any kind of app)?
pub fn focused_is_password() -> Result<bool> {
    let ui = automation()?;
    // SAFETY: COM calls on valid interface pointers.
    unsafe {
        let focused = ui.GetFocusedElement().map_err(err)?;
        Ok(focused.CurrentIsPassword().map_err(err)?.as_bool())
    }
}
