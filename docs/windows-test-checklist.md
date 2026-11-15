# Windows test checklist

Phases 0 to 3 were built and tested on macOS. Windows was first built and run
on 15 Nov 2026, on one Windows 11 PC (build 26300, two 1920x1080 monitors at
100%, VS 2019 Build Tools, claude 2.1.323 then 2.1.325). A ticked box below
was seen on that PC; a note after a box says how far an unticked one got.

On Windows today:

- **Works**: the build (see "Build"), `cargo test --workspace` (168 + 24 + 3
  tests), the app with Setup, Settings and Activity, the MCP server and its
  guards, the overlay on both monitors, the microphone check, hold-to-talk
  (`Hotkey`, self-healing), the Esc stop key (`StopKey`), speech to text,
  text to speech, the brain in ConPTY, the whole voice loop with a WAV in
  place of the microphone (`--test-voice`, RESULT PASS) and with the real
  microphone and keys, screenshots (`Capture`), mouse and keyboard
  (`Input`), and the window list, apps, approval gate, opening apps and
  bringing a window forward (`Accessibility`). A real turn typed and
  clicked in a test window (`--test-turn`, TURN COMPLETED).
- **Also works, on a test window only**: the element tree through UI
  Automation (`read_window`, `find`, click by ref, press without the
  pointer) and the password-field check (`IsPassword`). Not yet tried on a
  browser, Office or a store app.
- The permission report lists only the microphone, read from the three
  privacy switches in the registry. Its button opens
  `ms-settings:privacy-microphone`. There is no "Ask" button: Windows has
  no prompt for a desktop app.
- The token and config files are written with Unix mode 600 on macOS. On
  Windows no ACL is set yet: they rely on `%APPDATA%` being per-user.
- `scripts/check-windows.sh` still type-checks both crates from the Mac. It
  only proves the Rust compiles.

Still needed: a second PC or a monitor scaled to 150%, a physical-keyboard
pass, and the items left unticked.

## Build

- [x] Install Rust (MSVC toolchain), WebView2 (preinstalled on Windows 11) and
      Claude Code; run `claude login`.
- [x] `cargo build --workspace` and `cargo test --workspace` pass.
      With Visual Studio 2022 Build Tools (a recent MSVC) the plain commands
      should link; with only VS 2019 Build Tools they fail in `libort_sys`
      (unresolved `__std_find_trivial_*` and friends). What was used here:
      unpack Microsoft's `onnxruntime-win-x64-1.28.0.zip` (the version
      `ort-sys` pins) and set `ORT_LIB_LOCATION=<that folder>\lib` and
      `ORT_PREFER_DYNAMIC_LINK=1` before `cargo`. `onnxruntime.dll` is then
      copied next to the exe.
- [x] `cd app && npm ci && npm run check && npm test && npm run build`.

## (a) Hotkey: hold Ctrl+Win

- [x] Implement `Hotkey` with `WH_KEYBOARD_LL`. Report down when both Ctrl and
      Win are held, and up when either is released.
      (`spike_hotkey listen` printed Down and, 311 ms later, Up for a
      synthetic hold.)
- [x] Holding and releasing Ctrl+Win must **not** open the Start menu. An
      unassigned key (VK 0xE8) is pressed once when the hold starts.
      (The foreground window did not change after the synthetic hold. A
      physical keyboard is still to check.)
- [ ] The hook keeps working after the PC sleeps and wakes, and while an
      elevated window has focus. If it can't reach that window, report it;
      don't fail silently. (Not checked. A hook in a normal process does
      not see keys typed into an elevated window, and nothing reports it.)

## (b) Overlay

- [ ] `pindot --demo` shows the cursor on every monitor, including the 150%
      one, and the cursor glides across the monitor edge.
      (Every action played on both monitors at 100%, seen through each
      overlay page over CDP. No 150% monitor here.)
- [ ] Clicks and scrolls pass through the overlay to the window underneath.
      (The windows have `WS_EX_TRANSPARENT | WS_EX_LAYERED`, and a screen
      copy of the second monitor with the overlay up matched the one before
      in over 92% of its pixels. Not clicked by hand.)
- [ ] The overlay never takes focus. The Alt+Tab list and the taskbar don't
      show it. (Focus stayed with the app in front. The overlay windows are
      titled "Tauri App" and have `WS_EX_APPWINDOW`, so Alt+Tab may list
      them: look.)
- [ ] The overlay stays above a full-screen app, such as a video in F11 mode
      or a PowerPoint slideshow.
