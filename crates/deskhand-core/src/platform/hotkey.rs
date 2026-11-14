//! The push-to-talk binding: which modifier keys to hold, how it is written
//! in settings, and the pure chord tracker every platform's key hook feeds.
//!
//! A binding is one to four modifier keys held together, e.g. `Fn`,
//! `Right Option`, `Ctrl+Cmd` (macOS) or `Ctrl+Win`, `Right Alt` (Windows).
//! Matching is exact: the chord is "held" while exactly those modifiers are
//! down (a side-less key such as `Ctrl` accepts either side). Pressing any
//! other key, or adding another modifier, while the chord is held means the
//! user is typing a shortcut, so the hold is cancelled, not sent.
//!
//! Non-modifier keys (`F13`, `Space`) are not supported as bindings yet.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::HotkeyEvent;

/// A modifier key kind. `Command` is the Windows key on Windows, `Option`
/// is Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModKey {
    Control,
    Option,
    Shift,
    Command,
    Fn,
}

impl ModKey {
    pub const ALL: [ModKey; 5] = [ModKey::Control, ModKey::Option, ModKey::Shift, ModKey::Command, ModKey::Fn];
}

/// Which physical key of a left/right pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Either,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChordKey {
    pub key: ModKey,
    pub side: Side,
}

/// The OS a label or a check is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) { Os::Windows } else { Os::MacOs }
    }
}

/// A hold-to-talk chord of modifier keys. Stored in settings as text
/// (`"Right Option"`, `"Ctrl+Cmd"`); see [`HotkeyBinding::from_str`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotkeyBinding {
    keys: Vec<ChordKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    #[error("the shortcut is empty; hold at least one modifier key")]
    Empty,
    #[error("\"{0}\" is not a modifier key. Use Fn, Ctrl, Option/Alt, Shift or Cmd/Win, optionally with Left or Right")]
    NotAModifier(String),
    #[error("{0} appears twice")]
    Duplicate(String),
    #[error("Fn has no left or right side")]
    FnSide,
    #[error("hold at most four keys")]
    TooMany,
    #[error("the Fn key never reaches Windows (the keyboard handles it); pick another chord")]
    FnOnWindows,
    #[error("Shift alone would stop you typing capitals; add another key")]
    ShiftAlone,
}

impl HotkeyBinding {
    pub fn new(mut keys: Vec<ChordKey>) -> Result<Self, BindingError> {
        if keys.is_empty() {
            return Err(BindingError::Empty);
        }
        if keys.len() > 4 {
            return Err(BindingError::TooMany);
        }
        keys.sort();
        for w in keys.windows(2) {
            if w[0].key == w[1].key {
                return Err(BindingError::Duplicate(key_word(w[0].key, Os::current()).into()));
            }
        }
        if keys.iter().any(|k| k.key == ModKey::Fn && k.side != Side::Either) {
            return Err(BindingError::FnSide);
        }
        if keys.len() == 1 && keys[0].key == ModKey::Shift {
            return Err(BindingError::ShiftAlone);
        }
        Ok(Self { keys })
    }

    /// Hold Fn (macOS default).
    pub fn fn_key() -> Self {
        Self { keys: vec![ChordKey { key: ModKey::Fn, side: Side::Either }] }
    }

    /// Hold Ctrl+Win (Windows default).
    pub fn ctrl_win() -> Self {
        Self {
            keys: vec![
                ChordKey { key: ModKey::Control, side: Side::Either },
                ChordKey { key: ModKey::Command, side: Side::Either },
            ],
        }
    }

    pub fn default_for(os: Os) -> Self {
        match os {
            Os::MacOs => Self::fn_key(),
            Os::Windows => Self::ctrl_win(),
        }
    }

    pub fn platform_default() -> Self {
        Self::default_for(Os::current())
    }

    pub fn keys(&self) -> &[ChordKey] {
        &self.keys
    }

