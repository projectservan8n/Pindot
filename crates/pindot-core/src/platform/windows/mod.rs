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
//! - `uia`: the element tree through UI Automation (`read_window`, `find`,
//!   pressing an element without the pointer, password fields).
//! - `window`: keeping the overlay windows out of Alt+Tab.
//!
//! A click at x,y is always a real click on Windows: `actionable_at` (find
//! the control under a point and press it instead) is not built.
//!
//! The decisions that need no Windows API live in
//! `platform::windows_logic`, where they are tested on every OS.

pub mod apps;
pub mod capture;
pub mod input;
pub mod keyhook;
pub mod permissions;
pub mod uia;
pub mod window;

pub use apps::WinAccessibility;
pub use capture::WinCapture;
pub use input::WinInput;
pub use keyhook::{WinHotkey, WinStopKey};
pub use permissions::{permission_report, pointer_location};
