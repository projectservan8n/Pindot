//! Mapping between screenshot pixels (what the model sees) and global
//! logical points (what the OS input APIs and the overlay use).

use serde::Serialize;

use crate::platform::{DisplayInfo, Point};

/// Long edge of every screenshot we send to the model, in pixels.
/// Large enough to read UI text, small enough to keep tokens down.
pub const MAX_LONG_EDGE: u32 = 1280;

/// Geometry of the last screenshot sent to the model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotGeometry {
    pub display: DisplayInfo,
    /// Size of the image the model received, in image pixels.
    pub image_width: u32,
    pub image_height: u32,
}

impl ShotGeometry {
    /// Image pixel (as seen by the model) to global logical point.
    pub fn to_global(&self, sx: f64, sy: f64) -> Point {
        let b = self.display.bounds;
        Point {
            x: b.x + sx * b.width / self.image_width as f64,
            y: b.y + sy * b.height / self.image_height as f64,
        }
    }

    /// Global logical point to image pixel. `None` if the point is not on
    /// this display.
    pub fn from_global(&self, p: Point) -> Option<(f64, f64)> {
        let b = self.display.bounds;
        if !b.contains(p) {
            return None;
        }
        Some((
            (p.x - b.x) * self.image_width as f64 / b.width,
            (p.y - b.y) * self.image_height as f64 / b.height,
        ))
    }

    /// True if the image pixel lies inside the image.
    pub fn in_image(&self, sx: f64, sy: f64) -> bool {
        sx >= 0.0 && sy >= 0.0 && sx < self.image_width as f64 && sy < self.image_height as f64
    }
}

/// Size to downscale a `w`x`h` capture to so the long edge is at most
/// `max_long`, keeping the aspect ratio. Never upscales.
pub fn downscaled_size(w: u32, h: u32, max_long: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_long || long == 0 {
        return (w, h);
    }
    let scale = max_long as f64 / long as f64;
    (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Rect;

    fn display(x: f64, y: f64, w: f64, h: f64, scale: f64) -> DisplayInfo {
        DisplayInfo {
            id: 7,
            name: "t".into(),
            bounds: Rect { x, y, width: w, height: h },
            scale_factor: scale,
            is_primary: x == 0.0 && y == 0.0,
        }
    }

    #[test]
    fn downscale_keeps_aspect_and_never_upscales() {
        assert_eq!(downscaled_size(2880, 1800, 1280), (1280, 800));
        assert_eq!(downscaled_size(1800, 2880, 1280), (800, 1280));
        assert_eq!(downscaled_size(1024, 768, 1280), (1024, 768));
        assert_eq!(downscaled_size(3840, 2160, 1280), (1280, 720));
    }

    #[test]
    fn primary_retina_round_trip() {
        // 1440x900 pt display captured at 2x = 2880x1800 px, sent at 1280x800.
        let g = ShotGeometry { display: display(0.0, 0.0, 1440.0, 900.0, 2.0), image_width: 1280, image_height: 800 };
        let p = g.to_global(640.0, 400.0);
        assert_eq!(p, Point::new(720.0, 450.0));
        let (sx, sy) = g.from_global(p).unwrap();
        assert!((sx - 640.0).abs() < 1e-9 && (sy - 400.0).abs() < 1e-9);
    }

    #[test]
    fn secondary_display_left_of_primary_has_negative_origin() {
        let g = ShotGeometry { display: display(-1920.0, 0.0, 1920.0, 1080.0, 1.0), image_width: 1280, image_height: 720 };
        assert_eq!(g.to_global(0.0, 0.0), Point::new(-1920.0, 0.0));
        assert_eq!(g.to_global(1280.0, 720.0), Point::new(0.0, 1080.0));
        assert!(g.from_global(Point::new(10.0, 10.0)).is_none());
    }

    #[test]
    fn display_above_primary() {
        let g = ShotGeometry { display: display(200.0, -1080.0, 1920.0, 1080.0, 1.0), image_width: 1920, image_height: 1080 };
        assert_eq!(g.to_global(100.0, 100.0), Point::new(300.0, -980.0));
    }
}
