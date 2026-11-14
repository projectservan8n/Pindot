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
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
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
                "ctrl" | "control" | "⌃" => mods.ctrl = true,
                "alt" | "option" | "opt" | "⌥" => mods.alt = true,
                "shift" | "⇧" => mods.shift = true,
                other => return Err(format!("unknown modifier {other:?} in {s:?} (use cmd, ctrl, alt/option, shift)")),
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

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (on, name) in [(m.cmd, "cmd"), (m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift")] {
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
