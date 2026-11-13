//! The desktop tools Claude Code sees as `mcp__desktop__*`.
//! Phase 0: `screenshot` and `click` only.

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{ServerState, ToolCallEvent};
use crate::coords::{MAX_LONG_EDGE, ShotGeometry, downscaled_size};
use crate::platform::{MouseButton, PlatformError};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotArgs {
    /// Which display to capture: 0 is the main display, 1 the next, and so
    /// on. Omit for the main display.
    #[serde(default)]
    pub display: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickArgs {
    /// X coordinate in pixels of the most recent screenshot image.
    pub x: f64,
    /// Y coordinate in pixels of the most recent screenshot image.
    pub y: f64,
    /// Mouse button. Default: left.
    #[serde(default)]
    pub button: Option<MouseButton>,
    /// Double-click instead of a single click. Default: false.
    #[serde(default)]
    pub double: Option<bool>,
}

#[derive(Clone)]
pub struct DesktopServer {
    state: Arc<ServerState>,
    #[allow(dead_code)] // read by the #[tool_handler] macro
    tool_router: ToolRouter<Self>,
}

impl DesktopServer {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state, tool_router: Self::tool_router() }
    }

    fn event(&self, tool: &str, detail: String) {
        tracing::info!(tool, "{detail}");
        let _ = self.state.tool_events.send(ToolCallEvent { tool: tool.into(), detail });
    }
}

fn tool_error(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

fn platform_error(e: PlatformError) -> CallToolResult {
    match e {
        PlatformError::PermissionMissing(p) => tool_error(format!(
            "Deskhand is missing the macOS {p} permission. Ask the user to grant it in System Settings > Privacy & Security > {p}, then retry."
        )),
        other => tool_error(other),
    }
}

/// Capture, downscale and JPEG-encode one display. Blocking.
fn shoot(state: &ServerState, index: u32) -> Result<(String, ShotGeometry, usize), CallToolResult> {
    let displays = state.capture.displays().map_err(platform_error)?;
    let n = displays.len();
    let display = displays
        .get(index as usize)
        .cloned()
        .ok_or_else(|| tool_error(format!("There are {n} displays; use display 0..{}", n.saturating_sub(1))))?;
    let frame = state.capture.capture_display(display.id).map_err(platform_error)?;
    let (w, h) = downscaled_size(frame.image.width(), frame.image.height(), MAX_LONG_EDGE);
    let small = image::imageops::resize(&frame.image, w, h, image::imageops::FilterType::Triangle);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut Cursor::new(&mut jpeg), 80)
        .encode_image(&rgb)
        .map_err(tool_error)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
    Ok((b64, ShotGeometry { display: frame.display, image_width: w, image_height: h }, n))
}

#[tool_router]
impl DesktopServer {
    #[tool(
        description = "Take a screenshot of one of the user's displays. Returns a JPEG image. Coordinates you pass to `click` are pixel positions in the most recent screenshot."
    )]
    async fn screenshot(&self, Parameters(args): Parameters<ScreenshotArgs>) -> Result<CallToolResult, ErrorData> {
        let index = args.display.unwrap_or(0);
        let state = self.state.clone();
        let shot = tokio::task::spawn_blocking(move || shoot(&state, index))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let (b64, geom, n) = match shot {
            Ok(v) => v,
            Err(result) => return Ok(result),
        };
        let note = format!(
            "Display {index} of {n} ({}), image {}x{} px. Click coordinates are pixels in this image.",
            geom.display.name, geom.image_width, geom.image_height
        );
        self.event("screenshot", note.clone());
        *self.state.last_shot.lock().unwrap() = Some(geom);
        Ok(CallToolResult::success(vec![ContentBlock::image(b64, "image/jpeg"), ContentBlock::text(note)]))
    }

    #[tool(
        description = "Click at a position in the most recent screenshot (pixels of that image). The user sees an AI cursor glide there first. Take a new screenshot afterwards to check the result."
    )]
    async fn click(&self, Parameters(args): Parameters<ClickArgs>) -> Result<CallToolResult, ErrorData> {
        let Some(geom) = self.state.last_shot.lock().unwrap().clone() else {
            return Ok(tool_error("Take a screenshot first; click coordinates are pixels in the latest screenshot."));
        };
        if !geom.in_image(args.x, args.y) {
            return Ok(tool_error(format!(
                "({}, {}) is outside the last screenshot ({}x{}).",
                args.x, args.y, geom.image_width, geom.image_height
            )));
        }
        let point = geom.to_global(args.x, args.y);
        let button = args.button.unwrap_or(MouseButton::Left);
        let count = if args.double.unwrap_or(false) { 2 } else { 1 };

        // Cursor first, then the click, so the user always sees where it lands.
        let travel = self.state.overlay.move_cursor(point, Some("click".into()));
        tokio::time::sleep(travel).await;
        self.state.overlay.set_acting(true);
        let input = self.state.input.clone();
        let res = tokio::task::spawn_blocking(move || input.click(point, button, count))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        self.state.overlay.set_acting(false);
        if let Err(e) = res {
            return Ok(platform_error(e));
        }
        let msg = format!(
            "Clicked {button:?} x{count} at ({:.0}, {:.0}) in the screenshot = screen point ({:.0}, {:.0}).",
            args.x, args.y, point.x, point.y
        );
        self.event("click", msg.clone());
        Ok(CallToolResult::success(vec![ContentBlock::text(msg)]))
    }
}

#[tool_handler]
impl ServerHandler for DesktopServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("deskhand", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Deskhand gives you the user's screen and mouse. Call screenshot to see a display, then click with pixel coordinates from that screenshot.",
            )
    }
}
