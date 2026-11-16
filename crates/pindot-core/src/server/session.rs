//! Per-MCP-session state: approved apps, the latest screenshot geometry,
//! element refs and cached tree snapshots. One `claude` process = one MCP
//! session = one `Session`. Dropping the session releases the input lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use super::control::Control;
use super::tree::Filter;
use crate::coords::ShotGeometry;
use crate::platform::{Accessibility, AxNode, ElementHandle, Point, Rect, WindowInfo};

/// Refs kept per session; the oldest are forgotten first.
pub const MAX_REFS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApprovedApp {
    pub name: String,
    pub bundle_id: Option<String>,
}

impl ApprovedApp {
    pub fn matches(&self, name: &str, bundle_id: Option<&str>) -> bool {
        match (&self.bundle_id, bundle_id) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => self.name.eq_ignore_ascii_case(name),
        }
    }
}

/// What a ref points at.
#[derive(Clone)]
pub struct RefEntry {
    pub handle: ElementHandle,
    pub pid: i32,
    pub app: String,
    pub window_id: u64,
    pub role: String,
    pub name: String,
    /// "File > New" for menu items.
    pub menu_path: Option<String>,
    /// The top-level menu bar item of a menu item (where the cursor goes).
    pub menu_root: Option<ElementHandle>,
    /// Bounds when the tree was read.
    pub bounds: Option<Rect>,
}

#[derive(Default)]
pub struct RefTable {
    next: u64,
    map: HashMap<u64, RefEntry>,
    order: VecDeque<u64>,
}