    pub fn has_fn(&self) -> bool {
        self.keys.iter().any(|k| k.key == ModKey::Fn)
    }

    /// Is this binding usable on `os`?
    pub fn check_for(&self, os: Os) -> Result<(), BindingError> {
        if os == Os::Windows && self.has_fn() {
            return Err(BindingError::FnOnWindows);
        }
        Ok(())
    }

    /// Exact match: the chord's keys are down (on the right side) and no
    /// other modifier is.
    pub fn matches(&self, held: &HeldMods) -> bool {
        ModKey::ALL.iter().all(|&kind| {
            let (l, r) = held.get(kind);
            match self.keys.iter().find(|k| k.key == kind) {
                None => !l && !r,
                Some(k) => match k.side {
                    Side::Either => l || r,
                    Side::Left => l && !r,
                    Side::Right => r && !l,
                },
            }
        })
    }

    /// A caution for the settings screen: a lone Ctrl, Option or Cmd is part
    /// of many shortcuts, and each shortcut briefly starts a hold (which stops
    /// speech) before it is cancelled.
    pub fn caution(&self) -> Option<&'static str> {
        match self.keys.as_slice() {
            [k] if k.key != ModKey::Fn && k.side == Side::Either => Some(
                "This key is part of many shortcuts. Each shortcut briefly counts as a hold and stops speech. A Right-side key or a two-key chord avoids that.",
            ),
            _ => None,
        }
    }

    /// The label for `os`, e.g. "Right Option" on macOS, "Right Alt" on Windows.
    pub fn label_for(&self, os: Os) -> String {
        self.keys
            .iter()
            .map(|k| {
                let word = key_word(k.key, os);
                match k.side {
                    Side::Either => word.to_string(),
                    Side::Left => format!("Left {word}"),
                    Side::Right => format!("Right {word}"),
                }
            })
            .collect::<Vec<_>>()
            .join("+")
    }
}

fn key_word(key: ModKey, os: Os) -> &'static str {
    match (key, os) {
        (ModKey::Control, _) => "Ctrl",
        (ModKey::Option, Os::MacOs) => "Option",
        (ModKey::Option, Os::Windows) => "Alt",
        (ModKey::Shift, _) => "Shift",
        (ModKey::Command, Os::MacOs) => "Cmd",
        (ModKey::Command, Os::Windows) => "Win",
        (ModKey::Fn, _) => "Fn",
    }
}

fn parse_key(token: &str) -> Result<ChordKey, BindingError> {
    let raw = token.trim();
    let lower = raw.to_ascii_lowercase();
    let squashed: String = lower.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect();
    let (side, name) = if let Some(rest) = squashed.strip_prefix("left") {
        (Side::Left, rest)
    } else if let Some(rest) = squashed.strip_prefix("right") {
        (Side::Right, rest)
    } else {
        (Side::Either, squashed.as_str())
    };
    // Short forms: lctrl, ralt, rcmd, lshift...
    let (side, name) = match (side, name) {
        (Side::Either, n) if n.len() > 1 && (n.starts_with('l') || n.starts_with('r')) && short_name(&n[1..]).is_some() => {
            (if n.starts_with('l') { Side::Left } else { Side::Right }, &n[1..])
        }
        other => other,
    };
    let key = short_name(name).ok_or_else(|| BindingError::NotAModifier(raw.to_string()))?;
    if key == ModKey::Fn && side != Side::Either {
        return Err(BindingError::FnSide);
    }
    Ok(ChordKey { key, side })
}

fn short_name(name: &str) -> Option<ModKey> {
    Some(match name {
        "ctrl" | "control" | "ctl" => ModKey::Control,
        "opt" | "option" | "alt" | "altgr" => ModKey::Option,
        "shift" => ModKey::Shift,
        "cmd" | "command" | "win" | "windows" | "super" | "meta" => ModKey::Command,
        "fn" | "globe" | "function" => ModKey::Fn,
        _ => return None,
    })
}

