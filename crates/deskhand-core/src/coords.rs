//! Mapping between screenshot pixels (what the model sees) and global
//! logical points (what the OS input APIs and the overlay use).
//!
//! Every coordinate Claude passes in is a pixel of the most recent
//! screenshot. A screenshot covers a `region` of one display: the whole
//! display, or one window. The mapping is linear per screenshot, so it
//! works on any monitor layout (negative origins, mixed scale factors).

use serde::Serialize;

use crate::platform::{DisplayInfo, Point, Rect};

/// Long edge of every screenshot we send to the model, in pixels.
/// Large enough to read UI text, small enough to keep tokens down.
pub const MAX_LONG_EDGE: u32 = 1280;

/// Geometry of the last screenshot sent to the model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotGeometry {
    /// The display the screenshot was taken on.
    pub display: DisplayInfo,
    /// Index of that display in `Capture::displays()` (0 = main).
    pub display_index: usize,
    /// The part of the desktop the image shows, in global logical points.
    pub region: Rect,
    /// Size of the image the model received, in image pixels.
    pub image_width: u32,
    pub image_height: u32,
}

impl ShotGeometry {
    /// Geometry for a capture of `region` (clipped to the display), scaled
    /// so its long edge is at most `max_long` pixels.
    pub fn new(display: DisplayInfo, display_index: usize, region: Rect, max_long: u32) -> Self {
        let region = region.intersect(&display.bounds).unwrap_or(display.bounds);
        let pw = (region.width * display.scale_factor).round().max(1.0) as u32;
        let ph = (region.height * display.scale_factor).round().max(1.0) as u32;
        let (image_width, image_height) = downscaled_size(pw, ph, max_long);
        Self { display, display_index, region, image_width, image_height }
    }

    /// Geometry of a full-display screenshot.
    pub fn full_display(display: DisplayInfo, display_index: usize) -> Self {
        let b = display.bounds;
        Self::new(display, display_index, b, MAX_LONG_EDGE)
    }

    /// Image pixel (as seen by the model) to global logical point.
    pub fn to_global(&self, sx: f64, sy: f64) -> Point {
        let r = self.region;
        Point { x: r.x + sx * r.width / self.image_width as f64, y: r.y + sy * r.height / self.image_height as f64 }
    }

    /// Global logical point to image pixel. `None` if the point is outside
    /// the region the image shows.
    pub fn from_global(&self, p: Point) -> Option<(f64, f64)> {
        if !self.region.contains(p) {
            return None;
        }
        Some(self.project(p))
    }

    /// Global point to image pixel, even outside the image (may be negative
    /// or past the edge).
    pub fn project(&self, p: Point) -> (f64, f64) {
        let r = self.region;
        ((p.x - r.x) * self.image_width as f64 / r.width, (p.y - r.y) * self.image_height as f64 / r.height)
    }

    /// A global rectangle in image pixels (rounded), even if partly outside.
    pub fn project_rect(&self, b: Rect) -> (i64, i64, i64, i64) {
        let (x0, y0) = self.project(Point::new(b.x, b.y));
        let (x1, y1) = self.project(Point::new(b.x + b.width, b.y + b.height));
        (x0.round() as i64, y0.round() as i64, (x1 - x0).round() as i64, (y1 - y0).round() as i64)
    }

    /// An image-pixel rectangle to global points.
    pub fn rect_to_global(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        let a = self.to_global(x, y);
        let b = self.to_global(x + w, y + h);
        Rect::new(a.x, a.y, b.x - a.x, b.y - a.y)
    }

    /// True if the image pixel lies inside the image.
    pub fn in_image(&self, sx: f64, sy: f64) -> bool {
        sx >= 0.0 && sy >= 0.0 && sx < self.image_width as f64 && sy < self.image_height as f64
    }

