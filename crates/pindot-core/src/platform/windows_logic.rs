//! The parts of the Windows implementation that need no Windows API, so they
//! are tested on every OS: what the microphone privacy switches mean, and
//! what the low-level keyboard hook does with each key.
//!
//! `platform/windows` reads the registry and runs the hook; it only passes
//! the raw values in here.

use super::keys::Modifiers;
use super::{ChordTracker, DisplayInfo, HeldMods, HotkeyBinding, HotkeyEvent, KeyName, ModKey, PermissionState, Point, Side};

// ------------------------------------------------------------- windows and apps

pub const WS_EX_TRANSPARENT: u32 = 0x20;
pub const WS_EX_LAYERED: u32 = 0x8_0000;

pub const WS_EX_TOOLWINDOW: u32 = 0x80;
pub const WS_EX_APPWINDOW: u32 = 0x4_0000;

/// The extended style of an overlay window: a tool window, so it is in
/// neither the Alt+Tab list nor the taskbar. Everything else is kept.
pub fn overlay_ex_style(ex_style: u32) -> u32 {
    (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
}

/// What Pindot could learn about another process's rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// Runs as administrator.
    Elevated,
    Normal,
    /// Windows would not let us look: only a more privileged process says that.
    Denied,
    /// The process is gone, or the question failed some other way.
    Unknown,
}

/// Will Windows drop input Pindot sends to that process? It does when the
/// target runs with more rights than Pindot (an administrator's window
/// while Pindot runs normally), and `SendInput` still reports success.
pub fn input_is_blocked(pindot_elevated: bool, target: Elevation) -> bool {
    !pindot_elevated && matches!(target, Elevation::Elevated | Elevation::Denied)
}

/// What `EnumWindows` knows about one visible top-level window.
#[derive(Debug, Clone, Copy)]
pub struct RawWindow<'a> {
    pub class: &'a str,
    pub ex_style: u32,
    /// DWM hides it (a suspended store app, another virtual desktop).
    pub cloaked: bool,
    pub minimised: bool,
    pub width: i32,
    pub height: i32,
}

/// What a top-level window is to Pindot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Not listed: the desktop itself, or nothing to see.
    Skip,
    /// On screen, but never something Claude may control.
    Taskbar,
    /// Clicks pass through it: another program's overlay, or ours.
    ClickThrough,
    Normal,
}

pub fn classify_window(w: &RawWindow) -> WindowKind {
    if w.cloaked || w.minimised || w.width <= 0 || w.height <= 0 {
        return WindowKind::Skip;
    }
    match w.class {
        // The desktop (wallpaper and icons).
        "Progman" | "WorkerW" => WindowKind::Skip,
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" => WindowKind::Taskbar,
        _ if w.ex_style & WS_EX_TRANSPARENT != 0 && w.ex_style & WS_EX_LAYERED != 0 => WindowKind::ClickThrough,
        _ => WindowKind::Normal,
    }
}

fn exe_stem(exe: &str) -> &str {
    let lower_len = exe.len().saturating_sub(4);
    if exe.len() > 4 && exe[lower_len..].eq_ignore_ascii_case(".exe") { &exe[..lower_len] } else { exe }
}

/// Does `query` (what Claude or the user called an app) mean the program
/// with this display `name` and executable file name `exe`?
pub fn app_matches(query: &str, name: &str, exe: &str) -> bool {
    let q = exe_stem(query.trim());
    !q.is_empty() && (q.eq_ignore_ascii_case(name) || q.eq_ignore_ascii_case(exe_stem(exe)))
}

/// One entry of the Start menu's app list (`Get-StartApps`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartApp {
    pub name: String,
    /// What `shell:AppsFolder\<id>` opens.
    pub app_id: String,
}

/// Parse `Get-StartApps | ConvertTo-Json`: an array, or a single object
/// when there is one app. Uninstallers and documents are left out.
pub fn parse_start_apps(json: &str) -> Vec<StartApp> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let items = match value {
        serde_json::Value::Array(a) => a,
        one @ serde_json::Value::Object(_) => vec![one],
        _ => return vec![],
    };
    const NOT_APPS: [&str; 6] = [".url", ".chm", ".txt", ".htm", ".html", ".pdf"];
    items
        .iter()
        .filter_map(|v| {
            let name = v.get("Name")?.as_str()?.trim();
            let app_id = v.get("AppID")?.as_str()?.trim();
            let id = app_id.to_ascii_lowercase();
            let skip = name.is_empty()
                || app_id.is_empty()
                || name.to_ascii_lowercase().starts_with("uninstall")
                || NOT_APPS.iter().any(|ext| id.ends_with(ext));
            (!skip).then(|| StartApp { name: name.to_string(), app_id: app_id.to_string() })
        })
        .collect()
}

/// The Start-menu app `query` means: its exact name, else the only app
/// whose name contains the query as whole words ("Chrome" for "Google
/// Chrome"). `None` when nothing or more than one app fits.
pub fn find_start_app<'a>(apps: &'a [StartApp], query: &str) -> Option<&'a StartApp> {
    let q = exe_stem(query.trim()).to_lowercase();
    if q.is_empty() {
        return None;
    }
    if let Some(exact) = apps.iter().find(|a| a.name.to_lowercase() == q) {
        return Some(exact);
    }
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
    };
    let want = words(&q);
    if want.is_empty() {
        return None;
    }
    let mut fits = apps.iter().filter(|a| {
        let have = words(&a.name);
        have.windows(want.len()).any(|w| w == want.as_slice())
    });
    let first = fits.next()?;
    fits.next().is_none().then_some(first)
}

/// The Start-menu name of a running program, so an app has one name
/// whether Claude asked for it before or after it started ("Word", not the
/// executable's "Microsoft Word"). `aumid` is the process's application
/// user model id (store and other packaged apps have one); otherwise the
/// Start entry whose id names this executable. `None` if nothing, or more
/// than one entry, fits.
pub fn start_name_for(exe: &str, aumid: Option<&str>, apps: &[StartApp]) -> Option<String> {
    if let Some(id) = aumid
        && let Some(a) = apps.iter().find(|a| a.app_id.eq_ignore_ascii_case(id))
    {
        return Some(a.name.clone());
    }
    let exe = exe.to_lowercase();
    if exe.is_empty() {
        return None;
    }
    // The id names this executable: as a whole file name, not as the tail
    // of a longer one ("notepad.exe" is not "mynotepad.exe").
    let names_exe = |id: &str| {
        let id = id.to_lowercase();
        id.match_indices(&exe).any(|(i, m)| {
            let before = id[..i].chars().next_back();
            let after = id[i + m.len()..].chars().next();
            before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
        })
    };
    let mut fits = apps.iter().filter(|a| names_exe(&a.app_id));
    let first = fits.next()?;
    fits.next().is_none().then(|| first.name.clone())
}

