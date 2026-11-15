//! Key combinations for the `key` tool, e.g. "cmd+shift+s", "return",
//! "esc", "f5". Platform-neutral: each `Input` maps `KeyName` to its own
//! key codes. "cmd" means Command on macOS (the Windows mapping decides
//! what it means there).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyName {
    /// A printable character key, lowercase for letters.
    Char(char),
    Return,
    Tab,
    Space,
    /// The key labelled "delete" on a Mac (deletes backwards).
    Backspace,
    /// Forward delete.
    ForwardDelete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Command on macOS. On Windows it is pressed as Ctrl, which is what a
    /// Mac shortcut such as cmd+s means there.
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key ("win+r"). On macOS it is Command.
    pub win: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Modifiers,
    pub key: KeyName,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key combo".into());
        }
        // "cmd++" means Command and the plus key.
        let (body, plus_key) = match s.strip_suffix("++") {
            Some(b) => (b, true),
            None => (s, s == "+"),
        };
        let mut parts: Vec<&str> = if body.is_empty() || body == "+" { vec![] } else { body.split('+').map(str::trim).collect() };
        if plus_key {
            parts.push("+");
        }
        let Some(last) = parts.pop() else {
            return Err(format!("no key in {s:?}"));
        };
        let mut mods = Modifiers::default();
        for m in parts {
            match m.to_lowercase().as_str() {
                "cmd" | "command" | "meta" | "super" | "⌘" => mods.cmd = true,
                "win" | "windows" => mods.win = true,
                "ctrl" | "control" | "⌃" => mods.ctrl = true,
                "alt" | "option" | "opt" | "⌥" => mods.alt = true,
                "shift" | "⇧" => mods.shift = true,
                other => {
                    return Err(format!("unknown modifier {other:?} in {s:?} (use cmd, ctrl, alt/option, shift, win)"));
                }
            }
        }
        let key = parse_key(last).ok_or_else(|| format!("unknown key {last:?} in {s:?}"))?;
        Ok(Self { mods, key })
    }

    pub fn is_plain_escape(&self) -> bool {
        self.key == KeyName::Escape && self.mods == Modifiers::default()
    }
}

fn parse_key(k: &str) -> Option<KeyName> {
    let lower = k.to_lowercase();
    let named = match lower.as_str() {
        "return" | "enter" | "↩" => KeyName::Return,
        "tab" | "⇥" => KeyName::Tab,
        "space" | "spacebar" => KeyName::Space,
        "backspace" | "delete" | "del" | "⌫" => KeyName::Backspace,
        "forwarddelete" | "forward_delete" | "fwddelete" | "⌦" => KeyName::ForwardDelete,
        "esc" | "escape" | "⎋" => KeyName::Escape,
        "up" | "arrowup" | "uparrow" | "↑" => KeyName::Up,
        "down" | "arrowdown" | "downarrow" | "↓" => KeyName::Down,
        "left" | "arrowleft" | "leftarrow" | "←" => KeyName::Left,
        "right" | "arrowright" | "rightarrow" | "→" => KeyName::Right,
        "home" => KeyName::Home,
        "end" => KeyName::End,
        "pageup" | "pgup" => KeyName::PageUp,
        "pagedown" | "pgdn" => KeyName::PageDown,
        "plus" => KeyName::Char('+'),
        "minus" => KeyName::Char('-'),
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                if (1..=20).contains(&n) {
                    return Some(KeyName::F(n));
                }
            }
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyName::Char(c.to_lowercase().next().unwrap_or(c))
        }
    };
    Some(named)
}

impl KeyCombo {
    /// Key-cap labels for the overlay, modifiers first in the platform's
    /// order (Apple: Control, Option, Shift, Command).
    pub fn caps(&self) -> Vec<String> {
        let mac = cfg!(target_os = "macos");
        let m = self.mods;
        let mods = if mac {
            [(m.ctrl, "⌃"), (m.alt, "⌥"), (m.shift, "⇧"), (m.cmd || m.win, "⌘")]
        } else {
            // Windows presses `cmd` as Ctrl (see `windows_logic::modifier_vks`).
            [(m.ctrl || m.cmd, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.win, "Win")]
        };
        let mut caps: Vec<String> = mods.into_iter().filter(|(on, _)| *on).map(|(_, cap)| cap.to_string()).collect();
        caps.push(key_cap(self.key, mac));
        caps
    }
}