    /// One line describing this coordinate space, for tool output.
    pub fn describe(&self) -> String {
        let whole = self.region == self.display.bounds;
        format!(
            "{}x{} px image of {} on display {}",
            self.image_width,
            self.image_height,
            if whole { "the whole display".to_string() } else { "a window region".to_string() },
            self.display_index
        )
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

/// Index of the display that holds `p` (or the nearest one), 0 if none.
pub fn display_for_point(displays: &[DisplayInfo], p: Point) -> usize {
    if let Some(i) = displays.iter().position(|d| d.bounds.contains(p)) {
        return i;
    }
    displays
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.bounds.center().distance(p).total_cmp(&b.bounds.center().distance(p)))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let g = ShotGeometry::full_display(display(0.0, 0.0, 1440.0, 900.0, 2.0), 0);
        assert_eq!((g.image_width, g.image_height), (1280, 800));
        let p = g.to_global(640.0, 400.0);
        assert_eq!(p, Point::new(720.0, 450.0));
        let (sx, sy) = g.from_global(p).unwrap();
        assert!((sx - 640.0).abs() < 1e-9 && (sy - 400.0).abs() < 1e-9);
    }

    #[test]
    fn secondary_display_left_of_primary_has_negative_origin() {
        let g = ShotGeometry::full_display(display(-1920.0, 0.0, 1920.0, 1080.0, 1.0), 1);
        assert_eq!((g.image_width, g.image_height), (1280, 720));
        assert_eq!(g.to_global(0.0, 0.0), Point::new(-1920.0, 0.0));
        assert_eq!(g.to_global(1280.0, 720.0), Point::new(0.0, 1080.0));
        assert!(g.from_global(Point::new(10.0, 10.0)).is_none());
    }

    #[test]
    fn display_above_primary() {
        let g = ShotGeometry::full_display(display(200.0, -1080.0, 1920.0, 1080.0, 1.0), 1);
        // 1920 wide -> 1280 image: 1.5 points per pixel.
        assert_eq!(g.to_global(100.0, 100.0), Point::new(350.0, -930.0));
    }

    #[test]
    fn window_region_maps_inside_the_window() {
        // A 800x600 pt window at (100,50) on a 2x display: 1600x1200 px,
        // scaled to 1280x960.
        let d = display(0.0, 0.0, 1440.0, 900.0, 2.0);
        let g = ShotGeometry::new(d, 0, Rect::new(100.0, 50.0, 800.0, 600.0), MAX_LONG_EDGE);
        assert_eq!((g.image_width, g.image_height), (1280, 960));
        assert_eq!(g.to_global(0.0, 0.0), Point::new(100.0, 50.0));
        assert_eq!(g.to_global(640.0, 480.0), Point::new(500.0, 350.0));
        assert_eq!(g.project_rect(Rect::new(100.0, 50.0, 80.0, 60.0)), (0, 0, 128, 96));
        assert!(g.from_global(Point::new(50.0, 50.0)).is_none());
    }

    #[test]
    fn window_partly_off_display_is_clipped() {
        let d = display(0.0, 0.0, 1440.0, 900.0, 1.0);
        let g = ShotGeometry::new(d, 0, Rect::new(1240.0, 700.0, 400.0, 400.0), MAX_LONG_EDGE);
        assert_eq!(g.region, Rect::new(1240.0, 700.0, 200.0, 200.0));
        assert_eq!((g.image_width, g.image_height), (200, 200));
    }

    #[test]
    fn mixed_scale_displays_map_independently() {
        let left = display(-1920.0, 0.0, 1920.0, 1080.0, 1.0);
        let main = display(0.0, 0.0, 1512.0, 982.0, 2.0);
        let gl = ShotGeometry::full_display(left.clone(), 1);
        let gm = ShotGeometry::full_display(main.clone(), 0);
        assert_eq!(gl.to_global(640.0, 360.0), Point::new(-960.0, 540.0));
        let p = gm.to_global(gm.image_width as f64 / 2.0, gm.image_height as f64 / 2.0);
        assert!((p.x - 756.0).abs() < 0.5 && (p.y - 491.0).abs() < 0.5);
        assert_eq!(display_for_point(&[main, left], Point::new(-5.0, 10.0)), 1);
    }
}
