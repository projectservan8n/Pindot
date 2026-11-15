//! The four macOS privacy permissions Pindot uses: read-only checks for
//! the setup screen, and the request calls behind its buttons.
//!
//! The checks (`status`) never show a prompt: `AXIsProcessTrusted`,
//! `CGPreflightListenEventAccess`, `CGPreflightScreenCaptureAccess` and
//! `AVCaptureDevice authorizationStatusForMediaType:`. The requests
//! (`request`) may show a system prompt and add Pindot to the list in
//! System Settings; they only run when the user presses a button.

use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};

use super::accessibility::ax_is_trusted;
use crate::platform::{Permission, PermissionState};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> bool;
}

/// Read-only: never prompts.
pub fn status(p: Permission) -> PermissionState {
    let granted = |ok: bool| if ok { PermissionState::Granted } else { PermissionState::NotGranted };
    match p {
        Permission::Accessibility => granted(ax_is_trusted()),
        Permission::InputMonitoring => granted(objc2_core_graphics::CGPreflightListenEventAccess()),
        Permission::ScreenRecording => granted(objc2_core_graphics::CGPreflightScreenCaptureAccess()),
        Permission::Microphone => microphone_status(),
    }
}

fn microphone_status() -> PermissionState {
    // SAFETY: AVMediaTypeAudio is a framework constant; the class method is
    // a plain query (no prompt, no capture session).
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else { return PermissionState::Unknown };
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::Denied => PermissionState::Denied,
        AVAuthorizationStatus::Restricted => PermissionState::Restricted,
        AVAuthorizationStatus::NotDetermined => PermissionState::NotAsked,
        _ => PermissionState::Unknown,
    }
}

/// FOR THE USER'S BUTTON ONLY: may show a system prompt. Returns the state
/// right after the call (prompts answer later; the screen polls `status`).
pub fn request(p: Permission) -> PermissionState {
    match p {
        Permission::Accessibility => {
            let key = CFString::from_static_str("AXTrustedCheckOptionPrompt");
            let opts = CFDictionary::from_slices(&[&*key], &[&*CFBoolean::new(true)]);
            unsafe { AXIsProcessTrustedWithOptions(Some(opts.as_opaque())) };
        }
        Permission::InputMonitoring => {
            objc2_core_graphics::CGRequestListenEventAccess();
        }
        Permission::ScreenRecording => {
            objc2_core_graphics::CGRequestScreenCaptureAccess();
        }
        Permission::Microphone => {
            if let Some(audio) = unsafe { AVMediaTypeAudio } {
                let handler = block2::RcBlock::new(|_granted: objc2::runtime::Bool| {});
                unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
            }
        }
    }
    status(p)
}