// ------------------------------------------------------------- the element tree

/// UI Automation control type ids (`UIA_*ControlTypeId`).
pub const UIA_EDIT: i32 = 50004;
pub const UIA_MENU_BAR: i32 = 50010;
pub const UIA_MENU_ITEM: i32 = 50011;

/// The role and subrole Pindot's tree uses for a UI Automation control
/// type. They are the macOS accessibility names, because `server::tree`
/// (what counts as interactive, how a role is shown) is written in them.
pub fn uia_role(control_type: i32, is_password: bool, in_menu_bar: bool) -> (&'static str, Option<&'static str>) {
    let role = match control_type {
        50000 => "AXButton",
        50002 => "AXCheckBox",
        50003 => "AXComboBox",
        UIA_EDIT => "AXTextField",
        50005 => "AXLink",
        50006 => "AXImage",
        // List items, tree items and data items are the rows one clicks.
        50007 | 50024 | 50029 => "AXRow",
        50008 => "AXList",
        50009 => "AXMenu",
        UIA_MENU_BAR => "AXMenuBar",
        UIA_MENU_ITEM if in_menu_bar => "AXMenuBarItem",
        UIA_MENU_ITEM => "AXMenuItem",
        50012 => "AXProgressIndicator",
        50013 => "AXRadioButton",
        50014 => "AXScrollBar",
        50015 => "AXSlider",
        50016 => "AXIncrementor",
        50018 => "AXTabGroup",
        50019 => "AXTab",
        50020 => "AXStaticText",
        50021 | 50040 => "AXToolbar",
        50022 => "AXHelpTag",
        50023 => "AXOutline",
        50027 => "AXValueIndicator",
        50028 | 50036 => "AXTable",
        50030 => "AXTextArea",
        50031 => "AXMenuButton",
        50032 => "AXWindow",
        50035 => "AXButton",
        50038 => "AXSplitter",
        // Calendar, status bar, custom, group, pane, header, title bar, semantic zoom.
        50001 | 50017 | 50025 | 50026 | 50033 | 50034 | 50037 | 50039 => "AXGroup",
        _ => "AXUnknown",
    };
    let subrole = (control_type == UIA_EDIT && is_password).then_some("AXSecureTextField");
    (role, subrole)
}

pub const UIA_DOCUMENT: i32 = 50030;

/// Has a browser's (or an Electron app's) page not shown up in its tree
/// yet? Chromium and Firefox build a page's accessibility tree only once
/// something asks, so the first walk of such a window finds the browser's
/// frame and no page. `nodes` is the walk: control type and depth of each
/// node, in order.
pub fn web_tree_pending(window_class: &str, nodes: &[(i32, u16)]) -> bool {
    let browser = window_class.starts_with("Chrome_WidgetWin") || window_class == "MozillaWindowClass";
    if !browser || nodes.is_empty() {
        return false;
    }
    // Loaded: some document has something under it.
    !nodes.windows(2).any(|w| w[0].0 == UIA_DOCUMENT && w[1].1 > w[0].1)
}

/// Is this element's value worth reading (what a field holds, what a combo
/// box shows)? Never a password's.
pub fn uia_has_value(control_type: i32, is_password: bool) -> bool {
    // Edit, combo box, slider, spinner. Not a document: it can be huge.
    !is_password && matches!(control_type, UIA_EDIT | 50003 | 50015 | 50016)
}

// ------------------------------------------------------------- input

/// The virtual key of a named key and whether it is an "extended" key
/// (arrows and the navigation block, as opposed to the number pad).
/// `None` for a character: the keyboard layout decides.
pub fn vk_for_key(k: KeyName) -> Option<(u16, bool)> {
    Some(match k {
        KeyName::Char(_) => return None,
        KeyName::Backspace => (0x08, false),
        KeyName::Tab => (0x09, false),
        KeyName::Return => (0x0D, false),
        KeyName::Escape => (0x1B, false),
        KeyName::Space => (0x20, false),
        KeyName::PageUp => (0x21, true),
        KeyName::PageDown => (0x22, true),
        KeyName::End => (0x23, true),
        KeyName::Home => (0x24, true),
        KeyName::Left => (0x25, true),
        KeyName::Up => (0x26, true),
        KeyName::Right => (0x27, true),
        KeyName::Down => (0x28, true),
        KeyName::ForwardDelete => (0x2E, true),
        KeyName::F(n) => (0x70 + u16::from(n.clamp(1, 24)) - 1, false),
    })
}

/// The modifier virtual keys to hold for a combo, in press order. `cmd` is
/// Ctrl on Windows (what a Mac shortcut such as cmd+s means there); `win`
/// is the Windows key.
pub fn modifier_vks(m: Modifiers) -> Vec<u16> {
    [(m.ctrl || m.cmd, VK_CONTROL), (m.alt, VK_MENU), (m.shift, VK_SHIFT), (m.win, VK_LWIN)]
        .into_iter()
        .filter_map(|(on, vk)| on.then_some(vk as u16))
        .collect()
}

/// `SendInput` absolute mouse coordinates (0..=65535 across the virtual
/// desktop) for a physical pixel. `virt` is the virtual desktop: left, top,
/// width, height.
pub fn mouse_abs(x: i32, y: i32, virt: (i32, i32, i32, i32)) -> (i32, i32) {
    let (left, top, width, height) = virt;
    let norm = |v: i32, origin: i32, size: i32| -> i32 {
        let span = f64::from((size - 1).max(1));
        (f64::from(v - origin) * 65535.0 / span).round().clamp(0.0, 65535.0) as i32
    };
    (norm(x, left, width), norm(y, top, height))
}

