//! Mocks for every platform trait, for tests and headless runs.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::*;

/// A fake single display with a solid-colour image.
pub struct MockCapture {
    pub displays: Vec<DisplayInfo>,
}

impl Default for MockCapture {
    fn default() -> Self {
        Self {
            displays: vec![DisplayInfo {
                id: 1,
                name: "Mock Retina".into(),
                bounds: Rect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 },
                scale_factor: 2.0,
                is_primary: true,
            }],
        }
    }
}

impl Capture for MockCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays.clone())
    }
    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame> {
        let d = self
            .displays
            .iter()
            .find(|d| d.id == display_id)
            .cloned()
            .ok_or_else(|| PlatformError::Other(format!("no display {display_id}")))?;
        let w = (d.bounds.width * d.scale_factor) as u32;
        let h = (d.bounds.height * d.scale_factor) as u32;
        Ok(CapturedFrame { display: d, image: image::RgbaImage::from_pixel(w, h, image::Rgba([40, 80, 120, 255])) })
    }
}

/// One recorded input action.
#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    Click(Point, MouseButton, u32),
    Move(Point),
    Type(String),
    Key(KeyCombo),
    Scroll(Point, i32, i32),
    Drag(Point, Point),
}

/// Records every input action instead of performing it.
#[derive(Default)]
pub struct MockInput {
    pub actions: Mutex<Vec<InputAction>>,
    /// The pretend pointer (`None`: unknown, as on Windows today). Real
    /// actions move it; `user_moves_to` puts it somewhere else right after
    /// a click, as if the user moved the mouse meanwhile.
    pub pointer: Mutex<Option<Point>>,
    pub user_moves_to: Mutex<Option<Point>>,
}

impl MockInput {
    pub fn clicks(&self) -> Vec<(Point, MouseButton, u32)> {
        self.actions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|a| match a {
                InputAction::Click(p, b, n) => Some((*p, *b, *n)),
                _ => None,
            })
            .collect()
    }
    fn push(&self, a: InputAction) -> Result<()> {
        let at = match &a {
            InputAction::Click(p, ..) | InputAction::Move(p) | InputAction::Scroll(p, ..) => Some(*p),
            InputAction::Drag(_, to) => Some(*to),
            _ => None,
        };
        let mut ptr = self.pointer.lock().unwrap();
        if let (Some(p), Some(at)) = (ptr.as_mut(), at) {
            *p = at;
            if matches!(a, InputAction::Click(..))
                && let Some(u) = *self.user_moves_to.lock().unwrap()
            {
                *p = u;
            }
        }
        self.actions.lock().unwrap().push(a);
        Ok(())
    }
}

impl Input for MockInput {
    fn click(&self, at: Point, button: MouseButton, count: u32) -> Result<()> {
        self.push(InputAction::Click(at, button, count))
    }
    fn move_to(&self, at: Point) -> Result<()> {
        self.push(InputAction::Move(at))
    }
    fn type_text(&self, text: &str) -> Result<()> {
        self.push(InputAction::Type(text.into()))
    }
    fn key(&self, combo: &KeyCombo) -> Result<()> {
        self.push(InputAction::Key(*combo))
    }
    fn scroll(&self, at: Point, dx: i32, dy: i32) -> Result<()> {
        self.push(InputAction::Scroll(at, dx, dy))
    }
    fn drag(&self, from: Point, to: Point, _should_stop: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        self.push(InputAction::Drag(from, to))
    }
    fn pointer(&self) -> Option<Point> {
        *self.pointer.lock().unwrap()
    }
}

/// Emits a scripted sequence of hotkey events.
pub struct MockHotkey {
    pub script: Vec<HotkeyEvent>,
}

impl Hotkey for MockHotkey {
    fn start(&self, _binding: HotkeyBinding, events: std::sync::mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        for e in &self.script {
            let _ = events.send(*e);
        }
        Ok(ListenerHandle::new(|| {}))
    }
}

/// A stop key whose "press" is triggered by the test.
#[derive(Default)]
pub struct MockStopKey {
    on_press: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    pub armed: Arc<AtomicBool>,
    pub arm_count: Mutex<u32>,
}

impl MockStopKey {
    /// Simulate the user pressing Esc. Returns false if the tap is not armed
    /// (the key would then reach the app as normal).
    pub fn press(&self) -> bool {
        if !self.armed.load(Ordering::SeqCst) {
            return false;
        }
        let cb = self.on_press.lock().unwrap().clone();
        if let Some(cb) = cb {
            cb();
        }
        true
    }
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }
}