fn key_cap(k: KeyName, mac: bool) -> String {
    let pick = |a: &str, w: &str| (if mac { a } else { w }).to_string();
    match k {
        KeyName::Char(c) => c.to_uppercase().collect(),
        KeyName::F(n) => format!("F{n}"),
        KeyName::Space => "Space".into(),
        KeyName::Escape => "esc".into(),
        KeyName::Return => pick("↩", "Enter"),
        KeyName::Tab => pick("⇥", "Tab"),
        KeyName::Backspace => pick("⌫", "Backspace"),
        KeyName::ForwardDelete => pick("⌦", "Delete"),
        KeyName::Up => "↑".into(),
        KeyName::Down => "↓".into(),
        KeyName::Left => "←".into(),
        KeyName::Right => "→".into(),
        KeyName::Home => pick("↖", "Home"),
        KeyName::End => pick("↘", "End"),
        KeyName::PageUp => pick("⇞", "PgUp"),
        KeyName::PageDown => pick("⇟", "PgDn"),
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (on, name) in [(m.cmd, "cmd"), (m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift"), (m.win, "win")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            KeyName::Char(c) => write!(f, "{c}"),
            KeyName::F(n) => write!(f, "f{n}"),
            other => write!(f, "{}", format!("{other:?}").to_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn key_caps_use_mac_symbols_in_apple_order() {
        let caps = |s: &str| KeyCombo::parse(s).unwrap().caps();
        assert_eq!(caps("cmd+shift+s"), ["⇧", "⌘", "S"]);
        assert_eq!(caps("cmd+ctrl+alt+shift+q"), ["⌃", "⌥", "⇧", "⌘", "Q"]);
        assert_eq!(caps("return"), ["↩"]);
        assert_eq!(caps("cmd+backspace"), ["⌘", "⌫"]);
        assert_eq!(caps("esc"), ["esc"]);
        assert_eq!(caps("option+left"), ["⌥", "←"]);
        assert_eq!(caps("f12"), ["F12"]);
        assert_eq!(caps("space"), ["Space"]);
        assert_eq!(caps("cmd++"), ["⌘", "+"]);
    }

    #[test]
    fn parses_the_windows_key() {
        let c = KeyCombo::parse("win+r").unwrap();
        assert!(c.mods.win && !c.mods.cmd && !c.mods.ctrl);
        assert_eq!(c.key, KeyName::Char('r'));
        assert!(KeyCombo::parse("Windows+d").unwrap().mods.win);
        assert_eq!(KeyCombo::parse(&c.to_string()).unwrap(), c);
    }

    #[test]
    #[cfg(windows)]
    fn key_caps_on_windows_show_cmd_as_ctrl() {
        let caps = |s: &str| KeyCombo::parse(s).unwrap().caps();
        assert_eq!(caps("cmd+shift+s"), ["Ctrl", "Shift", "S"]);
        assert_eq!(caps("cmd+ctrl+c"), ["Ctrl", "C"]);
        assert_eq!(caps("win+r"), ["Win", "R"]);
        assert_eq!(caps("alt+f4"), ["Alt", "F4"]);
    }

    #[test]
    fn parses_common_combos() {
        let c = KeyCombo::parse("cmd+shift+S").unwrap();
        assert!(c.mods.cmd && c.mods.shift && !c.mods.ctrl && !c.mods.alt);
        assert_eq!(c.key, KeyName::Char('s'));
        assert_eq!(KeyCombo::parse("Return").unwrap().key, KeyName::Return);
        assert_eq!(KeyCombo::parse("enter").unwrap().key, KeyName::Return);
        assert!(KeyCombo::parse("esc").unwrap().is_plain_escape());
        assert!(!KeyCombo::parse("cmd+esc").unwrap().is_plain_escape());
        assert_eq!(KeyCombo::parse("option+left").unwrap().key, KeyName::Left);
        assert!(KeyCombo::parse("option+left").unwrap().mods.alt);
        assert_eq!(KeyCombo::parse("f12").unwrap().key, KeyName::F(12));
        assert_eq!(KeyCombo::parse("cmd++").unwrap().key, KeyName::Char('+'));
        assert!(KeyCombo::parse("cmd++").unwrap().mods.cmd);
        assert_eq!(KeyCombo::parse("+").unwrap().key, KeyName::Char('+'));
        assert_eq!(KeyCombo::parse("ctrl + a").unwrap().key, KeyName::Char('a'));
        assert_eq!(KeyCombo::parse("delete").unwrap().key, KeyName::Backspace);
    }

    #[test]
    fn rejects_garbage() {
        assert!(KeyCombo::parse("").is_err());
        assert!(KeyCombo::parse("hyper+a").is_err());
        assert!(KeyCombo::parse("cmd+banana").is_err());
        assert!(KeyCombo::parse("f99").is_err());
    }

    #[test]
    fn displays_round_trip() {
        for s in ["cmd+shift+s", "return", "alt+left", "f5", "ctrl+a"] {
            let c = KeyCombo::parse(s).unwrap();
            assert_eq!(KeyCombo::parse(&c.to_string()).unwrap(), c, "{s}");
        }
    }
}
