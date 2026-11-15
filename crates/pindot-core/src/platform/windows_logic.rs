//! The parts of the Windows implementation that need no Windows API, so they
//! are tested on every OS: what the microphone privacy switches mean, and
//! what the low-level keyboard hook does with each key.
//!
//! `platform/windows` reads the registry and runs the hook; it only passes
//! the raw values in here.

use super::{ChordTracker, HeldMods, HotkeyBinding, HotkeyEvent, ModKey, PermissionState, Side};

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