/// A global logical point to a physical pixel: times the scale of the
/// display it is on (the nearest display if it is on none).
pub fn to_physical(displays: &[DisplayInfo], p: Point) -> (i32, i32) {
    let scale = displays
        .iter()
        .find(|d| d.bounds.contains(p))
        .or_else(|| {
            displays.iter().min_by(|a, b| {
                a.bounds.center().distance(p).partial_cmp(&b.bounds.center().distance(p)).unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .map_or(1.0, |d| d.scale_factor);
    ((p.x * scale).round() as i32, (p.y * scale).round() as i32)
}

// ------------------------------------------------------------- microphone

/// One switch of the microphone consent store
/// (`...\CapabilityAccessManager\ConsentStore\microphone`, value `Value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    Allow,
    Deny,
    /// The value is not there. Windows allows the microphone by default.
    Missing,
    /// The registry could not be read, or holds a word we don't know.
    Unreadable,
}

impl Consent {
    /// `None` = the value does not exist.
    pub fn from_value(value: Option<&str>) -> Consent {
        match value {
            None => Consent::Missing,
            Some(v) if v.eq_ignore_ascii_case("Allow") => Consent::Allow,
            Some(v) if v.eq_ignore_ascii_case("Deny") => Consent::Deny,
            Some(_) => Consent::Unreadable,
        }
    }
}

/// The three switches on Settings > Privacy & security > Microphone that
/// decide whether a desktop app such as Pindot hears anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicSwitches {
    /// "Microphone access", for the whole PC (HKLM).
    pub device: Consent,
    /// "Let apps access your microphone", for this user (HKCU).
    pub user: Consent,
    /// "Let desktop apps access your microphone" (HKCU, `NonPackaged`).
    pub desktop_apps: Consent,
}

impl MicSwitches {
    /// Windows has no per-app grant for desktop apps and never prompts: the
    /// microphone works when none of the three switches is off.
    pub fn state(&self) -> PermissionState {
        let all = [self.device, self.user, self.desktop_apps];
        if all.contains(&Consent::Deny) {
            PermissionState::Denied
        } else if all.contains(&Consent::Unreadable) {
            PermissionState::Unknown
        } else {
            PermissionState::Granted
        }
    }

    /// The switch that is off, in the words Windows Settings uses.
    pub fn off_switch(&self) -> Option<&'static str> {
        [
            (self.device, "Microphone access"),
            (self.user, "Let apps access your microphone"),
            (self.desktop_apps, "Let desktop apps access your microphone"),
        ]
        .into_iter()
        .find(|(c, _)| *c == Consent::Deny)
        .map(|(_, name)| name)
    }
}

/// Does this state keep hold-to-talk from starting? On macOS anything short
/// of a grant does, because opening the microphone without one shows a
/// prompt. Windows never prompts, so only a switch that is off does: if the
/// switches can't be read, trying costs nothing.
pub fn blocks_voice(state: PermissionState, windows: bool) -> bool {
    if windows { matches!(state, PermissionState::Denied | PermissionState::Restricted) } else { !state.ok() }
}

// ------------------------------------------------------------- keys

pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;
/// An unassigned virtual key. Pressed while Win or Alt is held, it makes
/// Windows treat the hold as a shortcut, so letting go opens neither the
/// Start menu nor a window's menu bar.
pub const VK_MASK: u32 = 0xE8;
/// The scan code of the Left Ctrl that Windows makes up in front of AltGr.
pub const SCAN_ALTGR_CTRL: u32 = 0x21D;

/// One key transition as the low-level keyboard hook reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u32,
    pub scan: u32,
    pub down: bool,
    /// Pindot injected it (the mask key, or later the `key` tool).
    pub ours: bool,
}

impl KeyEvent {
    pub fn new(vk: u32, down: bool) -> Self {
        Self { vk, scan: 0, down, ours: false }
    }
}

/// The modifier a Windows virtual key belongs to (Win is `Command`, Alt is
/// `Option`). The generic codes count as the left key.
pub fn vk_modifier(vk: u32) -> Option<(ModKey, Side)> {
    Some(match vk {
        VK_LCONTROL | VK_CONTROL => (ModKey::Control, Side::Left),
        VK_RCONTROL => (ModKey::Control, Side::Right),
        VK_LMENU | VK_MENU => (ModKey::Option, Side::Left),
        VK_RMENU => (ModKey::Option, Side::Right),
        VK_LSHIFT | VK_SHIFT => (ModKey::Shift, Side::Left),
        VK_RSHIFT => (ModKey::Shift, Side::Right),
        VK_LWIN => (ModKey::Command, Side::Left),
        VK_RWIN => (ModKey::Command, Side::Right),
        _ => return None,
    })
}

/// What the hook does after the hold-to-talk logic saw a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChordOutcome {
    pub event: Option<HotkeyEvent>,
    /// Press [`VK_MASK`] now.
    pub mask: bool,
}

/// Hold-to-talk on Windows: feeds the shared [`ChordTracker`] from key
/// transitions. Nothing is swallowed.
#[derive(Debug, Clone)]
pub struct WinChord {
    tracker: ChordTracker,
    held: HeldMods,
    needs_mask: bool,
}

impl WinChord {
    pub fn new(binding: HotkeyBinding) -> Self {
        let needs_mask = binding.keys().iter().any(|k| matches!(k.key, ModKey::Command | ModKey::Option));
        Self { tracker: ChordTracker::new(binding), held: HeldMods::default(), needs_mask }
    }

    /// The chord is held right now.
    pub fn is_active(&self) -> bool {
        self.tracker.is_active()
    }

    /// Correct the tracked modifiers from the keys that are really down
    /// (`GetAsyncKeyState`), on a timer. The hook can miss a key: Windows
    /// drops a slow hook without telling it, and it never sees keys typed
    /// on the lock screen, a UAC prompt or into an administrator's window.
    /// Without this a missed key-up left a modifier "held" for good and the
    /// chord never matched again.
    pub fn resync(&mut self, mut real: HeldMods) -> ChordOutcome {
        // AltGr: Windows holds a made-up Left Ctrl with Right Alt, which
        // `on_key` ignored.
        if real.r_opt && real.l_ctrl && !self.held.l_ctrl {
            real.l_ctrl = false;
        }
        if real == self.held {
            return ChordOutcome::default();
        }
        self.held = real;
        let event = self.tracker.on_mods(real);
        ChordOutcome { event, mask: self.needs_mask && event == Some(HotkeyEvent::Down) }
    }

