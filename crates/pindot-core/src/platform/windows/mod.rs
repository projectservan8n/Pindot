//! Windows implementations. See `docs/windows-test-checklist.md` for what
//! each must do and how to test it.
//!
//! - `keyhook`: hold-to-talk and the Esc stop key (a low-level keyboard hook).
//! - `permissions`: the microphone privacy switches and the pointer position.
//! - `capture`: screenshots (GDI) and the monitors.
//! - `input`: mouse and keyboard through `SendInput`, stamped with
//!   `keyhook::INPUT_MARKER`.
//! - `apps`: the window list, the apps behind it, opening an app and
//!   bringing a window to the front.
//!
//! Not built yet: the element tree (UI Automation: TreeWalker over
//! ControlView, the Invoke pattern for press, ElementFromPoint for hit
//! tests, IsPassword for secure fields). Until then `read_window` and `find`
//! answer "not supported" and Claude works from screenshots.
//!
//! The decisions that need no Windows API live in
//! `platform::windows_logic`, where they are tested on every OS.

pub mod apps;
pub mod capture;
pub mod input;
pub mod keyhook;
pub mod permissions;

pub use apps::WinAccessibility;
pub use capture::WinCapture;
pub use input::WinInput;
pub use keyhook::{WinHotkey, WinStopKey};
pub use permissions::{permission_report, pointer_location};
