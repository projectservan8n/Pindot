# Windows test checklist

Phases 0 to 3 were built and tested on macOS. Windows was first built and run
on 15 Nov 2026, on one Windows 11 PC (build 26300, two 1920x1080 monitors at
100%, VS 2019 Build Tools, claude 2.1.323 then 2.1.325). A ticked box below
was seen on that PC; a note after a box says how far an unticked one got.

On Windows today:

- **Works**: the build (see "Build"), `cargo test --workspace` (197 + 24 + 6
  tests), the app with Setup, Settings and Activity, the MCP server and its
  guards, the overlay on both monitors, the microphone check, hold-to-talk
  (`Hotkey`, self-healing), the Esc stop key (`StopKey`), speech to text,
  text to speech, the brain in ConPTY, the whole voice loop with a WAV in
  place of the microphone (`--test-voice`, RESULT PASS) and with the real
  microphone and keys, screenshots (`Capture`), mouse and keyboard
  (`Input`), and the window list, apps, approval gate, opening apps and
  bringing a window forward (`Accessibility`). A real turn typed and
  clicked in a test window (`--test-turn`, TURN COMPLETED).
- **Also works, on few apps**: the element tree through UI Automation
  (`read_window`, `find`, click by ref, press without the pointer) and the
  password-field check (`IsPassword`): on a test window, in Edge (a local
  page and one public site), on Paint, on Settings, on File Explorer and
  on Word. Not yet on a chat or mail app, or a heavy web app.
- **Installing**: `scripts\build-windows.ps1` makes the app and a per-user
  installer. A silent install, a start of the installed copy and a silent
  uninstall were run. Starting Pindot twice opens the running one's
  window.
- The permission report lists only the microphone, read from the three
  privacy switches in the registry. Its button opens
  `ms-settings:privacy-microphone`. There is no "Ask" button: Windows has
  no prompt for a desktop app.
- The token and config files are written with Unix mode 600 on macOS. On
  Windows no ACL is set yet: they rely on `%APPDATA%` being per-user.
- `scripts/check-windows.sh` still type-checks both crates from the Mac. It
  only proves the Rust compiles.