- [x] A screenshot (Win+Shift+S and our `screenshot` tool) does **not** show
      the cursor. `content_protected` maps to
      `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
      (`GetWindowDisplayAffinity` reports it on both overlays and the main
      window, and a screen copy with the Setup window up differed from one
      without it by 1.2%, against 23.9% with `--capturable`. Our own
      `screenshot` tool does not exist on Windows yet.)
- [ ] The process is per-monitor DPI aware v2: cursor positions match the
      screen on a mixed-DPI setup.

## (c) MCP tools

- [x] Implement `Capture` and `Input`. Done with the raw Win32 API
      (`windows-sys`): GDI `BitBlt` and `SendInput`, no xcap or enigo.
- [x] `python3 scripts/mcp_call.py init; python3 scripts/mcp_call.py call screenshot '{}'` returns a JPEG with a long
      edge of at most 1280 px. (1280x720 for a 1920x1080 display. Set
      `PINDOT_DATA_DIR`: the script's default folder is the Mac one. With no
      app approved the image is solid grey.)
- [x] By hand, against a test window (`spike_mcp` built with
      `--features test-switches` and `PINDOT_TEST_AUTO_APPROVE=<its name>`):
      click, double-click, `type_text` (line break, accents, CJK, emoji),
      `key` (`cmd+a` pressed as Ctrl+A, `end`, `return`, `shift+left` x3,
      `backspace`, `ctrl+x`), scroll, drag (selected text), zoom, point,
      `focus_window`, `open_app`. The pointer was put back after each.
- [ ] From a normal `claude` session with `--mcp-config`, "take a screenshot
      and click X" clicks the right spot on each monitor and at each scale.
      (A real turn clicked and typed correctly on the primary monitor at
      100%: `pindot --test-turn ... --cwd %APPDATA%\Pindot\session`, haiku,
      14.6 s, two screenshots. The second monitor and other scales are not
      tried.)
- [ ] Clicking into an elevated window fails with a clear error, not silently.
      (Not built: Windows drops the input and `SendInput` reports success.)

## (d) Brain in ConPTY

- [x] `spike_brain --prompt 'Reply with exactly the word: pong' --model haiku`
      works under ConPTY (`portable-pty`). ("pong" 2.5 s after the prompt,
      RESULT PASS. It needed the pty reader to answer ConPTY's opening
      cursor query, `ESC[6n`: without the answer nothing `claude` prints
      comes through.)
- [x] The trust dialog is detected and answered only with `--accept-trust`.
      (Refused without the flag in `.spike-cwd`; answered with it in
      `%APPDATA%\Pindot\session`.)
- [ ] The child `claude` has `ANTHROPIC_API_KEY` set to empty (check with
      Process Explorer > Environment) and no `CLAUDECODE` variables.
      (`spike_brain` cannot read another process's environment on Windows.)
- [x] The Stop hook arrives and the transcript path under
      `%USERPROFILE%\.claude\projects\` matches. The slug is the cwd with
      every character that is not a letter or digit turned into `-`:
      `C--Users-<name>-AppData-Roaming-Pindot-session`.
- [x] Ctrl+C ends the child (1.3 s); no `claude.exe` left. `/exit` does not
      work: with `--disable-slash-commands` it is an unknown command.
      (`conhost.exe` was not counted.)

## Data dir and token

- [ ] The token and configs go in `%APPDATA%\Pindot\` and are readable by
      the current user only (check the ACL with `icacls`).

## (e) Phase 1: tools, element tree and guardrails

Implement the Windows side of each trait, then repeat the macOS checks in
`docs/phase1/README.md` in Notepad (the Windows stand-in for TextEdit).
Only click and type in windows you opened for the test.

- [x] `Accessibility`, the part without UI Automation: `windows()` from
      `EnumWindows` front to back, with pid, owner app, title and bounds;
      running apps, the frontmost app, resolving and opening an app from the
      Start menu, and `raise_window()` (`AttachThreadInput`, then an Alt
      tap).
- [x] `Accessibility` with UI Automation (the `windows` crate):
      `window_tree()` maps UIA control types to our roles (Button ->
      AXButton and so on) and fills name, value, bounds, enabled and
      focused; `press()` uses the Invoke pattern (then Toggle, Select,
      Expand/Collapse); `focused_is_secure()` uses `IsPassword`.
      (On the test window: `read_window` in 0.11 s, `find("Press me")`, a
      click by ref pressed the button without moving the pointer, the typed
      value showed when the window was read again. A real turn with haiku
      needed no screenshot: 18.1 s.)
- [ ] `read_window` on Notepad lists the menu bar (File, Edit, ...) and the
      edit area with refs; `find("Save as")` finds the menu item.
      (Not tried on Notepad or any real app. Items of a closed menu are not
      in the tree on Windows.)
- [x] Secure fields: a password box (UIA `IsPassword`) is reported as
      `securetextfield` and `type_text` refuses to type into it.
      (A classic password box. Still to try: a password field on a web
      page, in Chrome and in Edge.)
- [ ] `StopKey` with `WH_KEYBOARD_LL`: while armed, a physical Esc returns 1
      (swallowed: Notepad must not see it), stops the action in flight and
      shows the "Stopped" banner. Our own `key esc` (tagged with
      `dwExtraInfo` = our marker) passes through.
- [ ] Approval window: `request_access(["Notepad"])` shows our prompt, Deny
      and "Allow for this session" both work by mouse, Esc denies, and a
      120 s timeout denies. Extra warning shows for Terminal/PowerShell,
      Explorer, Settings/Control Panel, password managers.
- [x] The gate: `click` on a point over a window of a non-approved app is
      refused before any input; screenshots grey out windows of
      non-approved apps. (A click on Chrome and one on the taskbar were
      refused; the screenshot showed only the approved test window.
      `type_text` into a classic password box was refused.)
- [ ] The banner "Claude is controlling input" shows while an action runs
      and is not in our own screenshots (`WDA_EXCLUDEFROMCAPTURE`).
- [ ] Coordinates on a 150% monitor and a 100% monitor: `click(x,y)` from a
      screenshot of each display lands on the right spot.
- [ ] The activity view (`pindot --activity`) lists calls live, and
      `%APPDATA%\Pindot\activity.jsonl` is readable by the user only.
- [ ] Done-when on Windows: build with `cargo build -p pindot --features
      test-switches`, then `pindot --test-turn "...Notepad..." --model
      haiku --permission-mode default` with `PINDOT_TEST_AUTO_APPROVE=Notepad`
      completes with at most one screenshot.

## (f) Phase 2: the voice loop

- [x] `voice_check` (`cargo run --release -p pindot-core --bin
      voice_check`) lists the audio devices (cpal on WASAPI), and runs STT
      on `docs/phase2/test-utterance.wav` ("Open text edit n type hello",
      299 ms) and TTS (transcribed back word for word). The models were
      downloaded and verified through the Setup screen instead (230 MB in
      15 s; "Check files" 5 s). `voice_check play` was not run.
- [ ] ONNX Runtime: `ort` downloads its Windows binaries at build time;
      check the app starts on a clean machine without a separate install
      (or that the needed DLL is bundled). (Here it was linked dynamically,
      see "Build": `onnxruntime.dll` must sit next to `pindot.exe`, and the
      installer does not bundle it yet.)
- [x] Test build (`--release --features test-switches`): `pindot --test-voice
      docs/phase2/test-utterance.wav --model haiku` prints `RESULT PASS`.
      (Listening, Thinking, Acting twice, Speaking, Idle. Speech to text
      321 ms, the turn 7.0 s, first audio 746 ms after it, 8.1 s from the
      key release to the first sound. Claude called `request_access` twice
      and said it could not reach a text editor: no app resolves on
      Windows yet. A debug build takes 11.5 s to the first audio.)
- [ ] Real use: hold Ctrl+Win, say "Open Notepad and type hello", let go.
      Expect Listening, Thinking, Working, the approval window, the text in
      Notepad and a spoken reply. The Start menu must not open.
- [ ] Barge-in: hold Ctrl+Win while it speaks: speech stops and it listens.
- [ ] Esc during a turn: the action stops, speech stops, the turn is
      interrupted (Esc is written to the ConPTY), and the next request
      works in the same session.
- [ ] The persistent `claude.exe` under ConPTY survives 10 minutes idle and
      the next request starts a fresh session; quitting Pindot leaves no
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
- [x] Setup > Permissions shows the microphone row; "Open Settings" opens
      the microphone privacy page. The row reads the three privacy switches
      ("Allowed" here, all three on). Still to check: switch "Let desktop
      apps access your microphone" off and see "Denied", the note naming
      the switch, and hold-to-talk held back.
- [ ] Setup > Voice models: licenses shown before download, progress,
      cancel and resume (HTTP 206), and "Check files" repairs a file with
      one changed byte. The Moonshine fallback (blob.handy.computer) works
      when huggingface.co is blocked (hosts file).
- [ ] Settings: presets list `Ctrl+Win` (default), `Right Alt`,
      `Right Ctrl`, ...; Record captures a held chord inside the window;
      typing `Fn` is refused with the Windows message; Save writes
      `settings.json` and the new chord works without a restart.
- [ ] Settings > Regenerate token: the old token gets 401 and the new one
      200; check the file ACL with `icacls "%APPDATA%\Pindot\mcp-token"`
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
      only `%APPDATA%\Pindot\`.
