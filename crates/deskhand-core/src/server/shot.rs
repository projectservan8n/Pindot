//! Screenshots for the model: one display or one window, with every window
//! of an app that is not approved for the session painted grey, downscaled
//! (long edge <= 1280) and JPEG-encoded. Deskhand's own windows never
//! appear: they opt out of capture at the OS level and are skipped here.

use std::io::Cursor;

use image::{GenericImageView, Rgba, RgbaImage, imageops};

use crate::coords::{MAX_LONG_EDGE, ShotGeometry, display_for_point, downscaled_size};
use crate::platform::{Capture, CapturedFrame, DisplayInfo, Rect, WindowInfo};

/// Colour of masked (not approved) windows and the bare desktop.
pub const MASK: Rgba<u8> = Rgba([92, 92, 96, 255]);
pub const JPEG_QUALITY: u8 = 80;

pub enum Target {
    /// Display index (0 = main).
    Display(usize),
    Window(WindowInfo),
}

pub struct Shot {
    pub jpeg: Vec<u8>,
    pub geometry: ShotGeometry,
    /// Apps whose windows were painted over.
    pub masked: Vec<String>,
    pub display_count: usize,
}

/// A rectangle in global points to a pixel rectangle of `display`'s frame,
/// clipped to the frame. `None` if nothing is left.
fn px_rect(display: &DisplayInfo, r: &Rect, img_w: u32, img_h: u32) -> Option<(u32, u32, u32, u32)> {
    let r = r.intersect(&display.bounds)?;
    let s = display.scale_factor;
    let x0 = (((r.x - display.bounds.x) * s).floor().max(0.0) as u32).min(img_w);
    let y0 = (((r.y - display.bounds.y) * s).floor().max(0.0) as u32).min(img_h);
    let x1 = (((r.x + r.width - display.bounds.x) * s).ceil().max(0.0) as u32).min(img_w);
    let y1 = (((r.y + r.height - display.bounds.y) * s).ceil().max(0.0) as u32).min(img_h);
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

/// Paint the frame back to front: windows `allowed` shows keep their
/// pixels, every other window (and the bare desktop) turns grey. Returns
/// the masked image and the names of the masked apps. `windows` is front to
/// back, as the window server lists them.
pub fn mask_frame(
    frame: &CapturedFrame,
    windows: &[WindowInfo],
    own_pid: i32,
    allowed: &dyn Fn(&WindowInfo) -> bool,
) -> (RgbaImage, Vec<String>) {
    let (w, h) = frame.image.dimensions();
    let mut out = RgbaImage::from_pixel(w, h, MASK);
    let mut masked: Vec<String> = vec![];
    for win in windows.iter().rev() {
        if win.pid == own_pid || win.alpha < 0.05 {
            continue;
        }
        let Some((x, y, rw, rh)) = px_rect(&frame.display, &win.bounds, w, h) else { continue };
        let width = w as usize;
        let rows = (y as usize)..(y + rh) as usize;
        let (c0, c1) = (x as usize * 4, (x + rw) as usize * 4);
        if allowed(win) {
            let src: &[u8] = frame.image.as_raw();
            let dst: &mut [u8] = &mut out;
            for row in rows {
                let base = row * width * 4;
                dst[base + c0..base + c1].copy_from_slice(&src[base + c0..base + c1]);
            }
        } else {
            let dst: &mut [u8] = &mut out;
            for row in rows {
                let base = row * width * 4;
                for px in dst[base + c0..base + c1].chunks_exact_mut(4) {
                    px.copy_from_slice(&MASK.0);
                }
            }
            if !win.app.is_empty() && !masked.contains(&win.app) {
                masked.push(win.app.clone());
            }
        }
    }
    masked.sort();
    (out, masked)
}

fn crop(img: &RgbaImage, display: &DisplayInfo, region: &Rect) -> RgbaImage {
    let (w, h) = img.dimensions();
    match px_rect(display, region, w, h) {
        Some((x, y, rw, rh)) => img.view(x, y, rw, rh).to_image(),
        None => img.clone(),
    }
}

pub fn encode_jpeg(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut Cursor::new(&mut jpeg), JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(jpeg)
}

/// Take a masked screenshot of a display or a window.
pub fn take(
    capture: &dyn Capture,
    windows: &[WindowInfo],
    target: &Target,
    own_pid: i32,
    allowed: &dyn Fn(&WindowInfo) -> bool,
) -> Result<Shot, String> {
    let displays = capture.displays().map_err(|e| e.to_string())?;
    let n = displays.len();
    let (idx, region) = match target {
        Target::Display(i) => {
            let d = displays
                .get(*i)
                .ok_or_else(|| format!("There are {n} displays; use display 0..{}.", n.saturating_sub(1)))?;
            (*i, d.bounds)
        }
        Target::Window(w) => (display_for_point(&displays, w.bounds.center()), w.bounds),
    };
    let display = displays.get(idx).cloned().ok_or("no display")?;
    let geometry = ShotGeometry::new(display.clone(), idx, region, MAX_LONG_EDGE);
    let frame = capture.capture_display(display.id).map_err(|e| e.to_string())?;
    let (masked_img, masked) = mask_frame(&frame, windows, own_pid, allowed);
    let cropped = crop(&masked_img, &frame.display, &geometry.region);
    let small = imageops::resize(&cropped, geometry.image_width, geometry.image_height, imageops::FilterType::Triangle);
    // Only report apps that are actually inside the picture.
    let masked = masked
        .into_iter()
        .filter(|app| {
            windows.iter().any(|w| &w.app == app && w.bounds.intersect(&geometry.region).is_some())
        })
        .collect();
    Ok(Shot { jpeg: encode_jpeg(&small)?, geometry, masked, display_count: n })
}

/// A closer look at part of the latest screenshot. `x, y, w, h` are pixels
/// of that screenshot; the result is at most `MAX_LONG_EDGE` on its long
/// side and never upscaled past the display's real pixels.
pub fn zoom(
    capture: &dyn Capture,
    windows: &[WindowInfo],
    geom: &ShotGeometry,
    (x, y, w, h): (f64, f64, f64, f64),
    own_pid: i32,
    allowed: &dyn Fn(&WindowInfo) -> bool,
) -> Result<(Vec<u8>, u32, u32), String> {
    if w <= 0.0 || h <= 0.0 {
        return Err("zoom needs a region with a positive width and height.".into());
    }
    let region = geom
        .rect_to_global(x, y, w, h)
        .intersect(&geom.region)
        .ok_or("That region is outside the latest screenshot.")?;
    let frame = capture.capture_display(geom.display.id).map_err(|e| e.to_string())?;
    let (masked_img, _) = mask_frame(&frame, windows, own_pid, allowed);
    let cropped = crop(&masked_img, &frame.display, &region);
    let (cw, ch) = cropped.dimensions();
    let (tw, th) = downscaled_size(cw, ch, MAX_LONG_EDGE);
    let out = if (tw, th) == (cw, ch) { cropped } else { imageops::resize(&cropped, tw, th, imageops::FilterType::Triangle) };
    Ok((encode_jpeg(&out)?, tw, th))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockCapture;

    fn win(id: u64, pid: i32, app: &str, b: Rect) -> WindowInfo {
        WindowInfo { id, pid, app: app.into(), title: String::new(), bounds: b, layer: 0, alpha: 1.0 }
    }

    #[test]
    fn unapproved_windows_and_desktop_are_masked() {
        let cap = MockCapture::default();
        let frame = cap.capture_display(1).unwrap();
        let original = *frame.image.get_pixel(0, 0);
        let windows = vec![
            win(2, 20, "Chrome", Rect::new(200.0, 100.0, 400.0, 300.0)), // front
            win(1, 10, "TextEdit", Rect::new(0.0, 0.0, 400.0, 300.0)),
            win(3, 99, "Deskhand", Rect::new(0.0, 0.0, 1440.0, 900.0)), // own overlay
        ];
        let allowed = |w: &WindowInfo| w.pid == 10;
        let (img, masked) = mask_frame(&frame, &windows, 99, &allowed);
        // 2x display: point (50,50) = pixel (100,100).
        assert_eq!(*img.get_pixel(100, 100), original, "approved window visible");
        assert_eq!(*img.get_pixel(600, 400), MASK, "Chrome over TextEdit is masked");
        assert_eq!(*img.get_pixel(2000, 1600), MASK, "desktop is masked");
        assert_eq!(masked, vec!["Chrome".to_string()]);
    }

    #[test]
    fn window_shot_crops_to_the_window() {
        let cap = MockCapture::default();
        let w = win(1, 10, "TextEdit", Rect::new(100.0, 50.0, 800.0, 600.0));
        let shot = take(&cap, std::slice::from_ref(&w), &Target::Window(w.clone()), 99, &|_| true).unwrap();
        assert_eq!(shot.geometry.region, w.bounds);
        assert_eq!((shot.geometry.image_width, shot.geometry.image_height), (1280, 960));
        let img = image::load_from_memory(&shot.jpeg).unwrap();
        assert_eq!((img.width(), img.height()), (1280, 960));
        assert!(shot.masked.is_empty());
    }

    #[test]
    fn zoom_returns_native_pixels_of_a_small_region() {
        let cap = MockCapture::default();
        let g = ShotGeometry::full_display(cap.displays().unwrap()[0].clone(), 0);
        // 128x80 image px = 144x90 pt = 288x180 physical px.
        let (_, w, h) = zoom(&cap, &[], &g, (0.0, 0.0, 128.0, 80.0), 99, &|_| true).unwrap();
        assert_eq!((w, h), (288, 180));
        assert!(zoom(&cap, &[], &g, (0.0, 0.0, 0.0, 10.0), 99, &|_| true).is_err());
    }
}