impl FromStr for HotkeyBinding {
    type Err = BindingError;

    /// Parses `"Fn"`, `"Right Option"`, `"Ctrl+Cmd"`, `"ctrl + win"`,
    /// `"RightAlt"`, `"rcmd"`. Case and spaces don't matter.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let tokens: Vec<&str> = s.split('+').map(str::trim).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return Err(BindingError::Empty);
        }
        let keys = tokens.into_iter().map(parse_key).collect::<Result<Vec<_>, _>>()?;
        HotkeyBinding::new(keys)
    }
}

impl fmt::Display for HotkeyBinding {
    /// The settings form: macOS words (`Option`, `Cmd`), which parse on both
    /// platforms.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label_for(Os::MacOs))
    }
}

impl Serialize for HotkeyBinding {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for HotkeyBinding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Which modifier keys are physically down, left and right separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeldMods {
    pub l_ctrl: bool,
    pub r_ctrl: bool,
    pub l_opt: bool,
    pub r_opt: bool,
    pub l_shift: bool,
    pub r_shift: bool,
    pub l_cmd: bool,
    pub r_cmd: bool,
    pub fn_key: bool,
}

impl HeldMods {
    /// (left, right) for a kind; Fn reports on the left.
    pub fn get(&self, kind: ModKey) -> (bool, bool) {
        match kind {
            ModKey::Control => (self.l_ctrl, self.r_ctrl),
            ModKey::Option => (self.l_opt, self.r_opt),
            ModKey::Shift => (self.l_shift, self.r_shift),
            ModKey::Command => (self.l_cmd, self.r_cmd),
            ModKey::Fn => (self.fn_key, false),
        }
    }

    pub fn set(&mut self, kind: ModKey, side: Side, down: bool) {
        let (l, r) = match kind {
            ModKey::Control => (&mut self.l_ctrl, &mut self.r_ctrl),
            ModKey::Option => (&mut self.l_opt, &mut self.r_opt),
            ModKey::Shift => (&mut self.l_shift, &mut self.r_shift),
            ModKey::Command => (&mut self.l_cmd, &mut self.r_cmd),
            ModKey::Fn => {
                self.fn_key = down;
                return;
            }
        };
        match side {
            Side::Left | Side::Either => *l = down,
            Side::Right => *r = down,
        }
    }

    pub fn none(&self) -> bool {
        *self == HeldMods::default()
    }

