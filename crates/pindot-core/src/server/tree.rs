//! Turning an accessibility tree into the compact text `read_window` and
//! `find` return: one element per line with a ref id, role, name and
//! bounds in screenshot pixels. Pure functions, unit-tested.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::coords::ShotGeometry;
use crate::platform::{AxNode, Rect};

/// Lines per `read_window` page.
pub const PAGE_LINES: usize = 150;
/// Most results `find` returns.
pub const MAX_FIND: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    /// Only elements you can act on (buttons, fields, menus, rows...).
    #[default]
    Interactive,
    /// Every element, indented by depth.
    All,
}

/// A password box. Its value is never shown, listed or searched, so a typed
/// secret cannot come back to the model through a tree read.
pub fn is_secure(node: &AxNode) -> bool {
    node.role == "AXSecureTextField" || node.subrole.as_deref() == Some("AXSecureTextField")
}

/// Short role name: "AXPopUpButton" -> "popupbutton", with a few friendlier
/// names. Subroles that change meaning (password and search fields) win.
pub fn short_role(node: &AxNode) -> String {
    if let Some(sr) = node.subrole.as_deref() {
        match sr {
            "AXSecureTextField" => return "securetextfield".into(),
            "AXSearchField" => return "searchfield".into(),
            "AXCloseButton" => return "button(close)".into(),
            "AXMinimizeButton" => return "button(minimize)".into(),
            "AXZoomButton" => return "button(zoom)".into(),
            "AXFullScreenButton" => return "button(fullscreen)".into(),
            "AXDialog" | "AXSystemDialog" => return "dialog".into(),
            _ => {}
        }
    }
    match node.role.as_str() {
        "AXStaticText" => "text".into(),
        "AXRadioButton" => "radio".into(),
        "AXPopUpButton" => "popup".into(),
        r => r.strip_prefix("AX").unwrap_or(r).to_lowercase(),
    }
}

const INTERACTIVE: &[&str] = &[
    "button", "checkbox", "radio", "popup", "menubutton", "combobox", "textfield", "textarea", "searchfield",
    "securetextfield", "slider", "incrementor", "stepper", "link", "menubaritem", "menuitem", "row",
    "disclosuretriangle", "colorwell", "tab", "dockitem", "segmentedcontrol", "switch", "toggle",
];

pub fn is_interactive(node: &AxNode) -> bool {
    let r = short_role(node);
    let base = r.split('(').next().unwrap_or(&r);
    INTERACTIVE.contains(&base)
}

/// Containers worth a context line in interactive mode (a sheet or dialog
/// tells Claude a modal is open).
fn is_context(node: &AxNode) -> bool {
    matches!(short_role(node).as_str(), "sheet" | "dialog")
}

fn zero_size(node: &AxNode) -> bool {
    node.menu_path.is_none() && node.bounds.is_some_and(|b| b.width <= 0.0 || b.height <= 0.0)
}

/// Roles that often have no name of their own but hold their label as a
/// child text (sidebar rows, table cells, some buttons).
const NAMED_BY_CHILD: &[&str] = &["AXRow", "AXCell", "AXOutlineRow", "AXButton", "AXRadioButton", "AXCheckBox"];
/// How far into a node's subtree a derived name is looked for.
const NAME_LOOKAHEAD: usize = 24;

/// Clean a raw tree in place before it is listed:
/// - a nameless row, cell or button takes the first text found in its
///   subtree as its name (a Finder sidebar row is just a row with a text);
/// - only the last node that says it is focused keeps the flag (some apps,
///   like the Open panel's sidebar, report AXFocused on every cell).
pub fn tidy(nodes: &mut [AxNode]) {
    for i in 0..nodes.len() {
        if !nodes[i].name.is_empty() || !NAMED_BY_CHILD.contains(&nodes[i].role.as_str()) {
            continue;
        }
        let depth = nodes[i].depth;
        let derived = nodes[i + 1..]
            .iter()
            .take(NAME_LOOKAHEAD)
            .take_while(|n| n.depth > depth)
            .find_map(|n| match n.role.as_str() {
                "AXStaticText" | "AXTextField" => {
                    let t = if n.name.is_empty() { n.value.as_deref().unwrap_or("") } else { n.name.as_str() };
                    (!t.trim().is_empty()).then(|| t.trim().to_string())
                }
                _ => None,
            });
        if let Some(name) = derived {
            nodes[i].name = name;
        }
    }
    if let Some(last) = nodes.iter().rposition(|n| n.focused) {
        for n in &mut nodes[..last] {
            n.focused = false;
        }
    }
}