Still needed: a second PC or a monitor scaled to 150%, and the items left
unticked. The physical-keyboard pass was done by the PC's owner on 16 Nov
2026 (hold-to-talk, the approval window's Allow, Esc, the tray menu).

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
- [x] The overlay never takes focus. The Alt+Tab list and the taskbar don't
      show it. (Focus stayed with the app in front. The overlay windows are
      now tool windows titled "Pindot cursor": `WS_EX_TOOLWINDOW` set and
      `WS_EX_APPWINDOW` cleared, read back from the running app. Alt+Tab
      itself was not pressed.)
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
      (Built: the target's process is checked before any input is sent.
      `spike_hotkey status` flagged two administrator windows on the test
      PC. No click was aimed at one, so the error itself is unseen.)

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
      (Notepad was not used: the tester had it open. On Paint instead:
      104 controls in 0.46 s, with File and View, Save/Undo/Redo and their
      shortcuts, every tool and shape; the pencil picked by ref; "Don't
      save" found and pressed in the closing dialog. Items of a closed menu
      are not in the tree on Windows.)
- [x] A web page: Edge with its own profile on a local page. The first read
      lists the page (0.45 s, after one retry: a browser builds the tree
      only once asked); a field typed into; a checkbox, a button and a link
      pressed by ref without the pointer; typing into the page's password
      field refused, the field left empty.
- [x] A public site: Edge with its own profile on example.com. The page was
      listed on the first read (0.48 s); its link pressed by ref; the page
      it leads to read (74 controls, 32 links).
- [x] A store app: Settings (its frame is an `ApplicationFrameWindow`, the
      app is the process behind it). `open_app("Settings")` opened it and
      named it; 39 controls in 0.2 s; the row "Personalization" pressed by
      ref without the pointer and its page read (Background, Colors,
      Themes, ...); `find("Colors")`; Alt+F4 closed it. Nothing was changed
      in Settings. `open_app("Calculator")` said, rightly, that no such app
      is installed on this PC.
- [x] File Explorer, on a test folder in a window the test opened: 70
      controls in 0.17 s (each file with its name, date, type and size,
      the side pane, the toolbar with what is disabled). A click on a
      file's row by ref selects it: Cut, Copy, Rename and Delete came
      alive and nothing opened. Before the fix it opened the file (UI
      Automation's Invoke on a file is "open"); see `press_action`. A
      double-click on a row covered by another app's window was refused.
      Dates read "15 Nov 2026 10:04 pm" and `find` matches them (Explorer
      puts invisible direction marks between the parts). A row that can be
      selected and opened is now clicked for real (the pointer put back):
      nothing tells a file, where a click selects, from a tile that a
      click opens.
- [x] A click at x,y on a button presses it without the pointer (it stayed
      at 740,701); the same x,y again is a real click and the pointer is
      put back; a click at x,y in a text box is a real one. Seen in the
      full app, with the overlay up.
- [x] A classic menu bar (the test window's `MainMenu`): `read_window` with
      menus lists "File > New", "File > Save as..." with Ctrl+Shift+S,
      "Edit > Copy" as disabled, "View > Word wrap" as checked and "View >
      Zoom > In"; `find("Save as")` finds the item; a click on its ref ran
      the command with no pointer and no open menu; the disabled item was
      refused ("That control is disabled") and no menu was left open.
- [x] A right-click without the pointer where the app answers for it: on
      a file's row in File Explorer, by ref and at x,y, the context menu
      came up ("Copy as path", "Properties", "Open with") and the pointer
      stayed at 740,701; on a link in Edge ("Open link in new tab", "Copy
      link"); on File Explorer's search box (a XAML control). A classic
      text box answered "done" and showed no menu, so classic controls get
      a real right-click: its menu came up and Esc closed it.
- [x] `scripts\check-macos.ps1`: both crates type-check for
      `aarch64-apple-darwin` from this PC, the macOS-only files and the
      test switches included. Rust only: nothing is linked or run.
- [x] Word, on a blank document the test made: opened by name; the start
      page read (35 controls); a blank document; 126 controls in 0.38 s
      with the ribbon; 17 characters typed ("Word Count 3 words"); Bold
      pressed by ref; Alt+F4 and "Don't Save" by ref. Word closed, nothing
      saved.
- [x] Timings in a release build, as `docs/phase3` took them on the Mac: a
      screenshot 65.7 ms median, 88.6 ms p95, 111.6 ms max over 40 back to
      back and 65.9 ms median over 12 two seconds apart (Mac: 50.7 and
      86.1 ms median); capture 16 ms, mask 1 ms, resize 26 ms. `read_window`
      30 ms, `find` 30 ms, a press by ref 16 ms, `desktop_context` 2 ms on
      the test window. `claude` ready in 2.5 s (Mac 2.1 s). Speech starts
      35 to 70 ms after its text (Mac 43 to 124 ms): the first utterance
      took 520 ms until the model was warmed up when it loads.
- [x] Secure fields: a password box (UIA `IsPassword`) is reported as
      `securetextfield` and `type_text` refuses to type into it.
      (A classic password box. Still to try: a password field on a web
      page, in Chrome and in Edge.)
- [x] `StopKey` with `WH_KEYBOARD_LL`: while armed, a physical Esc returns 1
      (swallowed: Notepad must not see it), stops the action in flight and
      shows the "Stopped" banner. Our own `key esc` (tagged with
      `dwExtraInfo` = our marker) passes through.
      (`spike_hotkey esc` with a test window in front and unmarked,
      synthetic Esc presses: armed, both presses reported and the window
      saw none; disarmed, the window saw the press. The physical key
      stopped a turn in flight on 16 Nov 2026, see (f); the banner was not
      looked at.)
- [ ] Approval window: `request_access(["Notepad"])` shows our prompt, Deny
      and "Allow for this session" both work by mouse, Esc denies, and a
      120 s timeout denies. Extra warning shows for Terminal/PowerShell,
      Explorer, Settings/Control Panel, password managers.
      (Seen in a release build: the prompt on top with both apps, the
      file-manager warning for File Explorer and the reason marked "not
      verified"; Deny, pressed over CDP, gave the tool "The user denied
      access". Allow, Esc and the timeout were not tried, and nobody has
      used the mouse on it. Since then the PC's owner clicked "Allow for
      this session" during real use: "approval ...: user ALLOWED".)
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
- [x] ONNX Runtime: `ort` downloads its Windows binaries at build time;
      check the app starts on a clean machine without a separate install
      (or that the needed DLL is bundled). (Linked dynamically, see
      "Build": `onnxruntime.dll` must sit next to `pindot.exe`. The
      installer puts it there and the installed copy started. Not on a
      clean machine.)
- [x] Test build (`--release --features test-switches`): `pindot --test-voice
      docs/phase2/test-utterance.wav --model haiku` prints `RESULT PASS`.
      (Listening, Thinking, Acting twice, Speaking, Idle. Speech to text
      321 ms, the turn 7.0 s, first audio 746 ms after it, 8.1 s from the
      key release to the first sound. Claude called `request_access` twice
      and said it could not reach a text editor: no app resolves on
      Windows yet. A debug build takes 11.5 s to the first audio. Run again
      once desktop control was built, with a synthesised request (Parakeet,
      haiku): the test window's app opened, text typed, a button pressed
      and the reply spoken; RESULT PASS, 14.7 s from the key release to
      the first sound.)
- [x] Real use: hold Ctrl+Win, say "Open Notepad and type hello", let go.
      Expect Listening, Thinking, Working, the approval window, the text in
      Notepad and a spoken reply. The Start menu must not open.
      (Done by the PC's owner on 16 Nov 2026, with the physical keys, the
      real microphone and the release build, on Paint and a browser, not
      Notepad: 20 holds in the app's log, each request heard, the approval
      window answered with Allow ("user ALLOWED"), Paint opened and drawn in
      on request (a circle with the oval tool, a star, letters with the
      brush), a web page scrolled and read aloud. Speech began 110 to 230 ms
      after its text; once 2.3 s, behind a message still being spoken. In
      the browser one click at a position landed on a neighbouring link and
      the assistant went back and corrected it.)
- [ ] Barge-in: hold Ctrl+Win while it speaks: speech stops and it listens.
- [x] Esc during a turn: the action stops, speech stops, the turn is
      interrupted (Esc is written to the ConPTY), and the next request
      works in the same session.
      (The physical Esc, pressed by the PC's owner while a drawing request
      ran: "EMERGENCY STOP: user pressed Esc (action cancelled, input
      released)" and "turn interrupted with Esc in 100 ms". The log ends
      there, so the next request in the same session is not in it.)
- [ ] The persistent `claude.exe` under ConPTY survives 10 minutes idle and
      the next request starts a fresh session; quitting Pindot leaves no
      `claude.exe`, `node.exe` or `conhost.exe` behind.

## (g) Phase 3: app, settings, setup, release

- [x] Tray icon in the notification area with Setup, Settings, Activity,
      the hold-to-talk switch and Quit. Quit leaves no process.
      (The icon is in the hidden-icons flyout. Pressed through UI
      Automation; the menu, read with `MN_GETHMENU`, has the status line,
      Setup..., Settings..., Activity..., the checked Hold-to-talk
      (Ctrl+Win) and Quit Pindot. Quit exited cleanly: no `pindot`, no
      WebView2 process, the port free. Used by hand by the PC's owner on 16
      Nov 2026: the icon in the hidden-icons flyout, then Settings.)
- [x] No console window flashes when the release build is started the way
      Explorer starts it (`explorer.exe <path>`, so no console anywhere):
      every new visible window was watched for 25 s, through the start,
      `claude --version`, `claude auth status` and the voice brain, and
      only Pindot's own windows appeared.
- [x] First start opens Setup without stealing focus from the app in front.
      (A private instance that needed Setup: its window came up first in
      the z-order and the foreground window stayed another app's.)
- [x] Starting Pindot a second time opens the running one's window and the
      second process ends (54 ms). Seen with two private instances on one
      data folder: the window, closed before, came back; minimized before,
      it was restored and first in the z-order. Another data folder runs
      beside, and after the first quits the next start runs normally. Not
      seen: the window taking the keyboard when a person starts it (the
      test's launcher was a background process, which Windows never lets
      take the foreground).
- [ ] Setup: the `claude` check finds Claude Code and reads `claude auth
      status --json`; signed out shows "run `claude login`". (Found and
      signed in: seen. Signed out: not seen. `find_claude` looks for
      `claude.exe` on the PATH and in the home folder's usual places. An
      npm install gives `claude.cmd`, which Pindot cannot start in its
      terminal, so Setup says to install the native app.)
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
- [ ] Release: `scripts\build-windows.ps1` (see
      `docs/release.md`); install on a clean Windows 11, launch, check
      Setup, `curl -H "Authorization: Bearer <token>"
      http://127.0.0.1:47823/health` answers `ok`, and uninstall leaves
      only `%APPDATA%\Pindot\`.
      (On the build PC, not a clean one: a silent install into a test
      folder; the installed copy started with its own data folder and its
      server answered 401 without the token; a silent uninstall removed
      the files, the shortcuts and the Installed-apps entry. The pages of
      the installer and of the uninstaller were walked as well, by sending
      their buttons' commands; both ended clean. `/health` with the token
      and a person doing it by hand were not tried.)