impl StopKey for MockStopKey {
    fn start(&self, on_press: Arc<dyn Fn() + Send + Sync>) -> Result<ListenerHandle> {
        *self.on_press.lock().unwrap() = Some(on_press);
        *self.arm_count.lock().unwrap() += 1;
        self.armed.store(true, Ordering::SeqCst);
        let armed = self.armed.clone();
        Ok(ListenerHandle::new(move || armed.store(false, Ordering::SeqCst)))
    }
}

/// Mock element handle: an id into `MockAccessibility::trees`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockElement(pub u64);

/// A node in a mock tree.
#[derive(Debug, Clone)]
pub struct MockNode {
    pub id: u64,
    pub depth: u16,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub bounds: Option<Rect>,
    pub menu_path: Option<String>,
    pub menu_root: Option<usize>,
}

impl MockNode {
    pub fn new(id: u64, depth: u16, role: &str, name: &str, bounds: Option<Rect>) -> Self {
        Self { id, depth, role: role.into(), name: name.into(), value: None, bounds, menu_path: None, menu_root: None }
    }
}

/// Scripted windows, apps and trees.
/// What `actionable_at` finds (whatever the point): element `id` of app
/// `pid` offering `action`.
#[derive(Debug, Clone)]
pub struct MockTarget {
    pub id: u64,
    pub pid: i32,
    pub role: String,
    pub name: String,
    pub action: String,
}

pub struct MockAccessibility {
    pub trusted: bool,
    pub windows: Mutex<Vec<WindowInfo>>,
    pub apps: Mutex<Vec<AppInfo>>,
    /// Apps that are installed but not running.
    pub installed: Vec<String>,
    pub frontmost: Mutex<Option<i32>>,
    pub trees: Mutex<HashMap<u64, Vec<MockNode>>>,
    pub stale: Mutex<HashSet<u64>>,
    pub pressed: Mutex<Vec<u64>>,
    /// Elements that offer AXPress (by default none do).
    pub pressable: Mutex<HashSet<u64>>,
    /// Elements whose press reports an error.
    pub press_fails: Mutex<HashSet<u64>>,
    pub launched: Mutex<Vec<String>>,
    pub raised: Mutex<Vec<u64>>,
    pub secure_focus: AtomicBool,
    /// What `focused_page` reports (None: nothing focused).
    pub page: Mutex<Option<FocusedPage>>,
    pub hit: Mutex<Option<HitElement>>,
    pub targets: Mutex<Vec<MockTarget>>,
    /// AX actions performed, as (element id, action).
    pub performed: Mutex<Vec<(u64, String)>>,
}

impl Default for MockAccessibility {
    fn default() -> Self {
        Self {
            trusted: true,
            windows: Mutex::new(vec![]),
            apps: Mutex::new(vec![]),
            installed: vec![],
            frontmost: Mutex::new(None),
            trees: Mutex::new(HashMap::new()),
            stale: Mutex::new(HashSet::new()),
            pressed: Mutex::new(vec![]),
            pressable: Mutex::new(HashSet::new()),
            press_fails: Mutex::new(HashSet::new()),
            launched: Mutex::new(vec![]),
            raised: Mutex::new(vec![]),
            secure_focus: AtomicBool::new(false),
            page: Mutex::new(None),
            hit: Mutex::new(None),
            targets: Mutex::new(Vec::new()),
            performed: Mutex::new(Vec::new()),
        }
    }
}

impl MockAccessibility {
    pub fn untrusted() -> Self {
        Self { trusted: false, ..Self::default() }
    }

    fn find_node(&self, id: u64) -> Option<MockNode> {
        self.trees.lock().unwrap().values().flatten().find(|n| n.id == id).cloned()
    }

    fn mock_id(el: &ElementHandle) -> Result<u64> {
        el.downcast_ref::<MockElement>().map(|e| e.0).ok_or_else(|| PlatformError::Other("not a mock element".into()))
    }
}