/// Are these two nodes the same control on screen, seen twice? Windows
/// exposes a dialog both as its own window and inside the window that owns
/// it, so a search over an app's windows meets its buttons twice. Nodes
/// without a place on screen (menu items) are never the same.
pub fn same_element(a: &AxNode, b: &AxNode) -> bool {
    match (a.bounds, b.bounds) {
        (Some(x), Some(y)) => !x.is_empty() && x == y && a.role == b.role && a.name == b.name,
        _ => false,
    }
}

/// Which nodes a listing shows, as indexes into `nodes`.
pub fn select(nodes: &[AxNode], filter: Filter) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| match filter {
            Filter::All => true,
            Filter::Interactive => {
                !zero_size(n) && (is_interactive(n) || n.focused || is_context(n)) && !(n.role == "AXMenuItem" && n.name.is_empty())
            }
        })
        .map(|(i, _)| i)
        .collect()
}

fn quote(s: &str, max: usize) -> String {
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut t: String = one_line.chars().take(max).collect();
    if one_line.chars().count() > max {
        t.push('…');
    }
    format!("\"{}\"", t.replace('"', "'"))
}

/// One element as a line: `[e12] button "New Document" (412,300 96x24)`.
/// Bounds are pixels of the latest screenshot (`geom`); elements outside the
/// window or the image are flagged `offscreen`.
pub fn format_line(ref_id: &str, node: &AxNode, geom: Option<&ShotGeometry>, window: Option<&Rect>, indent: bool) -> String {
    let mut s = String::new();
    if indent {
        for _ in 0..node.depth.min(24) {
            s.push_str("  ");
        }
    }
    s.push_str(&format!("[{ref_id}] {}", short_role(node)));
    let label = node.menu_path.clone().unwrap_or_else(|| node.name.clone());
    if !label.is_empty() {
        s.push(' ');
        s.push_str(&quote(&label, 80));
    }
    if let Some(v) = node.value.as_deref().filter(|v| !v.is_empty() && *v != node.name && !is_secure(node)) {
        s.push_str(&format!(" value={}", quote(v, 60)));
    }
    if let Some(b) = node.bounds {
        match geom {
            Some(g) => {
                let (x, y, w, h) = g.project_rect(b);
                s.push_str(&format!(" ({x},{y} {w}x{h})"));
                let c = b.center();
                // Menu bar titles live outside every window.
                let in_window = node.role == "AXMenuBarItem" || window.is_none_or(|w| w.contains(c));
                let (cx, cy) = g.project(c);
                if !in_window || !g.in_image(cx, cy) {
                    s.push_str(" offscreen");
                }
            }
            None => s.push_str(" (no screenshot yet)"),
        }
    }
    if let Some(k) = &node.shortcut {
        s.push_str(&format!(" {k}"));
    }
    if !node.enabled {
        s.push_str(" disabled");
    }
    if node.focused {
        s.push_str(" focused");
    }
    s
}

/// Page `page` (1-based) of `lines`. Returns the lines and the page count.
pub fn paginate(lines: &[String], page: usize, per_page: usize) -> (&[String], usize) {
    let pages = lines.len().div_ceil(per_page).max(1);
    let page = page.clamp(1, pages);
    let start = (page - 1) * per_page;
    let end = (start + per_page).min(lines.len());
    (&lines[start.min(lines.len())..end], pages)
}

