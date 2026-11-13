use crate::platform::{Capture, CapturedFrame, DisplayInfo, Permission, PlatformError, Rect, Result};

/// Screen capture through `xcap` (CoreGraphics underneath).
pub struct MacCapture;

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

fn info(m: &xcap::Monitor) -> Result<DisplayInfo> {
    Ok(DisplayInfo {
        id: m.id().map_err(err)?,
        name: m.name().map_err(err)?,
        // xcap reports macOS monitor bounds in logical points (CGDisplayBounds).
        bounds: Rect {
            x: m.x().map_err(err)? as f64,
            y: m.y().map_err(err)? as f64,
            width: m.width().map_err(err)? as f64,
            height: m.height().map_err(err)? as f64,
        },
        scale_factor: m.scale_factor().map_err(err)? as f64,
        is_primary: m.is_primary().map_err(err)?,
    })
}

impl Capture for MacCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut out = xcap::Monitor::all().map_err(err)?.iter().map(info).collect::<Result<Vec<_>>>()?;
        out.sort_by_key(|d| !d.is_primary);
        Ok(out)
    }

    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame> {
        if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
            return Err(PlatformError::PermissionMissing(Permission::ScreenRecording));
        }
        let monitors = xcap::Monitor::all().map_err(err)?;
        let m = monitors
            .iter()
            .find(|m| m.id().ok() == Some(display_id))
            .ok_or_else(|| PlatformError::Other(format!("display {display_id} not found")))?;
        let display = info(m)?;
        let image = m.capture_image().map_err(err)?;
        Ok(CapturedFrame { display, image })
    }
}