    pub fn on_key(&mut self, e: KeyEvent) -> ChordOutcome {
        if e.ours {
            return ChordOutcome::default();
        }
        let Some((key, side)) = vk_modifier(e.vk) else {
            let event = if e.down { self.tracker.on_other_key() } else { None };
            return ChordOutcome { event, mask: false };
        };
        if e.vk == VK_LCONTROL && e.scan == SCAN_ALTGR_CTRL {
            return ChordOutcome::default();
        }
        self.held.set(key, side, e.down);
        let event = self.tracker.on_mods(self.held);
        ChordOutcome { event, mask: self.needs_mask && event == Some(HotkeyEvent::Down) }
    }
}

/// What the hook does after the stop-key logic saw a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EscOutcome {
    /// Keep the key from every app.
    pub swallow: bool,
    /// A new press: stop everything.
    pub pressed: bool,
}

/// The Esc emergency stop on Windows: a real Esc is swallowed and reported
/// once per press; Pindot's own Esc passes through.
#[derive(Debug, Clone, Copy, Default)]
pub struct EscFilter {
    down: bool,
}

impl EscFilter {
    pub fn on_key(&mut self, e: KeyEvent) -> EscOutcome {
        if e.ours || e.vk != VK_ESCAPE {
            return EscOutcome::default();
        }
        let pressed = e.down && !self.down;
        self.down = e.down;
        EscOutcome { swallow: true, pressed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(class: &str, ex_style: u32) -> RawWindow<'_> {
        RawWindow { class, ex_style, cloaked: false, minimised: false, width: 800, height: 600 }
    }

    #[test]
    fn an_overlay_is_a_tool_window_and_keeps_its_other_styles() {
        // What the overlay has today: topmost, click-through, app window, layered, no-activate.
        let before = 0x8 | WS_EX_TRANSPARENT | WS_EX_APPWINDOW | WS_EX_LAYERED | 0x0800_0000;
        let after = overlay_ex_style(before);
        assert_ne!(after & WS_EX_TOOLWINDOW, 0, "a tool window is not in Alt+Tab");
        assert_eq!(after & WS_EX_APPWINDOW, 0, "an app window is forced into Alt+Tab and the taskbar");
        assert_eq!(after & !WS_EX_TOOLWINDOW, before & !WS_EX_APPWINDOW, "nothing else changes");
        assert_eq!(overlay_ex_style(after), after, "applying it twice changes nothing");
    }

    #[test]
    fn input_to_an_administrators_window_is_blocked() {
        assert!(input_is_blocked(false, Elevation::Elevated));
        assert!(input_is_blocked(false, Elevation::Denied));
        assert!(!input_is_blocked(false, Elevation::Normal));
        // Not knowing is not a reason to refuse every click.
        assert!(!input_is_blocked(false, Elevation::Unknown));
        // Pindot itself runs as administrator: nothing is above it.
        assert!(!input_is_blocked(true, Elevation::Elevated));
        assert!(!input_is_blocked(true, Elevation::Denied));
    }

    #[test]
    fn an_ordinary_window_is_listed() {
        assert_eq!(classify_window(&raw("Notepad", 0)), WindowKind::Normal);
        assert_eq!(classify_window(&raw("Chrome_WidgetWin_1", 0x100)), WindowKind::Normal);
    }

    #[test]
    fn hidden_minimised_and_empty_windows_are_skipped() {
        assert_eq!(classify_window(&RawWindow { cloaked: true, ..raw("ApplicationFrameWindow", 0) }), WindowKind::Skip);
        assert_eq!(classify_window(&RawWindow { minimised: true, ..raw("Notepad", 0) }), WindowKind::Skip);
        assert_eq!(classify_window(&RawWindow { width: 0, ..raw("Notepad", 0) }), WindowKind::Skip);
        assert_eq!(classify_window(&RawWindow { height: 0, ..raw("Notepad", 0) }), WindowKind::Skip);
    }

    #[test]
    fn the_desktop_is_skipped_and_the_taskbar_is_furniture() {
        assert_eq!(classify_window(&raw("Progman", 0)), WindowKind::Skip);
        assert_eq!(classify_window(&raw("WorkerW", 0)), WindowKind::Skip);
        assert_eq!(classify_window(&raw("Shell_TrayWnd", 0x88)), WindowKind::Taskbar);
        assert_eq!(classify_window(&raw("Shell_SecondaryTrayWnd", 0x88)), WindowKind::Taskbar);
    }

    #[test]
    fn a_click_through_overlay_is_not_a_click_target() {
        // Pindot's own overlay, and e.g. a game or recording overlay.
        let overlay = raw("Tauri Window", WS_EX_TRANSPARENT | WS_EX_LAYERED | 0x8);
        assert_eq!(classify_window(&overlay), WindowKind::ClickThrough);
        // Layered alone is an ordinary (maybe translucent) window.
        assert_eq!(classify_window(&raw("Whatever", WS_EX_LAYERED)), WindowKind::Normal);
    }

    #[test]
    fn an_app_is_matched_by_its_name_or_its_executable() {
        assert!(app_matches("Notepad", "Notepad", "Notepad.exe"));
        assert!(app_matches("notepad", "Notepad", "Notepad.exe"));
        assert!(app_matches(" notepad.exe ", "Notepad", "Notepad.exe"));
        assert!(app_matches("chrome", "Google Chrome", "chrome.exe"));
        assert!(app_matches("Google Chrome", "Google Chrome", "chrome.exe"));
        assert!(app_matches("Code", "Visual Studio Code", "Code.exe"));
        assert!(!app_matches("Chrom", "Google Chrome", "chrome.exe"));
        assert!(!app_matches("", "Google Chrome", "chrome.exe"));
    }

    #[test]
    fn parses_the_start_menu_list() {
        let json = r#"[
            {"Name":"Notepad","AppID":"Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"},
            {"Name":"Google Chrome","AppID":"Chrome"},
            {"Name":"Uninstall Foo","AppID":"C:\\Program Files\\Foo\\unins000.exe"},
            {"Name":"Foo Help","AppID":"C:\\Program Files\\Foo\\help.chm"},
            {"Name":"Foo on the Web","AppID":"C:\\Program Files\\Foo\\foo.url"}
        ]"#;
        let apps = parse_start_apps(json);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0], StartApp { name: "Notepad".into(), app_id: "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App".into() });
        assert_eq!(apps[1].name, "Google Chrome");
    }

    #[test]
    fn parses_a_single_start_app_and_survives_garbage() {
        let one = parse_start_apps(r#"{"Name":"Calculator","AppID":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"}"#);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "Calculator");
        assert!(parse_start_apps("").is_empty());
        assert!(parse_start_apps("not json").is_empty());
    }

    fn start(names: &[&str]) -> Vec<StartApp> {
        names.iter().map(|n| StartApp { name: n.to_string(), app_id: format!("id:{n}") }).collect()
    }

    #[test]
    fn finds_a_start_app_by_exact_name_first() {
        let apps = start(&["Notepad", "Notepad++", "Google Chrome"]);
        assert_eq!(find_start_app(&apps, "notepad").map(|a| a.name.as_str()), Some("Notepad"));
        assert_eq!(find_start_app(&apps, "Notepad++").map(|a| a.name.as_str()), Some("Notepad++"));
    }

    #[test]
    fn finds_a_start_app_by_whole_words_when_only_one_fits() {
        let apps = start(&["Google Chrome", "Visual Studio Code", "Microsoft Edge", "Visual Studio 2022"]);
        assert_eq!(find_start_app(&apps, "Chrome").map(|a| a.name.as_str()), Some("Google Chrome"));
        assert_eq!(find_start_app(&apps, "edge").map(|a| a.name.as_str()), Some("Microsoft Edge"));
        assert_eq!(find_start_app(&apps, "Visual Studio"), None, "two apps fit");
        assert_eq!(find_start_app(&apps, "Chrom"), None, "not a whole word");
        assert_eq!(find_start_app(&apps, "Firefox"), None);
        assert_eq!(find_start_app(&apps, ""), None);
    }

    fn start_ids(apps: &[(&str, &str)]) -> Vec<StartApp> {
        apps.iter().map(|(n, id)| StartApp { name: n.to_string(), app_id: id.to_string() }).collect()
    }

    #[test]
    fn a_running_program_gets_its_start_menu_name() {
        let apps = start_ids(&[
            ("Notepad", "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"),
            ("Word", "Microsoft.Office.WINWORD.EXE.15"),
            ("Notepad++", r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe"),
            ("Google Chrome", "Chrome"),
        ]);
        // A packaged app: by its application id.
        let aumid = Some("microsoft.windowsnotepad_8wekyb3d8bbwe!App");
        assert_eq!(start_name_for("Notepad.exe", aumid, &apps).as_deref(), Some("Notepad"));
        // Office: the id names the executable.
        assert_eq!(start_name_for("WINWORD.EXE", None, &apps).as_deref(), Some("Word"));
        // A shortcut: the id is the path of the executable.
        assert_eq!(start_name_for("notepad++.exe", None, &apps).as_deref(), Some("Notepad++"));
        // Nothing names chrome.exe; the caller falls back to the file's description.
        assert_eq!(start_name_for("chrome.exe", None, &apps), None);
    }

    #[test]
    fn an_executable_named_by_two_start_entries_gets_no_start_name() {
        let apps = start_ids(&[("Foo", r"C:\Foo\foo.exe"), ("Foo (Safe Mode)", r"C:\Foo\foo.exe")]);
        assert_eq!(start_name_for("foo.exe", None, &apps), None);
        // "notepad.exe" must not match "notepad++.exe" or "mynotepad.exe".
        let apps = start_ids(&[("Notepad++", r"C:\P\notepad++.exe"), ("My Notepad", r"C:\P\mynotepad.exe")]);
        assert_eq!(start_name_for("notepad.exe", None, &apps), None);
    }

    #[test]
    fn control_types_map_to_the_trees_roles() {
        assert_eq!(uia_role(50000, false, false), ("AXButton", None));
        assert_eq!(uia_role(50002, false, false), ("AXCheckBox", None));
        assert_eq!(uia_role(50003, false, false), ("AXComboBox", None));
        assert_eq!(uia_role(UIA_EDIT, false, false), ("AXTextField", None));
        assert_eq!(uia_role(50005, false, false), ("AXLink", None));
        assert_eq!(uia_role(50007, false, false), ("AXRow", None));
        assert_eq!(uia_role(50013, false, false), ("AXRadioButton", None));
        assert_eq!(uia_role(50019, false, false), ("AXTab", None));
        assert_eq!(uia_role(50020, false, false), ("AXStaticText", None));
        assert_eq!(uia_role(50024, false, false), ("AXRow", None));
        assert_eq!(uia_role(50030, false, false), ("AXTextArea", None));
        assert_eq!(uia_role(50032, false, false), ("AXWindow", None));
        assert_eq!(uia_role(50033, false, false), ("AXGroup", None));
        assert_eq!(uia_role(99999, false, false), ("AXUnknown", None));
    }

    #[test]
    fn a_password_field_is_a_secure_text_field() {
        assert_eq!(uia_role(UIA_EDIT, true, false), ("AXTextField", Some("AXSecureTextField")));
        assert!(!uia_has_value(UIA_EDIT, true), "a password's value is never read");
        assert!(uia_has_value(UIA_EDIT, false));
        assert!(uia_has_value(50003, false), "a combo box shows its choice");
        assert!(!uia_has_value(50000, false));
        assert!(!uia_has_value(50030, false), "a document can be huge");
    }

    #[test]
    fn a_browser_window_without_its_page_is_pending() {
        // What Edge gave on the first read: the frame, an address bar, no page.
        let frame_only = [(50032, 0), (50000, 1), (50000, 1), (UIA_EDIT, 1), (UIA_EDIT, 1)];
        assert!(web_tree_pending("Chrome_WidgetWin_1", &frame_only));
        assert!(web_tree_pending("MozillaWindowClass", &frame_only));
        // The document is there but still empty.
        let empty_page = [(50032, 0), (50033, 1), (UIA_DOCUMENT, 2)];
        assert!(web_tree_pending("Chrome_WidgetWin_1", &empty_page));
        let empty_then_sibling = [(50032, 0), (UIA_DOCUMENT, 1), (50000, 1)];
        assert!(web_tree_pending("Chrome_WidgetWin_1", &empty_then_sibling));
    }

    #[test]
    fn a_loaded_page_and_an_ordinary_window_are_not_pending() {
        let loaded = [(50032, 0), (50033, 1), (UIA_DOCUMENT, 2), (UIA_EDIT, 3), (50000, 3)];
        assert!(!web_tree_pending("Chrome_WidgetWin_1", &loaded));
        // Not a browser: nothing to wait for, with or without a document.
        let frame_only = [(50032, 0), (50000, 1)];
        assert!(!web_tree_pending("Notepad", &frame_only));
        assert!(!web_tree_pending("WindowsForms10.Window.8.app.0.141b42a_r6_ad1", &frame_only));
        assert!(!web_tree_pending("Chrome_WidgetWin_1", &[]), "an empty walk failed for another reason");
    }

    #[test]
    fn a_menu_item_on_the_menu_bar_is_a_menu_bar_item() {
        assert_eq!(uia_role(UIA_MENU_ITEM, false, true), ("AXMenuBarItem", None));
        assert_eq!(uia_role(UIA_MENU_ITEM, false, false), ("AXMenuItem", None));
        assert_eq!(uia_role(UIA_MENU_BAR, false, false), ("AXMenuBar", None));
    }

    #[test]
    fn the_tree_lists_the_mapped_controls_as_interactive() {
        use crate::platform::AxNode;
        use crate::server::tree::is_interactive;
        let node = |ct: i32, pw: bool| {
            let (role, subrole) = uia_role(ct, pw, false);
            AxNode {
                handle: std::sync::Arc::new(()),
                depth: 0,
                role: role.into(),
                subrole: subrole.map(String::from),
                name: String::new(),
                value: None,
                bounds: None,
                enabled: true,
                focused: false,
                shortcut: None,
                menu_path: None,
                menu_root: None,
            }
        };
        // Button, check box, combo box, edit, link, list item, menu item,
        // radio, slider, tab item, tree item, document, split button.
        for ct in [50000, 50002, 50003, 50004, 50005, 50007, 50011, 50013, 50015, 50019, 50024, 50030, 50031] {
            assert!(is_interactive(&node(ct, false)), "control type {ct}");
        }
        assert!(is_interactive(&node(UIA_EDIT, true)), "a password box is listed (as securetextfield)");
        // Text, pane, group, window, image.
        for ct in [50020, 50033, 50026, 50032, 50006] {
            assert!(!is_interactive(&node(ct, false)), "control type {ct}");
        }
    }

    #[test]
    fn named_keys_map_to_virtual_keys() {
        assert_eq!(vk_for_key(KeyName::Return), Some((0x0D, false)));
        assert_eq!(vk_for_key(KeyName::Tab), Some((0x09, false)));
        assert_eq!(vk_for_key(KeyName::Space), Some((0x20, false)));
        assert_eq!(vk_for_key(KeyName::Backspace), Some((0x08, false)));
        assert_eq!(vk_for_key(KeyName::Escape), Some((0x1B, false)));
        assert_eq!(vk_for_key(KeyName::F(1)), Some((0x70, false)));
        assert_eq!(vk_for_key(KeyName::F(12)), Some((0x7B, false)));
        assert_eq!(vk_for_key(KeyName::Char('a')), None);
    }

    #[test]
    fn navigation_keys_are_extended_keys() {
        // Without the flag Windows takes them for the number pad's.
        assert_eq!(vk_for_key(KeyName::Left), Some((0x25, true)));
        assert_eq!(vk_for_key(KeyName::Up), Some((0x26, true)));
        assert_eq!(vk_for_key(KeyName::Right), Some((0x27, true)));
        assert_eq!(vk_for_key(KeyName::Down), Some((0x28, true)));
        assert_eq!(vk_for_key(KeyName::Home), Some((0x24, true)));
        assert_eq!(vk_for_key(KeyName::End), Some((0x23, true)));
        assert_eq!(vk_for_key(KeyName::PageUp), Some((0x21, true)));
        assert_eq!(vk_for_key(KeyName::PageDown), Some((0x22, true)));
        assert_eq!(vk_for_key(KeyName::ForwardDelete), Some((0x2E, true)));
    }

    #[test]
    fn cmd_is_pressed_as_ctrl_and_win_as_the_windows_key() {
        let m = |cmd, ctrl, alt, shift, win| Modifiers { cmd, ctrl, alt, shift, win };
        assert_eq!(modifier_vks(m(true, false, false, false, false)), vec![0x11]);
        assert_eq!(modifier_vks(m(true, true, false, false, false)), vec![0x11], "not Ctrl twice");
        assert_eq!(modifier_vks(m(true, false, false, true, false)), vec![0x11, 0x10]);
        assert_eq!(modifier_vks(m(false, false, true, false, false)), vec![0x12]);
        assert_eq!(modifier_vks(m(false, false, false, false, true)), vec![0x5B]);
        assert_eq!(modifier_vks(m(false, false, false, false, false)), Vec::<u16>::new());
    }

    #[test]
    fn mouse_coordinates_span_the_virtual_desktop() {
        // Two 1920x1080 monitors, the second to the left of the primary.
        let virt = (-1920, 0, 3840, 1080);
        assert_eq!(mouse_abs(-1920, 0, virt), (0, 0));
        assert_eq!(mouse_abs(1919, 1079, virt), (65535, 65535));
        let (x, y) = mouse_abs(0, 540, virt);
        assert_eq!(x, (1920.0_f64 * 65535.0 / 3839.0).round() as i32);
        assert_eq!(y, (540.0_f64 * 65535.0 / 1079.0).round() as i32);
    }

    fn display(x: f64, y: f64, w: f64, h: f64, scale: f64) -> DisplayInfo {
        DisplayInfo { id: 1, name: String::new(), bounds: super::super::Rect::new(x, y, w, h), scale_factor: scale, is_primary: x == 0.0 }
    }

    #[test]
    fn a_logical_point_becomes_physical_with_its_displays_scale() {
        // Primary at 150% (2880x1620 px = 1920x1080 pt), a 100% one to its left.
        let displays = [display(0.0, 0.0, 1920.0, 1080.0, 1.5), display(-1920.0, 0.0, 1920.0, 1080.0, 1.0)];
        assert_eq!(to_physical(&displays, Point::new(100.0, 200.0)), (150, 300));
        assert_eq!(to_physical(&displays, Point::new(-100.0, 200.0)), (-100, 200));
        // Off every display: the nearest one's scale.
        assert_eq!(to_physical(&displays, Point::new(-5000.0, 10.0)), (-5000, 10));
    }

    fn switches(device: Consent, user: Consent, desktop_apps: Consent) -> MicSwitches {
        MicSwitches { device, user, desktop_apps }
    }

    #[test]
    fn reads_the_consent_words() {
        assert_eq!(Consent::from_value(Some("Allow")), Consent::Allow);
        assert_eq!(Consent::from_value(Some("Deny")), Consent::Deny);
        assert_eq!(Consent::from_value(Some("deny")), Consent::Deny);
        assert_eq!(Consent::from_value(None), Consent::Missing);
        assert_eq!(Consent::from_value(Some("Prompt")), Consent::Unreadable);
    }

    #[test]
    fn microphone_is_granted_when_every_switch_is_on() {
        use Consent::*;
        assert_eq!(switches(Allow, Allow, Allow).state(), PermissionState::Granted);
        assert_eq!(switches(Allow, Allow, Allow).off_switch(), None);
    }

    #[test]
    fn a_missing_switch_counts_as_on() {
        use Consent::*;
        assert_eq!(switches(Missing, Missing, Missing).state(), PermissionState::Granted);
        assert_eq!(switches(Allow, Missing, Allow).state(), PermissionState::Granted);
    }

    #[test]
    fn one_switch_off_denies_and_is_named() {
        use Consent::*;
        let dev = switches(Deny, Allow, Allow);
        assert_eq!(dev.state(), PermissionState::Denied);
        assert_eq!(dev.off_switch(), Some("Microphone access"));
        let user = switches(Allow, Deny, Allow);
        assert_eq!(user.state(), PermissionState::Denied);
        assert_eq!(user.off_switch(), Some("Let apps access your microphone"));
        let desk = switches(Allow, Allow, Deny);
        assert_eq!(desk.state(), PermissionState::Denied);
        assert_eq!(desk.off_switch(), Some("Let desktop apps access your microphone"));
    }

    #[test]
    fn an_unreadable_switch_is_unknown_unless_another_is_off() {
        use Consent::*;
        assert_eq!(switches(Allow, Unreadable, Allow).state(), PermissionState::Unknown);
        assert_eq!(switches(Unreadable, Allow, Deny).state(), PermissionState::Denied);
    }

    #[test]
    fn only_a_switch_that_is_off_blocks_voice_on_windows() {
        assert!(!blocks_voice(PermissionState::Granted, true));
        assert!(!blocks_voice(PermissionState::Unknown, true));
        assert!(!blocks_voice(PermissionState::NotNeeded, true));
        assert!(blocks_voice(PermissionState::Denied, true));
        assert!(blocks_voice(PermissionState::Restricted, true));
    }

    #[test]
    fn anything_short_of_a_grant_blocks_voice_on_macos() {
        assert!(!blocks_voice(PermissionState::Granted, false));
        assert!(!blocks_voice(PermissionState::NotNeeded, false));
        assert!(blocks_voice(PermissionState::Unknown, false));
        assert!(blocks_voice(PermissionState::NotAsked, false));
        assert!(blocks_voice(PermissionState::Denied, false));
    }

    #[test]
    fn maps_virtual_keys_to_modifiers() {
        assert_eq!(vk_modifier(VK_LCONTROL), Some((ModKey::Control, Side::Left)));
        assert_eq!(vk_modifier(VK_RCONTROL), Some((ModKey::Control, Side::Right)));
        assert_eq!(vk_modifier(VK_LWIN), Some((ModKey::Command, Side::Left)));
        assert_eq!(vk_modifier(VK_RWIN), Some((ModKey::Command, Side::Right)));
        assert_eq!(vk_modifier(VK_LMENU), Some((ModKey::Option, Side::Left)));
        assert_eq!(vk_modifier(VK_RMENU), Some((ModKey::Option, Side::Right)));
        assert_eq!(vk_modifier(VK_LSHIFT), Some((ModKey::Shift, Side::Left)));
        assert_eq!(vk_modifier(VK_RSHIFT), Some((ModKey::Shift, Side::Right)));
        assert_eq!(vk_modifier(VK_CONTROL), Some((ModKey::Control, Side::Left)));
        assert_eq!(vk_modifier(0x41), None); // A
        assert_eq!(vk_modifier(VK_ESCAPE), None);
    }

    fn chord(s: &str) -> WinChord {
        WinChord::new(s.parse().unwrap())
    }

    fn events(c: &mut WinChord, keys: &[(u32, bool)]) -> Vec<Option<HotkeyEvent>> {
        keys.iter().map(|&(vk, down)| c.on_key(KeyEvent::new(vk, down)).event).collect()
    }

    #[test]
    fn ctrl_win_held_and_released_is_down_then_up() {
        let mut c = chord("Ctrl+Win");
        let got = events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true), (VK_LWIN, false), (VK_LCONTROL, false)]);
        assert_eq!(got, vec![None, Some(HotkeyEvent::Down), Some(HotkeyEvent::Up), None]);
    }

    #[test]
    fn win_first_then_ctrl_also_starts_the_hold() {
        let mut c = chord("Ctrl+Win");
        let got = events(&mut c, &[(VK_RWIN, true), (VK_RCONTROL, true), (VK_RCONTROL, false)]);
        assert_eq!(got, vec![None, Some(HotkeyEvent::Down), Some(HotkeyEvent::Up)]);
    }

    #[test]
    fn key_repeat_while_holding_reports_nothing() {
        let mut c = chord("Ctrl+Win");
        events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true)]);
        let got = events(&mut c, &[(VK_LWIN, true), (VK_LWIN, true), (VK_LWIN, true)]);
        assert_eq!(got, vec![None, None, None]);
    }

    #[test]
    fn another_key_during_the_hold_cancels_it() {
        let mut c = chord("Ctrl+Win");
        events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true)]);
        // Ctrl+Win+D: a shortcut, not speech.
        assert_eq!(c.on_key(KeyEvent::new(0x44, true)).event, Some(HotkeyEvent::Cancel));
        // Letting go afterwards is not an Up.
        let got = events(&mut c, &[(0x44, false), (VK_LWIN, false), (VK_LCONTROL, false)]);
        assert_eq!(got, vec![None, None, None]);
    }

    #[test]
    fn an_extra_modifier_cancels_the_hold() {
        let mut c = chord("Ctrl+Win");
        events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true)]);
        assert_eq!(c.on_key(KeyEvent::new(VK_LSHIFT, true)).event, Some(HotkeyEvent::Cancel));
    }

    #[test]
    fn the_mask_key_is_pressed_once_when_a_win_chord_starts() {
        let mut c = chord("Ctrl+Win");
        assert!(!c.on_key(KeyEvent::new(VK_LCONTROL, true)).mask);
        assert!(c.on_key(KeyEvent::new(VK_LWIN, true)).mask);
        assert!(!c.on_key(KeyEvent::new(VK_LWIN, true)).mask, "not again on key repeat");
        assert!(!c.on_key(KeyEvent::new(VK_LWIN, false)).mask);
    }

    #[test]
    fn an_alt_chord_is_masked_too_but_a_ctrl_chord_is_not() {
        let mut alt = chord("Right Alt");
        assert!(alt.on_key(KeyEvent::new(VK_RMENU, true)).mask);
        let mut ctrl = chord("Right Ctrl");
        let out = ctrl.on_key(KeyEvent::new(VK_RCONTROL, true));
        assert_eq!(out, ChordOutcome { event: Some(HotkeyEvent::Down), mask: false });
    }

    #[test]
    fn our_own_mask_key_does_not_cancel_the_hold() {
        let mut c = chord("Ctrl+Win");
        events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true)]);
        let ours = |down| KeyEvent { vk: VK_MASK, scan: 0, down, ours: true };
        assert_eq!(c.on_key(ours(true)), ChordOutcome::default());
        assert_eq!(c.on_key(ours(false)), ChordOutcome::default());
        assert_eq!(c.on_key(KeyEvent::new(VK_LWIN, false)).event, Some(HotkeyEvent::Up));
    }

    #[test]
    fn altgr_counts_as_right_alt_alone() {
        // AltGr arrives as a made-up Left Ctrl (scan 0x21D), then Right Alt.
        let mut c = chord("Right Alt");
        let fake_ctrl = |down| KeyEvent { vk: VK_LCONTROL, scan: SCAN_ALTGR_CTRL, down, ours: false };
        assert_eq!(c.on_key(fake_ctrl(true)).event, None);
        assert_eq!(c.on_key(KeyEvent::new(VK_RMENU, true)).event, Some(HotkeyEvent::Down));
        assert_eq!(c.on_key(fake_ctrl(false)).event, None);
        assert_eq!(c.on_key(KeyEvent::new(VK_RMENU, false)).event, Some(HotkeyEvent::Up));
    }

    fn held(keys: &[(ModKey, Side)]) -> HeldMods {
        let mut h = HeldMods::default();
        for &(k, s) in keys {
            h.set(k, s, true);
        }
        h
    }

    #[test]
    fn resync_changes_nothing_when_the_hook_saw_every_key() {
        let mut c = chord("Ctrl+Win");
        events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true)]);
        let real = held(&[(ModKey::Control, Side::Left), (ModKey::Command, Side::Left)]);
        assert_eq!(c.resync(real), ChordOutcome::default());
        assert!(c.is_active());
    }

    #[test]
    fn resync_clears_a_modifier_whose_key_up_was_missed() {
        let mut c = chord("Ctrl+Win");
        // Shift went down, and its key-up never reached the hook.
        c.on_key(KeyEvent::new(VK_LSHIFT, true));
        let stuck = events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true), (VK_LWIN, false), (VK_LCONTROL, false)]);
        assert_eq!(stuck, vec![None, None, None, None], "the phantom Shift keeps the chord from matching");
        assert_eq!(c.resync(HeldMods::default()), ChordOutcome::default());
        let got = events(&mut c, &[(VK_LCONTROL, true), (VK_LWIN, true), (VK_LWIN, false)]);
        assert_eq!(got, vec![None, Some(HotkeyEvent::Down), Some(HotkeyEvent::Up)]);
    }

    #[test]
    fn resync_finds_a_hold_the_hook_never_saw() {
        // A dead hook: no key events at all, only the timer.
        let mut c = chord("Ctrl+Win");
        let real = held(&[(ModKey::Control, Side::Left), (ModKey::Command, Side::Left)]);
        assert_eq!(c.resync(real), ChordOutcome { event: Some(HotkeyEvent::Down), mask: true });
        assert_eq!(c.resync(real), ChordOutcome::default(), "still held: nothing new");
        assert_eq!(c.resync(HeldMods::default()), ChordOutcome { event: Some(HotkeyEvent::Up), mask: false });
    }

    #[test]
    fn resync_keeps_an_altgr_hold_as_right_alt() {
        let mut c = chord("Right Alt");
        let fake_ctrl = KeyEvent { vk: VK_LCONTROL, scan: SCAN_ALTGR_CTRL, down: true, ours: false };
        c.on_key(fake_ctrl);
        assert_eq!(c.on_key(KeyEvent::new(VK_RMENU, true)).event, Some(HotkeyEvent::Down));
        // Windows reports the made-up Left Ctrl as down too.
        let real = held(&[(ModKey::Control, Side::Left), (ModKey::Option, Side::Right)]);
        assert_eq!(c.resync(real), ChordOutcome::default());
        assert!(c.is_active());
    }

    #[test]
    fn a_side_specific_chord_ignores_the_other_side() {
        let mut c = chord("Right Alt");
        assert_eq!(c.on_key(KeyEvent::new(VK_LMENU, true)).event, None);
    }

    #[test]
    fn esc_is_swallowed_and_reported_once_per_press() {
        let mut f = EscFilter::default();
        assert_eq!(f.on_key(KeyEvent::new(VK_ESCAPE, true)), EscOutcome { swallow: true, pressed: true });
        // Key repeat while it is held: still swallowed, not a new press.
        assert_eq!(f.on_key(KeyEvent::new(VK_ESCAPE, true)), EscOutcome { swallow: true, pressed: false });
        assert_eq!(f.on_key(KeyEvent::new(VK_ESCAPE, false)), EscOutcome { swallow: true, pressed: false });
        assert_eq!(f.on_key(KeyEvent::new(VK_ESCAPE, true)), EscOutcome { swallow: true, pressed: true });
    }

    #[test]
    fn other_keys_and_our_own_esc_pass_through() {
        let mut f = EscFilter::default();
        assert_eq!(f.on_key(KeyEvent::new(0x41, true)), EscOutcome::default());
        let ours = KeyEvent { vk: VK_ESCAPE, scan: 0, down: true, ours: true };
        assert_eq!(f.on_key(ours), EscOutcome::default());
        // Our own Esc did not count as "held".
        assert_eq!(f.on_key(KeyEvent::new(VK_ESCAPE, true)), EscOutcome { swallow: true, pressed: true });
    }
}
