use std::thread;
use std::time::Duration;

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

use super::accessibility::ax_is_trusted;
use crate::platform::{Input, KeyCombo, KeyName, MouseButton, Permission, PlatformError, Point, Result};

/// Value stamped into `kCGEventSourceUserData` on every event we post, so our
/// own Esc tap (and tests) can recognise Deskhand's synthetic input.
pub const INPUT_MARKER: i64 = 0x0DE5_4A4D;

/// Synthetic mouse and keyboard input through `enigo` (CGEvent underneath).
/// macOS input coordinates are global logical points, which is what `Point`
/// holds.
#[derive(Default)]
pub struct MacInput;

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

/// Enigo is not Send on macOS, so make one per action on the calling thread.
fn enigo() -> Result<Enigo> {
    if !ax_is_trusted() {
        return Err(PlatformError::PermissionMissing(Permission::Accessibility));
    }
    let settings = Settings {
        open_prompt_to_get_permissions: false,
        event_source_user_data: Some(INPUT_MARKER),
        ..Settings::default()
    };
    Enigo::new(&settings).map_err(err)
}

fn button(b: MouseButton) -> Button {
    match b {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn move_abs(e: &mut Enigo, at: Point) -> Result<()> {
    e.move_mouse(at.x.round() as i32, at.y.round() as i32, Coordinate::Abs).map_err(err)
}

fn key_of(k: KeyName) -> Key {
    match k {
        KeyName::Char(c) => Key::Unicode(c),
        KeyName::Return => Key::Return,
        KeyName::Tab => Key::Tab,
        KeyName::Space => Key::Space,
        KeyName::Backspace => Key::Backspace,
        KeyName::ForwardDelete => Key::Delete,
        KeyName::Escape => Key::Escape,
        KeyName::Up => Key::UpArrow,
        KeyName::Down => Key::DownArrow,
        KeyName::Left => Key::LeftArrow,
        KeyName::Right => Key::RightArrow,
        KeyName::Home => Key::Home,
        KeyName::End => Key::End,
        KeyName::PageUp => Key::PageUp,
        KeyName::PageDown => Key::PageDown,
        KeyName::F(n) => match n {
            1 => Key::F1,
            2 => Key::F2,
            3 => Key::F3,
            4 => Key::F4,
            5 => Key::F5,
            6 => Key::F6,
            7 => Key::F7,
            8 => Key::F8,
            9 => Key::F9,
            10 => Key::F10,
            11 => Key::F11,
            12 => Key::F12,
            13 => Key::F13,
            14 => Key::F14,
            15 => Key::F15,
            16 => Key::F16,
            17 => Key::F17,
            18 => Key::F18,
            19 => Key::F19,
            _ => Key::F20,
        },
    }
}

impl Input for MacInput {
    fn click(&self, at: Point, b: MouseButton, count: u32) -> Result<()> {
        let mut e = enigo()?;
        move_abs(&mut e, at)?;
        thread::sleep(Duration::from_millis(40));
        for _ in 0..count.clamp(1, 3) {
            e.button(button(b), Direction::Click).map_err(err)?;
        }
        Ok(())
    }

    fn move_to(&self, at: Point) -> Result<()> {
        move_abs(&mut enigo()?, at)
    }

    fn type_text(&self, text: &str) -> Result<()> {
        let mut e = enigo()?;
        // enigo's fast path prefixes a leading newline with U+200B, which
        // would land in the document, so newlines and tabs are real keys.
        let mut buf = String::new();
        let flush = |e: &mut Enigo, buf: &mut String| -> Result<()> {
            if !buf.is_empty() {
                e.text(buf).map_err(err)?;
                buf.clear();
            }
            Ok(())
        };
        for c in text.chars() {
            match c {
                '\n' | '\r' => {
                    flush(&mut e, &mut buf)?;
                    e.key(Key::Return, Direction::Click).map_err(err)?;
                }
                '\t' => {
                    flush(&mut e, &mut buf)?;
                    e.key(Key::Tab, Direction::Click).map_err(err)?;
                }
                c => buf.push(c),
            }
        }
        flush(&mut e, &mut buf)
    }

    fn key(&self, combo: &KeyCombo) -> Result<()> {
        let mut e = enigo()?;
        let m = combo.mods;
        let mods: Vec<Key> = [(m.cmd, Key::Meta), (m.ctrl, Key::Control), (m.alt, Key::Alt), (m.shift, Key::Shift)]
            .into_iter()
            .filter_map(|(on, k)| on.then_some(k))
            .collect();
        for k in &mods {
            e.key(*k, Direction::Press).map_err(err)?;
        }
        let res = e.key(key_of(combo.key), Direction::Click).map_err(err);
        // Always release the modifiers, even if the key failed.
        for k in mods.iter().rev() {
            let _ = e.key(*k, Direction::Release);
        }
        res
    }

    fn scroll(&self, at: Point, dx: i32, dy: i32) -> Result<()> {
        let mut e = enigo()?;
        move_abs(&mut e, at)?;
        thread::sleep(Duration::from_millis(30));
        if dy != 0 {
            e.scroll(dy, Axis::Vertical).map_err(err)?;
        }
        if dx != 0 {
            e.scroll(dx, Axis::Horizontal).map_err(err)?;
        }
        Ok(())
    }

    fn drag(&self, from: Point, to: Point, should_stop: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        let mut e = enigo()?;
        move_abs(&mut e, from)?;
        thread::sleep(Duration::from_millis(60));
        e.button(Button::Left, Direction::Press).map_err(err)?;
        thread::sleep(Duration::from_millis(80));
        // While the button is held, enigo posts LeftMouseDragged events.
        let steps = ((from.distance(to) / 12.0).ceil() as usize).clamp(8, 60);
        let mut result = Ok(());
        for i in 1..=steps {
            if should_stop() {
                result = Err(PlatformError::Other("stopped by the user (Esc)".into()));
                break;
            }
            let t = i as f64 / steps as f64;
            let p = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            if let Err(x) = move_abs(&mut e, p) {
                result = Err(x);
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        thread::sleep(Duration::from_millis(60));
        // Always let go of the button.
        e.button(Button::Left, Direction::Release).map_err(err)?;
        result
    }
}
