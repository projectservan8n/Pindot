use crate::platform::Accessibility;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// True if this process (really: the responsible app) has the Accessibility
/// permission. Never prompts.
pub fn ax_is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub struct MacAccessibility;

impl Accessibility for MacAccessibility {
    fn is_trusted(&self) -> bool {
        ax_is_trusted()
    }
}
