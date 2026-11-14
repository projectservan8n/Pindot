# Windows test checklist

Phases 0 and 1 were built and tested on macOS only. On Windows today:

- `deskhand-core` type-checks for `x86_64-pc-windows-msvc`. Command:
  `cargo check -p deskhand-core --all-targets --target x86_64-pc-windows-msvc`
  (run on the Mac, 14 Nov 2026).
- The platform traits have Windows stubs that return
  `PlatformError::Unsupported`: `Capture`, `Input`, `Hotkey`, `StopKey` (the
  global Esc) and `Accessibility` (window list, element tree, element
  actions, frontmost app, launch, secure-field check). The server, the
  approval gate, the session lock, the activity log and the tree text are
  plain Rust and are covered by `cargo test` on any OS.
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
- [ ] `python3 scripts/mcp_call.py init; python3 scripts/mcp_call.py call screenshot '{}'` returns a JPEG with a long
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

## (e) Phase 1: tools, element tree and guardrails

Implement the Windows side of each trait, then repeat the macOS checks in
`docs/phase1/README.md` in Notepad (the Windows stand-in for TextEdit).
Only click and type in windows you opened for the test.

- [ ] `Accessibility` with UI Automation (`uiautomation` or `windows` crate):
      `windows()` from `EnumWindows` front to back, with pid, owner app,
      title, bounds and a layer/topmost hint; `window_tree()` maps UIA
      control types to our roles (Button -> AXButton and so on) and fills
      name, value, bounds, enabled and focused; `press()` uses the Invoke
      pattern, `set_focus()` and `raise_window()` use `SetForegroundWindow`
      rules (it may refuse; report that clearly).
- [ ] `read_window` on Notepad lists the menu bar (File, Edit, ...) and the
      edit area with refs; `find("Save as")` finds the menu item.
- [ ] Secure fields: a password box (UIA `IsPassword`) is reported as
      `securetextfield` and `type_text` refuses to type into it.
- [ ] `StopKey` with `WH_KEYBOARD_LL`: while armed, a physical Esc returns 1
      (swallowed: Notepad must not see it), stops the action in flight and
      shows the "Stopped" banner. Our own `key esc` (tagged with
      `dwExtraInfo` = our marker) passes through.
- [ ] Approval window: `request_access(["Notepad"])` shows our prompt, Deny
      and "Allow for this session" both work by mouse, Esc denies, and a
      120 s timeout denies. Extra warning shows for Terminal/PowerShell,
      Explorer, Settings/Control Panel, password managers.
- [ ] The gate: `click` on a point over a window of a non-approved app is
      refused before any input; screenshots grey out windows of
      non-approved apps.
- [ ] The banner "Claude is controlling input" shows while an action runs
      and is not in our own screenshots (`WDA_EXCLUDEFROMCAPTURE`).
- [ ] Coordinates on a 150% monitor and a 100% monitor: `click(x,y)` from a
      screenshot of each display lands on the right spot.
- [ ] The activity view (`deskhand --activity`) lists calls live, and
      `%APPDATA%\Deskhand\activity.jsonl` is readable by the user only.
- [ ] Done-when on Windows: `deskhand --test-turn "...Notepad..." --model
      haiku --permission-mode default` with `DESKHAND_TEST_AUTO_APPROVE=Notepad`
      completes with at most one screenshot.
