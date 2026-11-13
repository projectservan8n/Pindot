//! Mocks for every platform trait, for tests and headless runs.

use std::sync::Mutex;
use std::time::Duration;

use super::*;

/// A fake single display with a solid-colour image.
pub struct MockCapture {
    pub displays: Vec<DisplayInfo>,
}

impl Default for MockCapture {
    fn default() -> Self {
        Self {
            displays: vec![DisplayInfo {
                id: 1,
                name: "Mock Retina".into(),
                bounds: Rect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 },
                scale_factor: 2.0,
                is_primary: true,
            }],
        }
    }
}

impl Capture for MockCapture {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays.clone())
    }
    fn capture_display(&self, display_id: u32) -> Result<CapturedFrame> {
        let d = self
            .displays
            .iter()
            .find(|d| d.id == display_id)
            .cloned()
            .ok_or_else(|| PlatformError::Other(format!("no display {display_id}")))?;
        let w = (d.bounds.width * d.scale_factor) as u32;
        let h = (d.bounds.height * d.scale_factor) as u32;
        Ok(CapturedFrame { display: d, image: image::RgbaImage::from_pixel(w, h, image::Rgba([40, 80, 120, 255])) })
    }
}

/// Records every click instead of performing it.
#[derive(Default)]
pub struct MockInput {
    pub clicks: Mutex<Vec<(Point, MouseButton, u32)>>,
}

impl Input for MockInput {
    fn click(&self, at: Point, button: MouseButton, count: u32) -> Result<()> {
        self.clicks.lock().unwrap().push((at, button, count));
        Ok(())
    }
}

/// Emits a scripted sequence of hotkey events.
pub struct MockHotkey {
    pub script: Vec<HotkeyEvent>,
}

impl Hotkey for MockHotkey {
    fn start(&self, _binding: HotkeyBinding, events: std::sync::mpsc::Sender<HotkeyEvent>) -> Result<ListenerHandle> {
        for e in &self.script {
            let _ = events.send(*e);
        }
        Ok(ListenerHandle::new(|| {}))
    }
}

pub struct MockAccessibility {
    pub trusted: bool,
}

impl Accessibility for MockAccessibility {
    fn is_trusted(&self) -> bool {
        self.trusted
    }
}

/// Records cursor moves.
#[derive(Default)]
pub struct MockOverlay {
    pub moves: Mutex<Vec<Point>>,
}

impl Overlay for MockOverlay {
    fn move_cursor(&self, to: Point, _label: Option<String>) -> Duration {
        self.moves.lock().unwrap().push(to);
        Duration::from_millis(1)
    }
    fn set_acting(&self, _acting: bool) {}
}
