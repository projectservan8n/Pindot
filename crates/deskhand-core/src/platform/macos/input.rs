use std::thread;
use std::time::Duration;

use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};

use super::accessibility::ax_is_trusted;
use crate::platform::{Input, MouseButton, Permission, PlatformError, Point, Result};

/// Value stamped into `kCGEventSourceUserData` on every event we post, so our
/// own hotkey tap (and tests) can recognise Deskhand's synthetic input.
pub const INPUT_MARKER: i64 = 0x0DE5_4A4D;

/// Synthetic mouse input through `enigo` (CGEvent underneath). macOS input
/// coordinates are global logical points, which is what `Point` holds.
#[derive(Default)]
pub struct MacInput;

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

impl Input for MacInput {
    fn click(&self, at: Point, button: MouseButton, count: u32) -> Result<()> {
        if !ax_is_trusted() {
            return Err(PlatformError::PermissionMissing(Permission::Accessibility));
        }
        let settings = Settings {
            open_prompt_to_get_permissions: false,
            event_source_user_data: Some(INPUT_MARKER),
            ..Settings::default()
        };
        // Enigo is not Send on macOS, so make one per action on this thread.
        let mut enigo = Enigo::new(&settings).map_err(err)?;
        enigo.move_mouse(at.x.round() as i32, at.y.round() as i32, Coordinate::Abs).map_err(err)?;
        thread::sleep(Duration::from_millis(40));
        let b = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
        };
        for _ in 0..count.clamp(1, 3) {
            enigo.button(b, Direction::Click).map_err(err)?;
        }
        Ok(())
    }
}
