# Windows test checklist

Phases 0 to 3 were built and tested on macOS only. **Nothing has been run on
Windows**: there is no Windows machine here. On Windows today:

- `deskhand-core` type-checks for `x86_64-pc-windows-msvc`. Command:
  `cargo check -p deskhand-core --all-targets --target x86_64-pc-windows-msvc`
  (run on the Mac, 14 Nov 2026; re-run it after Phase 3 changes before
  relying on it).
- The platform traits have Windows stubs that return
  `PlatformError::Unsupported`: `Capture`, `Input`, `Hotkey`, `StopKey` (the
  global Esc) and `Accessibility` (window list, element tree, element
  actions, frontmost app, launch, secure-field check). The server, the
  approval gate, the session lock, the activity log, the tree text, the
  settings file, the hotkey binding parser (including `Ctrl+Win` and the
  "Fn never reaches Windows" check), the model downloader and the voice
  state machine are plain Rust and are covered by `cargo test` on any OS.
- The permission report on Windows lists only the microphone, as
  "unknown": the privacy switch is not read yet. Its button opens
  `ms-settings:privacy-microphone`.
- The token and config files are written with Unix mode 600 on macOS. On
  Windows no ACL is set yet: they rely on `%APPDATA%` being per-user.
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
- [ ] Done-when on Windows: build with `cargo build -p deskhand --features
      test-switches`, then `deskhand --test-turn "...Notepad..." --model
      haiku --permission-mode default` with `DESKHAND_TEST_AUTO_APPROVE=Notepad`
      completes with at most one screenshot.

## (f) Phase 2: the voice loop

- [ ] `voice_check` (`cargo run --release -p deskhand-core --bin
      voice_check`) lists the audio devices (cpal on WASAPI), downloads
      and verifies both models into `%APPDATA%\Deskhand\models\`, and
      runs STT on `docs/phase2/test-utterance.wav` and TTS with a play at
      low volume.
- [ ] ONNX Runtime: `ort` downloads its Windows binaries at build time;
      check the app starts on a clean machine without a separate install
      (or that the needed DLL is bundled).
- [ ] Test build (`--features test-switches`): `deskhand --test-voice
      docs/phase2/test-utterance.wav --model haiku` with
      `DESKHAND_TEST_AUTO_APPROVE=Notepad` prints `RESULT PASS`.
- [ ] Real use: hold Ctrl+Win, say "Open Notepad and type hello", let go.
      Expect Listening, Thinking, Working, the approval window, the text in
      Notepad and a spoken reply. The Start menu must not open.
- [ ] Barge-in: hold Ctrl+Win while it speaks: speech stops and it listens.
- [ ] Esc during a turn: the action stops, speech stops, the turn is
      interrupted (Esc is written to the ConPTY), and the next request
      works in the same session.
- [ ] The persistent `claude.exe` under ConPTY survives 10 minutes idle and
      the next request starts a fresh session; quitting Deskhand leaves no
      `claude.exe`, `node.exe` or `conhost.exe` behind.

## (g) Phase 3: app, settings, setup, release

- [ ] Tray icon in the notification area with Setup, Settings, Activity,
      the hold-to-talk switch and Quit. Quit leaves no process.
- [ ] First start opens Setup without stealing focus from the app in front.
- [ ] Setup: the `claude` check finds Claude Code and reads `claude auth
      status --json`; signed out shows "run `claude login`". Known gap:
      `find_claude` looks only for `claude.exe` on PATH (the native
      installer); an npm install provides `claude.cmd`, which is not found
      yet, and a `.cmd` cannot be spawned in a PTY directly (use `cmd /c`).
- [ ] Setup > Permissions shows the microphone row; "Open Settings" opens
      the microphone privacy page. Wire the real privacy-switch check (it
      is "unknown" today).
- [ ] Setup > Voice models: licenses shown before download, progress,
      cancel and resume (HTTP 206), and "Check files" repairs a file with
      one changed byte. The Moonshine fallback (blob.handy.computer) works
      when huggingface.co is blocked (hosts file).
- [ ] Settings: presets list `Ctrl+Win` (default), `Right Alt`,
      `Right Ctrl`, ...; Record captures a held chord inside the window;
      typing `Fn` is refused with the Windows message; Save writes
      `settings.json` and the new chord works without a restart.
- [ ] Settings > Regenerate token: the old token gets 401 and the new one
      200; check the file ACL with `icacls "%APPDATA%\Deskhand\mcp-token"`
      (only the current user should have access; today nothing sets it).
- [ ] Settings > Port: a port in use shows the error banner; a new port
      applies after a restart.
- [ ] "Connect to Claude Code" runs `claude mcp add --transport http
      --scope user desktop http://127.0.0.1:<port>/mcp --header
      "Authorization: Bearer <token>"` through `claude.cmd` correctly
      (quoting of the header on Windows), then shows "connected";
      Disconnect removes it. Only on a test account: it changes the user's
      Claude Code config.
- [ ] Activity: filter by session and tool; Clear asks first and empties
      `activity.jsonl`.
- [ ] Release: `npx tauri build --bundles nsis,msi` (see
      `docs/release.md`); install on a clean Windows 11, launch, check
      Setup, `curl -H "Authorization: Bearer <token>"
      http://127.0.0.1:47823/health` answers `ok`, and uninstall leaves
      only `%APPDATA%\Deskhand\`.
