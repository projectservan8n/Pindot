//! One screenshot through ScreenCaptureKit that leaves out every window our
//! own process owns (streaming mode: the AI cursor is visible to OBS and
//! screen shares, but never in Pindot's own screenshots).
//!
//! `SCScreenshotManager` needs macOS 14. The framework itself exists on 13,
//! so the binary still launches there; `available()` looks the class up at
//! run time and nothing here is called when it is missing.

use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AnyThread;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use crate::platform::{PlatformError, Result};

/// How long to wait for ScreenCaptureKit before giving up on one call.
const WAIT: Duration = Duration::from_secs(10);

fn err(m: impl Into<String>) -> PlatformError {
    PlatformError::Other(m.into())
}

/// Is `SCScreenshotManager` here (macOS 14 or newer)?
pub fn available() -> bool {
    AnyClass::get(c"SCScreenshotManager").is_some()
}

/// A value handed from ScreenCaptureKit's callback queue to the caller. The
/// objects are only read by the receiving thread after the callback returned.
struct Handoff<T>(T);
// SAFETY: see above; the sender never touches the value again.
unsafe impl<T> Send for Handoff<T> {}

fn ns_error(e: *mut NSError) -> String {
    // SAFETY: a non-null error pointer from a completion handler is a live NSError.
    match unsafe { e.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "unknown error".into(),
    }
}

fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel::<std::result::Result<Handoff<Retained<SCShareableContent>>, String>>();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, e: *mut NSError| {
        // SAFETY: a non-null content pointer is a live object for the call; we retain it.
        let msg = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(Handoff(c)),
            None => Err(ns_error(e)),
        };
        let _ = tx.send(msg);
    });
    // SAFETY: the handler matches the documented signature and outlives the call (RcBlock).
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            false, true, &handler,
        )
    };
    match rx.recv_timeout(WAIT) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => Err(err(format!("ScreenCaptureKit could not list the screen: {e}"))),
        Err(_) => Err(err("ScreenCaptureKit did not answer in time")),
    }
}