impl Accessibility for MockAccessibility {
    fn is_trusted(&self) -> bool {
        self.trusted
    }
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(self.windows.lock().unwrap().clone())
    }
    fn running_apps(&self) -> Result<Vec<AppInfo>> {
        Ok(self.apps.lock().unwrap().clone())
    }
    fn app_info(&self, pid: i32) -> Option<AppInfo> {
        self.apps.lock().unwrap().iter().find(|a| a.pid == pid).cloned()
    }
    fn frontmost_pid(&self) -> Result<Option<i32>> {
        Ok(*self.frontmost.lock().unwrap())
    }
    fn resolve_app(&self, name: &str) -> Option<ResolvedApp> {
        if let Some(a) = self.apps.lock().unwrap().iter().find(|a| a.name.eq_ignore_ascii_case(name)) {
            return Some(ResolvedApp { name: a.name.clone(), bundle_id: a.bundle_id.clone(), running: true });
        }
        self.installed
            .iter()
            .find(|n| n.eq_ignore_ascii_case(name))
            .map(|n| ResolvedApp { name: n.clone(), bundle_id: None, running: false })
    }
    fn launch_app(&self, name: &str) -> Result<()> {
        self.launched.lock().unwrap().push(name.into());
        Ok(())
    }
    fn window_tree(&self, window: &WindowInfo, limits: &TreeLimits) -> Result<AxTree> {
        let trees = self.trees.lock().unwrap();
        let nodes = trees.get(&window.id).cloned().unwrap_or_default();
        let nodes: Vec<AxNode> = nodes
            .into_iter()
            .filter(|n| limits.menu_depth > 1 || n.menu_path.is_none() || n.role == "AXMenuBarItem")
            .filter(|n| limits.menu_depth > 0 || n.role != "AXMenuBarItem")
            .map(|n| AxNode {
                handle: Arc::new(MockElement(n.id)),
                depth: n.depth,
                role: n.role,
                subrole: None,
                name: n.name,
                value: n.value,
                bounds: n.bounds,
                enabled: true,
                focused: false,
                shortcut: None,
                menu_path: n.menu_path,
                menu_root: n.menu_root,
            })
            .collect();
        // menu_root indexes refer to the unfiltered list in fixtures; tests
        // that use menus ask for menu_depth >= 2, which keeps every node.
        Ok(AxTree { nodes, truncated: false })
    }
    fn element_frame(&self, el: &ElementHandle) -> Result<Option<Rect>> {
        let id = Self::mock_id(el)?;
        if self.stale.lock().unwrap().contains(&id) {
            return Err(PlatformError::Stale);
        }
        self.find_node(id).map(|n| n.bounds).ok_or(PlatformError::Stale)
    }
    fn press(&self, el: &ElementHandle) -> Result<()> {
        let id = Self::mock_id(el)?;
        if self.stale.lock().unwrap().contains(&id) {
            return Err(PlatformError::Stale);
        }
        if self.press_fails.lock().unwrap().contains(&id) {
            return Err(PlatformError::Other("AXPress failed (AXError -25204)".into()));
        }
        self.pressed.lock().unwrap().push(id);
        Ok(())
    }
    fn can_press(&self, el: &ElementHandle) -> Result<bool> {
        let id = Self::mock_id(el)?;
        if self.stale.lock().unwrap().contains(&id) {
            return Err(PlatformError::Stale);
        }
        Ok(self.pressable.lock().unwrap().contains(&id))
    }
    fn actionable_at(&self, _at: Point, action: &str) -> Result<Option<ActionTarget>> {
        Ok(self.targets.lock().unwrap().iter().find(|t| t.action == action).map(|t| ActionTarget {
            handle: Arc::new(MockElement(t.id)),
            pid: t.pid,
            role: t.role.clone(),
            name: t.name.clone(),
        }))
    }

    fn has_action(&self, el: &ElementHandle, action: &str) -> Result<bool> {
        let id = Self::mock_id(el)?;
        Ok(self.targets.lock().unwrap().iter().any(|t| t.id == id && t.action == action))
    }

    fn perform(&self, el: &ElementHandle, action: &str) -> Result<()> {
        let id = Self::mock_id(el)?;
        if action == "AXPress" {
            self.pressed.lock().unwrap().push(id);
        }
        self.performed.lock().unwrap().push((id, action.to_string()));
        Ok(())
    }

    fn element_at(&self, _at: Point) -> Result<Option<HitElement>> {
        Ok(self.hit.lock().unwrap().clone())
    }
    fn raise_window(&self, window: &WindowInfo) -> Result<()> {
        self.raised.lock().unwrap().push(window.id);
        *self.frontmost.lock().unwrap() = Some(window.pid);
        Ok(())
    }
    fn focused_is_secure(&self) -> Result<bool> {
        Ok(self.secure_focus.load(Ordering::SeqCst))
    }
    fn focused_page(&self) -> Result<Option<FocusedPage>> {
        let mut page = self.page.lock().unwrap().clone();
        if let Some(p) = page.as_mut() {
            p.secure |= self.secure_focus.load(Ordering::SeqCst);
        }
        Ok(page)
    }
}

/// Records cursor moves and the indicator state.
#[derive(Default)]
pub struct MockOverlay {
    pub moves: Mutex<Vec<Point>>,
    pub acting: Mutex<Vec<bool>>,
    pub actions: Mutex<Vec<CursorAction>>,
}

impl Overlay for MockOverlay {
    fn move_cursor(&self, to: Point, _label: Option<String>) -> Duration {
        self.moves.lock().unwrap().push(to);
        Duration::from_millis(1)
    }
    fn set_acting(&self, acting: bool) {
        self.acting.lock().unwrap().push(acting);
    }
    fn action(&self, action: &CursorAction) {
        self.actions.lock().unwrap().push(action.clone());
    }
}