    /// Decode macOS `CGEventFlags` (generic and device-dependent bits). A
    /// generic bit without either device bit (some synthetic events) counts
    /// as the left key.
    pub fn from_mac_flags(flags: u64) -> HeldMods {
        const CONTROL: u64 = 0x40000;
        const SHIFT: u64 = 0x20000;
        const ALTERNATE: u64 = 0x80000;
        const COMMAND: u64 = 0x100000;
        const SECONDARY_FN: u64 = 0x800000;
        const D_LCTL: u64 = 0x1;
        const D_LSHIFT: u64 = 0x2;
        const D_RSHIFT: u64 = 0x4;
        const D_LCMD: u64 = 0x8;
        const D_RCMD: u64 = 0x10;
        const D_LALT: u64 = 0x20;
        const D_RALT: u64 = 0x40;
        const D_RCTL: u64 = 0x2000;
        let pair = |generic: u64, left: u64, right: u64| -> (bool, bool) {
            if flags & generic == 0 {
                return (false, false);
            }
            let (l, r) = (flags & left != 0, flags & right != 0);
            if !l && !r { (true, false) } else { (l, r) }
        };
        let (l_ctrl, r_ctrl) = pair(CONTROL, D_LCTL, D_RCTL);
        let (l_opt, r_opt) = pair(ALTERNATE, D_LALT, D_RALT);
        let (l_shift, r_shift) = pair(SHIFT, D_LSHIFT, D_RSHIFT);
        let (l_cmd, r_cmd) = pair(COMMAND, D_LCMD, D_RCMD);
        HeldMods { l_ctrl, r_ctrl, l_opt, r_opt, l_shift, r_shift, l_cmd, r_cmd, fn_key: flags & SECONDARY_FN != 0 }
    }
}

/// Turns modifier-state updates and key presses into Down / Up / Cancel.
/// Pure: every platform hook feeds it, and it is unit-tested here.
#[derive(Debug, Clone)]
pub struct ChordTracker {
    binding: HotkeyBinding,
    held: HeldMods,
    active: bool,
    /// After a cancel, wait until every modifier is up before the next Down.
    blocked: bool,
}

impl ChordTracker {
    pub fn new(binding: HotkeyBinding) -> Self {
        Self { binding, held: HeldMods::default(), active: false, blocked: false }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn binding(&self) -> &HotkeyBinding {
        &self.binding
    }

    /// The modifier state changed (macOS `flagsChanged`, a Windows modifier
    /// key down/up).
    pub fn on_mods(&mut self, held: HeldMods) -> Option<HotkeyEvent> {
        let prev = self.held;
        self.held = held;
        if held.none() {
            self.blocked = false;
        }
        let matches = self.binding.matches(&held);
        if self.active {
            if matches {
                return None;
            }
            self.active = false;
            // A modifier added on top means a shortcut (Ctrl+Shift+Cmd+4);
            // otherwise a chord key was let go and the hold ended normally.
            let added = ModKey::ALL.iter().any(|&kind| {
                let (pl, pr) = prev.get(kind);
                let (nl, nr) = held.get(kind);
                (!pl && nl) || (!pr && nr)
            });
            if added {
                self.blocked = true;
                return Some(HotkeyEvent::Cancel);
            }
            return Some(HotkeyEvent::Up);
        }
        if matches && !self.blocked {
            self.active = true;
            return Some(HotkeyEvent::Down);
        }
        None
    }

    /// A non-modifier key went down. While the chord is held that means a
    /// shortcut (Option+e, Ctrl+Cmd+Q), so the hold is cancelled.
    pub fn on_other_key(&mut self) -> Option<HotkeyEvent> {
        if self.active {
            self.active = false;
            self.blocked = true;
            return Some(HotkeyEvent::Cancel);
        }
        // A key pressed with modifiers down (but no chord) also blocks a chord
        // formed later in the same shortcut.
        if !self.held.none() {
            self.blocked = true;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> HotkeyBinding {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn parses_names_aliases_and_sides() {
        assert_eq!(b("Fn"), HotkeyBinding::fn_key());
        assert_eq!(b("globe"), HotkeyBinding::fn_key());
        assert_eq!(b("Ctrl+Win"), HotkeyBinding::ctrl_win());
        assert_eq!(b("ctrl + cmd"), HotkeyBinding::ctrl_win());
        assert_eq!(b("Command+Control"), HotkeyBinding::ctrl_win());
        let ropt = vec![ChordKey { key: ModKey::Option, side: Side::Right }];
        for s in ["Right Option", "right-option", "RightAlt", "ralt", "Right Alt", "ROPT"] {
            assert_eq!(b(s).keys(), &ropt[..], "{s}");
        }
        assert_eq!(b("lcmd").keys(), &[ChordKey { key: ModKey::Command, side: Side::Left }][..]);
        assert_eq!(b("Left Shift + Left Ctrl").keys().len(), 2);
    }

    #[test]
    fn display_round_trips_and_labels_per_platform() {
        for s in ["Fn", "Right Option", "Ctrl+Cmd", "Left Ctrl+Option+Shift", "Fn+Ctrl"] {
            let parsed = b(s);
            assert_eq!(b(&parsed.to_string()), parsed, "{s}");
        }
        assert_eq!(b("Ctrl+Cmd").to_string(), "Ctrl+Cmd");
        assert_eq!(b("cmd+ctrl").label_for(Os::Windows), "Ctrl+Win");
        assert_eq!(b("ralt").label_for(Os::MacOs), "Right Option");
        assert_eq!(b("ralt").label_for(Os::Windows), "Right Alt");
        assert_eq!(HotkeyBinding::default_for(Os::MacOs).to_string(), "Fn");
        assert_eq!(HotkeyBinding::default_for(Os::Windows).label_for(Os::Windows), "Ctrl+Win");
    }

    #[test]
    fn rejects_bad_bindings() {
        assert_eq!("".parse::<HotkeyBinding>(), Err(BindingError::Empty));
        assert_eq!(" + ".parse::<HotkeyBinding>(), Err(BindingError::Empty));
        assert!(matches!("F13".parse::<HotkeyBinding>(), Err(BindingError::NotAModifier(_))));
        assert!(matches!("Ctrl+A".parse::<HotkeyBinding>(), Err(BindingError::NotAModifier(_))));
        assert!(matches!("Ctrl+Left Ctrl".parse::<HotkeyBinding>(), Err(BindingError::Duplicate(_))));
        assert_eq!("Right Fn".parse::<HotkeyBinding>(), Err(BindingError::FnSide));
        assert_eq!("Shift".parse::<HotkeyBinding>(), Err(BindingError::ShiftAlone));
        assert_eq!("Ctrl+Option+Shift+Cmd+Fn".parse::<HotkeyBinding>(), Err(BindingError::TooMany));
        assert_eq!(b("Fn").check_for(Os::Windows), Err(BindingError::FnOnWindows));
        assert_eq!(b("Fn").check_for(Os::MacOs), Ok(()));
        assert_eq!(b("Ctrl+Win").check_for(Os::Windows), Ok(()));
        assert!(b("Cmd").caution().is_some());
        assert!(b("Right Cmd").caution().is_none());
        assert!(b("Fn").caution().is_none());
        assert!(b("Ctrl+Cmd").caution().is_none());
    }

    #[test]
    fn serde_uses_the_text_form() {
        let j = serde_json::to_string(&b("right option")).unwrap();
        assert_eq!(j, "\"Right Option\"");
        let back: HotkeyBinding = serde_json::from_str(&j).unwrap();
        assert_eq!(back, b("Right Option"));
        assert!(serde_json::from_str::<HotkeyBinding>("\"Q\"").is_err());
    }

    #[test]
    fn mac_flags_decode_sides() {
        // Right Option: Alternate + device RAlt.
        let h = HeldMods::from_mac_flags(0x80000 | 0x40);
        assert!(h.r_opt && !h.l_opt);
        // Ctrl (left) + Cmd (right).
        let h = HeldMods::from_mac_flags(0x40000 | 0x1 | 0x100000 | 0x10);
        assert!(h.l_ctrl && h.r_cmd && !h.r_ctrl && !h.l_cmd);
        // Generic bit only counts as left.
        let h = HeldMods::from_mac_flags(0x80000);
        assert!(h.l_opt && !h.r_opt);
        // Fn.
        assert!(HeldMods::from_mac_flags(0x800000).fn_key);
        // Caps Lock (AlphaShift 0x10000) and non-coalesced (0x100) are ignored.
        assert!(HeldMods::from_mac_flags(0x10000 | 0x100).none());
    }

    fn held(f: impl FnOnce(&mut HeldMods)) -> HeldMods {
        let mut h = HeldMods::default();
        f(&mut h);
        h
    }

    #[test]
    fn exact_match_and_sides() {
        let ropt = b("Right Option");
        assert!(ropt.matches(&held(|h| h.r_opt = true)));
        assert!(!ropt.matches(&held(|h| h.l_opt = true)));
        assert!(!ropt.matches(&held(|h| {
            h.r_opt = true;
            h.l_opt = true
        })));
        assert!(!ropt.matches(&held(|h| {
            h.r_opt = true;
            h.l_shift = true
        })));
        let cc = b("Ctrl+Cmd");
        assert!(cc.matches(&held(|h| {
            h.r_ctrl = true;
            h.l_cmd = true
        })));
        assert!(!cc.matches(&held(|h| h.l_ctrl = true)));
        assert!(!cc.matches(&held(|h| {
            h.l_ctrl = true;
            h.l_cmd = true;
            h.fn_key = true
        })));
    }

    #[test]
    fn tracker_hold_and_release() {
        let mut t = ChordTracker::new(b("Ctrl+Cmd"));
        assert_eq!(t.on_mods(held(|h| h.l_ctrl = true)), None);
        assert_eq!(
            t.on_mods(held(|h| {
                h.l_ctrl = true;
                h.l_cmd = true
            })),
            Some(HotkeyEvent::Down)
        );
        assert!(t.is_active());
        assert_eq!(t.on_mods(held(|h| h.l_ctrl = true)), Some(HotkeyEvent::Up));
        assert_eq!(t.on_mods(HeldMods::default()), None);
    }

    #[test]
    fn tracker_cancels_on_key_or_extra_modifier() {
        // Right Option then "e": typing é, not a hold.
        let mut t = ChordTracker::new(b("Right Option"));
        assert_eq!(t.on_mods(held(|h| h.r_opt = true)), Some(HotkeyEvent::Down));
        assert_eq!(t.on_other_key(), Some(HotkeyEvent::Cancel));
        assert_eq!(t.on_mods(HeldMods::default()), None, "no Up after a cancel");

        // Ctrl+Cmd then Shift (Ctrl+Shift+Cmd+4): cancelled, and releasing
        // Shift while Ctrl+Cmd are still down does not start a new hold.
        let mut t = ChordTracker::new(b("Ctrl+Cmd"));
        let cc = held(|h| {
            h.l_ctrl = true;
            h.l_cmd = true
        });
        assert_eq!(t.on_mods(cc), Some(HotkeyEvent::Down));
        let ccs = held(|h| {
            h.l_ctrl = true;
            h.l_cmd = true;
            h.l_shift = true
        });
        assert_eq!(t.on_mods(ccs), Some(HotkeyEvent::Cancel));
        assert_eq!(t.on_mods(cc), None, "blocked until all keys are up");
        assert_eq!(t.on_mods(HeldMods::default()), None);
        assert_eq!(t.on_mods(cc), Some(HotkeyEvent::Down), "works again after release");
        assert_eq!(t.on_mods(HeldMods::default()), Some(HotkeyEvent::Up));
    }

    #[test]
    fn tracker_shortcut_started_before_chord_is_blocked() {
        // Cmd held, a key pressed (Cmd+Tab), then Ctrl added: not a hold.
        let mut t = ChordTracker::new(b("Ctrl+Cmd"));
        assert_eq!(t.on_mods(held(|h| h.l_cmd = true)), None);
        assert_eq!(t.on_other_key(), None);
        assert_eq!(
            t.on_mods(held(|h| {
                h.l_cmd = true;
                h.l_ctrl = true
            })),
            None
        );
        assert_eq!(t.on_mods(HeldMods::default()), None);
    }

    #[test]
    fn tracker_fn_is_unchanged_from_phase_2() {
        let mut t = ChordTracker::new(HotkeyBinding::fn_key());
        assert_eq!(t.on_mods(held(|h| h.fn_key = true)), Some(HotkeyEvent::Down));
        assert_eq!(t.on_mods(HeldMods::default()), Some(HotkeyEvent::Up));
        // Left Option alone never triggers a Right Option binding.
        let mut t = ChordTracker::new(b("Right Option"));
        assert_eq!(t.on_mods(held(|h| h.l_opt = true)), None);
        assert_eq!(t.on_mods(HeldMods::default()), None);
    }
}