/// How well `node` matches `query`: every word must appear in its name,
/// value, role or menu path. `None` = no match.
pub fn score(node: &AxNode, query: &str) -> Option<i32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let name = node.name.to_lowercase();
    let hay = format!(
        "{} {} {} {}",
        name,
        if is_secure(node) { String::new() } else { node.value.as_deref().unwrap_or("").to_lowercase() },
        short_role(node),
        node.menu_path.as_deref().unwrap_or("").to_lowercase()
    );
    if !q.split_whitespace().all(|w| hay.contains(w)) {
        return None;
    }
    let mut s = 10;
    if name == q || node.menu_path.as_deref().is_some_and(|p| p.to_lowercase() == q) {
        s += 50;
    } else if name.starts_with(&q) {
        s += 20;
    } else if name.contains(&q) {
        s += 10;
    }
    if is_interactive(node) {
        s += 15;
    }
    if node.enabled {
        s += 5;
    }
    if zero_size(node) {
        s -= 30;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::coords::{MAX_LONG_EDGE, ShotGeometry};
    use crate::platform::{DisplayInfo, Rect};

    fn node(role: &str, name: &str, bounds: Option<Rect>) -> AxNode {
        AxNode {
            handle: Arc::new(()),
            depth: 1,
            role: role.into(),
            subrole: None,
            name: name.into(),
            value: None,
            bounds,
            enabled: true,
            focused: false,
            shortcut: None,
            menu_path: None,
            menu_root: None,
        }
    }

    fn geom() -> ShotGeometry {
        let d = DisplayInfo {
            id: 1,
            name: "d".into(),
            bounds: Rect::new(0.0, 0.0, 1440.0, 900.0),
            scale_factor: 2.0,
            is_primary: true,
        };
        ShotGeometry::new(d.clone(), 0, d.bounds, MAX_LONG_EDGE)
    }

    #[test]
    fn line_has_ref_role_name_and_image_bounds() {
        let n = node("AXButton", "New Document", Some(Rect::new(720.0, 450.0, 90.0, 18.0)));
        let l = format_line("e3", &n, Some(&geom()), None, false);
        assert_eq!(l, "[e3] button \"New Document\" (640,400 80x16)");
    }

    #[test]
    fn a_password_boxs_value_is_never_listed_or_searchable() {
        let mut n = node("AXTextField", "Password", None);
        n.subrole = Some("AXSecureTextField".into());
        n.value = Some("hunter2-secret".into());
        let l = format_line("e1", &n, None, None, false);
        assert!(!l.contains("hunter2"), "{l}");
        assert!(score(&n, "hunter2").is_none());
        assert!(score(&n, "password").is_some());
    }

    #[test]
    fn menu_items_show_their_path_and_shortcut() {
        let mut n = node("AXMenuItem", "New", None);
        n.menu_path = Some("File > New".into());
        n.shortcut = Some("⌘N".into());
        assert_eq!(format_line("e9", &n, Some(&geom()), None, false), "[e9] menuitem \"File > New\" ⌘N");
    }

    #[test]
    fn the_same_control_seen_through_two_windows_is_one_element() {
        let b = Some(Rect::new(609.0, 371.0, 59.0, 21.0));
        let in_dialog = node("AXButton", "Don't save", b);
        let in_owner = node("AXButton", "Don't save", b);
        assert!(same_element(&in_dialog, &in_owner));
        // Another place, another name or another role: a different control.
        assert!(!same_element(&in_dialog, &node("AXButton", "Don't save", Some(Rect::new(10.0, 371.0, 59.0, 21.0)))));
        assert!(!same_element(&in_dialog, &node("AXButton", "Save", b)));
        assert!(!same_element(&in_dialog, &node("AXStaticText", "Don't save", b)));
        // Menu items have no place on screen: two "New" items are two items.
        assert!(!same_element(&node("AXMenuItem", "New", None), &node("AXMenuItem", "New", None)));
    }

    #[test]
    fn interactive_filter_keeps_controls_and_drops_layout() {
        let nodes = vec![
            node("AXGroup", "", Some(Rect::new(0.0, 0.0, 100.0, 100.0))),
            node("AXButton", "OK", Some(Rect::new(10.0, 10.0, 40.0, 20.0))),
            node("AXStaticText", "Hello", Some(Rect::new(10.0, 40.0, 40.0, 20.0))),
            node("AXButton", "hidden", Some(Rect::new(10.0, 40.0, 0.0, 0.0))),
            node("AXTextArea", "", Some(Rect::new(0.0, 60.0, 100.0, 40.0))),
        ];
        assert_eq!(select(&nodes, Filter::Interactive), vec![1, 4]);
        assert_eq!(select(&nodes, Filter::All).len(), 5);
    }

    #[test]
    fn pagination_splits_and_clamps() {
        let lines: Vec<String> = (0..320).map(|i| format!("l{i}")).collect();
        let (p1, n) = paginate(&lines, 1, 150);
        assert_eq!((p1.len(), n), (150, 3));
        assert_eq!(p1[0], "l0");
        let (p3, _) = paginate(&lines, 3, 150);
        assert_eq!(p3.len(), 20);
        assert_eq!(p3[0], "l300");
        let (p9, _) = paginate(&lines, 9, 150);
        assert_eq!(p9[0], "l300", "past the end clamps to the last page");
        let empty: Vec<String> = vec![];
        assert_eq!(paginate(&empty, 1, 150).1, 1);
    }

    #[test]
    fn find_scores_exact_names_first() {
        let exact = node("AXButton", "Save", Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let partial = node("AXButton", "Save As…", Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let text = node("AXStaticText", "Don't save changes", Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let other = node("AXButton", "Cancel", Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        let s = |n: &AxNode| score(n, "save");
        assert!(s(&exact).unwrap() > s(&partial).unwrap());
        assert!(s(&partial).unwrap() > s(&text).unwrap());
        assert_eq!(s(&other), None);
        // Every word must match, in any field.
        let mut item = node("AXMenuItem", "New", None);
        item.menu_path = Some("File > New".into());
        assert!(score(&item, "file new").is_some());
        assert!(score(&item, "file open").is_none());
    }

    #[test]
    fn nameless_rows_take_their_child_text() {
        let b = Some(Rect::new(0.0, 0.0, 80.0, 20.0));
        let mut row = node("AXRow", "", b);
        row.depth = 3;
        let mut cell = node("AXCell", "", b);
        cell.depth = 4;
        let mut text = node("AXStaticText", "Desktop", b);
        text.depth = 5;
        let mut row2 = node("AXRow", "", b);
        row2.depth = 3; // a sibling: its text must not leak into the row above
        let mut text2 = node("AXStaticText", "Downloads", b);
        text2.depth = 4;
        let mut empty_row = node("AXRow", "", b);
        empty_row.depth = 3;
        let mut nodes = vec![row, cell, text, row2, text2, empty_row];
        tidy(&mut nodes);
        assert_eq!(nodes[0].name, "Desktop");
        assert_eq!(nodes[1].name, "Desktop");
        assert_eq!(nodes[3].name, "Downloads");
        assert_eq!(nodes[5].name, "", "no text in the subtree, no name");
        // Named nodes keep their own name.
        let mut btn = node("AXButton", "OK", b);
        btn.depth = 1;
        let mut t = node("AXStaticText", "Other", b);
        t.depth = 2;
        let mut nodes = vec![btn, t];
        tidy(&mut nodes);
        assert_eq!(nodes[0].name, "OK");
    }

    #[test]
    fn only_the_last_focused_node_stays_focused() {
        let b = Some(Rect::new(0.0, 0.0, 80.0, 20.0));
        let mut nodes: Vec<AxNode> = (0..4)
            .map(|_| {
                let mut c = node("AXCell", "", b);
                c.focused = true;
                c
            })
            .collect();
        let mut list = node("AXList", "", b);
        list.focused = true;
        nodes.push(list);
        nodes.push(node("AXButton", "Open", b));
        tidy(&mut nodes);
        let focused: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.focused).map(|(i, _)| i).collect();
        assert_eq!(focused, vec![4]);
        // Unfocused cells drop out of the interactive listing.
        assert_eq!(select(&nodes, Filter::Interactive), vec![4, 5]);
    }

    #[test]
    fn offscreen_elements_are_flagged() {
        let n = node("AXButton", "Far", Some(Rect::new(2000.0, 100.0, 20.0, 20.0)));
        assert!(format_line("e1", &n, Some(&geom()), None, false).ends_with("offscreen"));
        let w = Rect::new(0.0, 0.0, 500.0, 500.0);
        let n2 = node("AXButton", "Scrolled away", Some(Rect::new(600.0, 100.0, 20.0, 20.0)));
        assert!(format_line("e2", &n2, Some(&geom()), Some(&w), false).ends_with("offscreen"));
    }
}