impl RefTable {
    pub fn insert(&mut self, e: RefEntry) -> String {
        self.next += 1;
        let id = self.next;
        self.map.insert(id, e);
        self.order.push_back(id);
        while self.order.len() > MAX_REFS {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        format!("e{id}")
    }

    pub fn get(&self, r: &str) -> Option<RefEntry> {
        let id: u64 = r.trim().trim_start_matches('[').trim_end_matches(']').strip_prefix('e')?.parse().ok()?;
        self.map.get(&id).cloned()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A tree read, kept briefly so later pages match the first.
pub struct Snapshot {
    pub taken: Instant,
    pub window: WindowInfo,
    pub filter: Filter,
    pub include_menus: bool,
    pub truncated: bool,
    /// (ref, node) for every listed element.
    pub items: Vec<(String, AxNode)>,
}

pub struct Session {
    pub id: String,
    control: Arc<Control>,
    approved: Mutex<Vec<ApprovedApp>>,
    /// Vault logins the user allowed for this session (by login id).
    approved_logins: Mutex<HashSet<String>>,
    /// Rate limit and lock for `vault_fill`.
    pub fill_gate: crate::vault::gate::FillGate,
    pub geometry: Mutex<Option<ShotGeometry>>,
    pub refs: Mutex<RefTable>,
    pub snapshots: Mutex<HashMap<u64, Snapshot>>,
    pub screenshots: AtomicU32,
}

impl Session {
    pub fn new(control: Arc<Control>) -> Arc<Self> {
        Self::with_id(control, uuid::Uuid::new_v4().simple().to_string()[..8].to_string())
    }

    pub fn with_id(control: Arc<Control>, id: String) -> Arc<Self> {
        Arc::new(Self {
            id,
            control,
            approved: Mutex::new(vec![]),
            approved_logins: Mutex::new(HashSet::new()),
            fill_gate: Default::default(),
            geometry: Mutex::new(None),
            refs: Mutex::new(RefTable::default()),
            snapshots: Mutex::new(HashMap::new()),
            screenshots: AtomicU32::new(0),
        })
    }

    pub fn approve(&self, app: ApprovedApp) {
        let mut a = self.approved.lock().unwrap();
        if !a.iter().any(|x| x.matches(&app.name, app.bundle_id.as_deref())) {
            a.push(app);
        }
    }

    pub fn approve_login(&self, id: &str) {
        self.approved_logins.lock().unwrap().insert(id.to_string());
    }

    pub fn is_login_approved(&self, id: &str) -> bool {
        self.approved_logins.lock().unwrap().contains(id)
    }

    pub fn approved(&self) -> Vec<ApprovedApp> {
        self.approved.lock().unwrap().clone()
    }

    pub fn is_approved_app(&self, name: &str, bundle_id: Option<&str>) -> bool {
        if super::approval::is_pindot(name, bundle_id) {
            return false;
        }
        self.approved.lock().unwrap().iter().any(|a| a.matches(name, bundle_id))
    }

    /// Is the app with this pid approved? Pindot itself never is.
    pub fn is_approved_pid(&self, pid: i32, own_pid: i32, ax: &dyn Accessibility) -> bool {
        if pid == own_pid {
            return false;
        }
        match ax.app_info(pid) {
            Some(a) => self.is_approved_app(&a.name, a.bundle_id.as_deref()),
            None => false,
        }
    }

    pub fn geometry(&self) -> Option<ShotGeometry> {
        self.geometry.lock().unwrap().clone()
    }

    pub fn set_geometry(&self, g: ShotGeometry) {
        *self.geometry.lock().unwrap() = Some(g);
    }

    /// An image pixel of the latest screenshot to a global point.
    pub fn image_to_global(&self, x: f64, y: f64) -> Result<Point, String> {
        let Some(g) = self.geometry() else {
            return Err("No screenshot coordinate space yet. Use refs from read_window, or take a screenshot first.".into());
        };
        if !g.in_image(x, y) {
            return Err(format!(
                "({x}, {y}) is outside the latest screenshot ({}x{} px).",
                g.image_width, g.image_height
            ));
        }
        Ok(g.to_global(x, y))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.control.release(&self.id);
    }
}

/// Is `win` a Pindot overlay (a click-through panel covering a whole
/// display)? Those are ignored by the hit test; any other Pindot window
/// (approval prompt, activity view) blocks a click.
pub fn is_overlay(win: &WindowInfo, displays: &[crate::platform::DisplayInfo]) -> bool {
    displays.iter().any(|d| {
        let b = d.bounds;
        (win.bounds.x - b.x).abs() < 1.0
            && (win.bounds.y - b.y).abs() < 1.0
            && (win.bounds.width - b.width).abs() < 1.0
            && (win.bounds.height - b.height).abs() < 1.0
    })
}

/// Window-server owners whose windows are part of the desktop furniture.
pub const SYSTEM_OWNERS: &[&str] =
    &["Window Server", "Dock", "SystemUIServer", "Control Center", "WindowManager", "TextInputMenuAgent"];

/// Menu bar and status item layers.
pub fn is_menu_bar_layer(layer: i32) -> bool {
    layer == 24 || layer == 25
}

/// Where a click at global `p` would land, and whether it may. `windows` is
/// front to back. `menu_hit` returns the pid and name of the accessibility
/// element under the point; it is asked only for the menu bar and for
/// full-display system windows (the Dock).
pub fn hit_test(
    windows: &[WindowInfo],
    displays: &[crate::platform::DisplayInfo],
    p: Point,
    own_pid: i32,
    is_approved: &dyn Fn(i32) -> bool,
    menu_hit: &dyn Fn() -> Option<(i32, String)>,
) -> Result<i32, String> {
    for w in windows {
        if w.alpha < 0.05 || !w.bounds.contains(p) {
            continue;
        }
        if w.pid == own_pid {
            if is_overlay(w, displays) {
                continue;
            }
            return Err("That point is on a Pindot window (approval prompt or activity view). Claude may not click Pindot itself.".into());
        }
        if is_approved(w.pid) {
            return Ok(w.pid);
        }
        if SYSTEM_OWNERS.contains(&w.app.as_str()) && !is_menu_bar_layer(w.layer) && is_overlay(w, displays) {
            // A mostly transparent system window over a whole display (on
            // macOS 26 the Dock's window spans the screen). Ask the
            // accessibility hit test what is really under the point.
            match menu_hit() {
                Some((pid, _)) if pid == w.pid => {
                    return Err(format!("That point is on the {}, which Claude may not use.", w.app));
                }
                _ => continue,
            }
        }
        if is_menu_bar_layer(w.layer) {
            return match menu_hit() {
                Some((_, name)) if name == "Apple" => {
                    Err("That point is on the Apple menu, which Claude may not use.".into())
                }
                Some((pid, _)) if pid != own_pid && is_approved(pid) => Ok(pid),
                Some((_, name)) => Err(format!(
                    "That point is on the menu bar item \"{name}\" of an app that is not approved. Use refs from read_window for the approved app's menus."
                )),
                None => Err("That point is on the menu bar, not on an approved app's menu.".into()),
            };
        }
        let app = if w.app.is_empty() { "another app".to_string() } else { w.app.clone() };
        return Err(format!(
            "That point is on a window of {app}, which is not approved for this session. Call request_access for it first, or pick a point inside an approved app's window."
        ));
    }
    Err("That point is on the desktop, not on an approved app's window.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::DisplayInfo;

    fn win(id: u64, pid: i32, app: &str, b: Rect, layer: i32) -> WindowInfo {
        WindowInfo { id, pid, app: app.into(), title: String::new(), bounds: b, layer, alpha: 1.0 }
    }

    fn displays() -> Vec<DisplayInfo> {
        vec![DisplayInfo {
            id: 1,
            name: "d".into(),
            bounds: Rect::new(0.0, 0.0, 1440.0, 900.0),
            scale_factor: 2.0,
            is_primary: true,
        }]
    }

    #[test]
    fn ref_table_parses_and_evicts() {
        let mut t = RefTable::default();
        let e = RefEntry {
            handle: Arc::new(()),
            pid: 1,
            app: "A".into(),
            window_id: 1,
            role: "AXButton".into(),
            name: "OK".into(),
            menu_path: None,
            menu_root: None,
            bounds: None,
        };
        let r = t.insert(e.clone());
        assert_eq!(r, "e1");
        assert_eq!(t.get("e1").unwrap().name, "OK");
        assert_eq!(t.get("[e1]").unwrap().name, "OK");
        assert!(t.get("e2").is_none());
        assert!(t.get("x1").is_none());
        for _ in 0..MAX_REFS {
            t.insert(e.clone());
        }
        assert_eq!(t.len(), MAX_REFS);
        assert!(t.get("e1").is_none(), "oldest ref evicted");
    }

    #[test]
    fn hit_test_only_lands_on_approved_windows() {
        let own = 99;
        let approved = |pid: i32| pid == 10;
        let no_menu = || None;
        let ws = vec![
            win(1, own, "Pindot", Rect::new(0.0, 0.0, 1440.0, 900.0), 1000), // overlay: ignored
            win(2, own, "Pindot", Rect::new(500.0, 300.0, 400.0, 200.0), 3), // approval prompt
            win(3, 20, "Chrome", Rect::new(800.0, 0.0, 600.0, 600.0), 0),
            win(4, 10, "TextEdit", Rect::new(0.0, 0.0, 900.0, 700.0), 0),
            win(5, 0, "Window Server", Rect::new(0.0, 0.0, 1440.0, 24.0), 24),
        ];
        let d = displays();
        // Inside TextEdit, not covered by anything else.
        assert_eq!(hit_test(&ws, &d, Point::new(100.0, 400.0), own, &approved, &no_menu), Ok(10));
        // Our own approval prompt blocks.
        assert!(hit_test(&ws, &d, Point::new(600.0, 350.0), own, &approved, &no_menu).unwrap_err().contains("Pindot"));
        // Chrome covers TextEdit there: refused, naming Chrome.
        assert!(hit_test(&ws, &d, Point::new(850.0, 200.0), own, &approved, &no_menu).unwrap_err().contains("Chrome"));
        // Desktop.
        assert!(hit_test(&ws, &d, Point::new(1000.0, 800.0), own, &approved, &no_menu).unwrap_err().contains("desktop"));
    }

    #[test]
    fn a_full_screen_dock_window_is_looked_through() {
        let own = 99;
        let approved = |pid: i32| pid == 10;
        let ws = vec![
            win(8, 433, "Dock", Rect::new(0.0, 0.0, 1440.0, 900.0), 20),
            win(4, 10, "TextEdit", Rect::new(0.0, 0.0, 900.0, 700.0), 0),
        ];
        let d = displays();
        // AX says the point is a TextEdit button: the click goes through.
        let on_textedit = || Some((10, "New Document".into()));
        assert_eq!(hit_test(&ws, &d, Point::new(100.0, 400.0), own, &approved, &on_textedit), Ok(10));
        // AX says it is a Dock icon: refused.
        let on_dock = || Some((433, "Finder".into()));
        assert!(hit_test(&ws, &d, Point::new(100.0, 880.0), own, &approved, &on_dock).unwrap_err().contains("Dock"));
        // A Dock window that is not full-screen still blocks like any window.
        let ws2 = vec![win(8, 433, "Dock", Rect::new(0.0, 820.0, 1440.0, 80.0), 20)];
        assert!(hit_test(&ws2, &d, Point::new(100.0, 850.0), own, &approved, &on_textedit).unwrap_err().contains("Dock"));
    }

    #[test]
    fn menu_bar_needs_an_approved_app_and_never_the_apple_menu() {
        let own = 99;
        let approved = |pid: i32| pid == 10;
        let ws = vec![win(5, 0, "Window Server", Rect::new(0.0, 0.0, 1440.0, 24.0), 24)];
        let d = displays();
        let p = Point::new(60.0, 10.0);
        assert_eq!(hit_test(&ws, &d, p, own, &approved, &|| Some((10, "File".into()))), Ok(10));
        assert!(hit_test(&ws, &d, p, own, &approved, &|| Some((10, "Apple".into()))).unwrap_err().contains("Apple"));
        assert!(hit_test(&ws, &d, p, own, &approved, &|| Some((20, "View".into()))).is_err());
    }

    #[test]
    fn approval_matches_bundle_then_name_and_never_pindot() {
        let ax = crate::platform::mock::MockAccessibility::default();
        ax.apps.lock().unwrap().push(crate::platform::AppInfo {
            pid: 10,
            name: "TextEdit".into(),
            bundle_id: Some("com.apple.TextEdit".into()),
        });
        let c = Control::new(
            Arc::new(crate::platform::mock::MockStopKey::default()),
            Arc::new(crate::platform::NoOverlay),
            Arc::new(super::super::activity::ActivityLog::memory()),
            std::time::Duration::from_secs(1),
        );
        let s = Session::new(c);
        assert!(!s.is_approved_pid(10, 99, &ax));
        s.approve(ApprovedApp { name: "textedit".into(), bundle_id: None });
        assert!(s.is_approved_pid(10, 99, &ax));
        assert!(!s.is_approved_pid(10, 10, &ax), "own pid never approved");
        s.approve(ApprovedApp { name: "Pindot".into(), bundle_id: None });
        assert!(!s.is_approved_app("Pindot", None));
    }
}
