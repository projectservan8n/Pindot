# Windows test checklist

Phase 0 was built and tested on macOS only. On Windows today:

- `deskhand-core` type-checks for `x86_64-pc-windows-msvc`. Command:
  `cargo check -p deskhand-core --all-targets --target x86_64-pc-windows-msvc`
  (run on the Mac, 14 Nov 2026).
- The platform traits have Windows stubs that return
  `PlatformError::Unsupported`. They are capture, input, hotkey and
  accessibility.
- The app's Windows overlay path is written but has never been compiled:
  `overlay.rs`, `#[cfg(not(target_os = "macos"))]`. It is a transparent,
  always-on-top, undecorated webview window per monitor with
  `set_ignore_cursor_events(true)` and `content_protected`. Cross-checking the
  app crate from macOS fails in `tauri-build`, which needs `llvm-rc` to embed
  `icons/icon.ico`. Build it on Windows.

Run these on a real Windows 11 machine, with at least two monitors and one of
them scaled to 150%.

## Build

- [ ] Install Rust (MSVC toolchain), WebView2 (preinstalled on Windows 11) and
      Claude Code; run `claude login`.
- [ ] `cargo build --workspace` and `cargo test --workspace` pass.

## (a) Hotkey: hold Ctrl+Win

- [ ] Implement `Hotkey` with `WH_KEYBOARD_LL`. Report down when both Ctrl and
      Win are held, and up when either is released.
- [ ] Holding and releasing Ctrl+Win must **not** open the Start menu. Mask
      the Win key-up, for example by injecting a dummy key (VK 0xFF) before
      the Win up.
- [ ] The hook keeps working after the PC sleeps and wakes, and while an
      elevated window has focus. If it can't reach that window, report it;
      don't fail silently.

## (b) Overlay

- [ ] `deskhand --demo` shows the cursor on every monitor, including the 150%
      one, and the cursor glides across the monitor edge.
- [ ] Clicks and scrolls pass through the overlay to the window underneath.
- [ ] The overlay never takes focus. The Alt+Tab list and the taskbar don't
      show it.
- [ ] The overlay stays above a full-screen app, such as a video in F11 mode
      or a PowerPoint slideshow.
- [ ] A screenshot (Win+Shift+S and our `screenshot` tool) does **not** show
      the cursor. `content_protected` maps to
      `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
- [ ] The process is per-monitor DPI aware v2: cursor positions match the
      screen on a mixed-DPI setup.

## (c) MCP tools

- [ ] Implement `Capture` (xcap) and `Input` (enigo); add both crates under
      `[target.'cfg(windows)'.dependencies]`.
- [ ] `python3 spikes/mcp-call.py screenshot '{}'` returns a JPEG with a long
      edge of at most 1280 px.
- [ ] From a normal `claude` session with `--mcp-config`, "take a screenshot
      and click X" clicks the right spot on each monitor and at each scale.
- [ ] Clicking into an elevated window fails with a clear error, not silently.

## (d) Brain in ConPTY

- [ ] `spike_brain --prompt 'Reply with exactly the word: pong' --model haiku`
      works under ConPTY (`portable-pty`).
- [ ] The trust dialog is detected and answered only with `--accept-trust`.
- [ ] The child `claude` has `ANTHROPIC_API_KEY` set to empty (check with
      Process Explorer > Environment) and no `CLAUDECODE` variables.
- [ ] The Stop hook arrives and the transcript path under
      `%USERPROFILE%\.claude\projects\` matches. Check how the cwd slug is
      built on Windows: drive letter and backslashes.
- [ ] `/exit` ends the child; no orphaned `claude.exe` or `conhost.exe`.

## Data dir and token

- [ ] The token and configs go in `%APPDATA%\Deskhand\` and are readable by
      the current user only (check the ACL with `icacls`).
