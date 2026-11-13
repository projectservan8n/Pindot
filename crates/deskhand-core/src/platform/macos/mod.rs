//! macOS implementations of the platform traits.

mod accessibility;
mod capture;
pub mod hotkey;
mod input;

pub use accessibility::{MacAccessibility, ax_is_trusted};
pub use capture::MacCapture;
pub use hotkey::MacHotkey;
pub use input::MacInput;

use super::{Permission, PermissionStatus};

/// Current state of every permission Deskhand uses, for the calling process.
/// macOS grants these per app (the .app bundle, or the terminal app a CLI
/// binary was launched from), never per helper binary.
pub fn permission_report() -> Vec<PermissionStatus> {
    vec![
        PermissionStatus { permission: Permission::Accessibility, granted: ax_is_trusted() },
        PermissionStatus {
            permission: Permission::InputMonitoring,
            granted: objc2_core_graphics::CGPreflightListenEventAccess(),
        },
        PermissionStatus {
            permission: Permission::ScreenRecording,
            granted: objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        },
    ]
}