/// Capture one display (CGDirectDisplayID `display_id`, `width` x `height`
/// physical pixels) as RGBA, without any window owned by process `pid` and
/// without the system pointer.
pub fn capture(display_id: u32, width: u32, height: u32, pid: i32) -> Result<image::RgbaImage> {
    if !available() {
        return Err(err("this macOS has no ScreenCaptureKit screenshots (needs macOS 14)"));
    }
    let content = shareable_content()?;
    // SAFETY: plain getters on a live object.
    let (displays, windows) = unsafe { (content.displays(), content.windows()) };
    let display = displays
        .iter()
        // SAFETY: as above.
        .find(|d| unsafe { d.displayID() } == display_id)
        .ok_or_else(|| err(format!("display {display_id} not found")))?;
    let mine: Vec<Retained<SCWindow>> = windows
        .iter()
        // SAFETY: as above.
        .filter(|w| unsafe { w.owningApplication().is_some_and(|a| a.processID() == pid) })
        .collect();
    let excluded = NSArray::from_retained_slice(&mine);

    // SAFETY: standard object construction; every argument is a live object.
    let (filter, config) = unsafe {
        let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &excluded);
        let config = SCStreamConfiguration::new();
        config.setWidth(width as usize);
        config.setHeight(height as usize);
        config.setShowsCursor(false);
        (filter, config)
    };

    let (tx, rx) = mpsc::channel::<std::result::Result<Handoff<CFRetained<CGImage>>, String>>();
    let handler = RcBlock::new(move |image: *mut CGImage, e: *mut NSError| {
        let msg = match NonNull::new(image) {
            // SAFETY: a non-null image from the handler is valid for the call; we retain it.
            Some(p) => Ok(Handoff(unsafe { CFRetained::retain(p) })),
            None => Err(ns_error(e)),
        };
        let _ = tx.send(msg);
    });
    // SAFETY: the handler matches the documented signature and outlives the call.
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(&handler)) };
    let cg = match rx.recv_timeout(WAIT) {
        Ok(Ok(i)) => i.0,
        Ok(Err(e)) => return Err(err(format!("ScreenCaptureKit screenshot failed: {e}"))),
        Err(_) => return Err(err("ScreenCaptureKit did not answer in time")),
    };

    let (w, h) = (CGImage::width(Some(&cg)), CGImage::height(Some(&cg)));
    if CGImage::bits_per_pixel(Some(&cg)) != 32 {
        return Err(err("ScreenCaptureKit gave an unexpected pixel format"));
    }
    let data = CGDataProvider::data(CGImage::data_provider(Some(&cg)).as_deref())
        .ok_or_else(|| err("could not read the screenshot pixels"))?
        .to_vec();
    let bytes_per_row = CGImage::bytes_per_row(Some(&cg));
    // Rows can carry padding bytes; the same unpacking xcap does.
    let mut buffer = Vec::with_capacity(w * h * 4);
    for row in data.chunks_exact(bytes_per_row).take(h) {
        buffer.extend_from_slice(&row[..w * 4]);
    }
    for bgra in buffer.chunks_exact_mut(4) {
        bgra.swap(0, 2);
    }
    image::RgbaImage::from_raw(w as u32, h as u32, buffer).ok_or_else(|| err("the screenshot has the wrong size"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Capture, macos::MacCapture};

    /// Needs Screen Recording permission and macOS 14, so it is opt-in:
    /// `cargo test -p pindot-core sck_matches_xcap -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn sck_matches_xcap() {
        assert!(available());
        let cap = MacCapture;
        let d = cap.displays().unwrap()[0].clone();
        crate::platform::set_streaming_capture(false);
        let t = std::time::Instant::now();
        let a = cap.capture_display(d.id).unwrap().image;
        let xcap_ms = t.elapsed().as_millis();
        crate::platform::set_streaming_capture(true);
        let mut sck_ms = Vec::new();
        let mut b = None;
        for _ in 0..4 {
            let t = std::time::Instant::now();
            b = Some(cap.capture_display(d.id).unwrap().image);
            sck_ms.push(t.elapsed().as_millis());
        }
        let b = b.unwrap();
        println!("time for one screenshot: xcap {xcap_ms} ms, sck {sck_ms:?} ms (first call, then warm)");
        crate::platform::set_streaming_capture(false);
        let a2 = cap.capture_display(d.id).unwrap().image;
        let drift = a.pixels().zip(a2.pixels()).filter(|(p, q)| (0..3).any(|i| (p.0[i] as i32 - q.0[i] as i32).abs() > 8)).count();
        println!("baseline: two xcap frames differ by more than 8 in {drift} pixels");
        println!("xcap {}x{}  sck {}x{}", a.width(), a.height(), b.width(), b.height());
        assert_eq!(a.dimensions(), b.dimensions());
        let (w, h) = a.dimensions();
        let mut worst = 0i32;
        for (fx, fy) in [(0.05, 0.05), (0.5, 0.5), (0.95, 0.05), (0.05, 0.95), (0.95, 0.95), (0.3, 0.6), (0.7, 0.2), (0.5, 0.02)] {
            let (x, y) = ((w as f64 * fx) as u32, (h as f64 * fy) as u32);
            let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            let diff = (0..3).map(|i| (p[i] as i32 - q[i] as i32).abs()).max().unwrap();
            worst = worst.max(diff);
            println!("({x},{y}) xcap {p:?} sck {q:?} diff {diff}");
        }
        let (mut sum, mut big) = (0u64, 0u64);
        for (p, q) in a.pixels().zip(b.pixels()) {
            let d = (0..3).map(|i| (p.0[i] as i32 - q.0[i] as i32).unsigned_abs()).max().unwrap();
            sum += d as u64;
            big += (d > 8) as u64;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, p) in a.enumerate_pixels() {
            let q = b.get_pixel(x, y);
            if (0..3).any(|i| (p.0[i] as i32 - q.0[i] as i32).abs() > 8) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        println!("big differences lie inside x {x0}..{x1}, y {y0}..{y1}");
        a.save("/tmp/cmp_xcap.png").unwrap();
        b.save("/tmp/cmp_sck.png").unwrap();
        println!("whole frame: mean channel diff {:.3}, pixels differing by more than 8: {} of {}", sum as f64 / (w * h) as f64, big, w * h);
        println!("worst channel difference {worst}; alpha xcap {} sck {}", a.get_pixel(0, 0).0[3], b.get_pixel(0, 0).0[3]);
    }
}
