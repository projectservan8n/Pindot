//! macOS implementations of the platform traits.

mod accessibility;
mod capture;
pub mod sck;
pub mod hotkey;
mod input;
pub mod permissions;
pub mod stopkey;
pub mod window;

pub use accessibility::{MacAccessibility, ax_is_trusted};
pub use capture::MacCapture;
pub use hotkey::MacHotkey;
pub use input::{INPUT_MARKER, MacInput, pointer_location};
pub use stopkey::MacStopKey;

use super::{Permission, PermissionStatus};

/// Current state of every permission Pindot uses, for the calling process.
/// macOS grants these per app (the .app bundle, or the terminal app a CLI
/// binary was launched from), never per helper binary. Read-only: never
/// prompts.
pub fn permission_report() -> Vec<PermissionStatus> {
    Permission::ALL.iter().map(|&p| PermissionStatus::new(p, permissions::status(p))).collect()
}
