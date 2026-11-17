# Windows test checklist

Phases 0 to 3 were built and tested on macOS. Windows was first built and run
on 15 Nov 2026, on one Windows 11 PC (build 26300, two 1920x1080 monitors at
100%, VS 2019 Build Tools, claude 2.1.323 then 2.1.325). A ticked box below
was seen on that PC; a note after a box says how far an unticked one got.

On Windows today:

- **Works**: the build (see "Build"), `cargo test --workspace` (308 + 35 + 6
  tests), the app with Setup, Chat, Vault, Settings and Activity (see (h)
  for what the pull of 16 Nov 2026 brought), the MCP server and its
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
- [x] Streaming mode, the mechanism (16 Nov 2026, a copy with no main
      window, the mode set in `settings.json`): with it on, both overlay
      windows read `WDA_NONE` while idle, so capture sees them; during
      each of Pindot's own `screenshot` calls they read
      `WDA_EXCLUDEFROMCAPTURE` (about 60 ms per screenshot in a debug
      build) and `WDA_NONE` again afterwards. With it off they read
      `WDA_EXCLUDEFROMCAPTURE` throughout. The window library's own
      invisible, untitled window is click-through and unprotected too, so
      it is switched and put back with them; nothing is drawn in it. 25
      screenshots in the release build: median 110 ms with the mode on,
      120 ms with it off (two monitors, timed to 10 ms), so no cost that
      shows.
- [x] Streaming mode, what an outside capture gets (a stand-in for a
      recorder: an 8-pixel marker drawn in the overlay's corner, and
      another process copying that patch of the screen about 300 times a
      second). With the mode on the marker was in 440 of 440 captures
      while idle; while Pindot took three screenshots of its own it was
      missing in 17 of 2661 and went and came back exactly three times,
      and it was there at the end. With the mode off it was in none of
      442, and none of 2664 during the screenshots. So a recorder sees the
      overlay only in streaming mode, and it blinks out for each of
      Pindot's own screenshots, as designed.
- [x] Streaming mode shows every Pindot window (the owner's request, 17
      Nov 2026: "also show the window of Pindot not just the cursor").
      Read from a private copy's windows on a hidden desktop: with the
      mode on, the two cursor windows, the main window and an approval
      popup are all `WDA_NONE`; during each of four of Pindot's own
      screenshots all of them were `WDA_EXCLUDEFROMCAPTURE` and came back;
      switched off in Settings while running, all of them went back to
      excluded at once, and a popup opened then was excluded too; on again,
      a new popup was visible. (The approval popups were answered Deny.)
- [ ] Streaming mode, with OBS itself (Settings, "Streaming mode" on; not
      tried: OBS was not run): the AI cursor and its caption bubble, the
      Pindot window and an approval popup show up in OBS (window or
      display capture); Rusty's own `screenshot` leaves them out (they are
      switched to `WDA_EXCLUDEFROMCAPTURE` for the length of one copy, so a
      viewer sees them blink out for a moment); after a capture that
      failed (unplug a display mid-capture, or ask for a display that is
      gone) they are visible to OBS again; switching the setting off hides
      them from OBS again with no restart.
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
- [ ] Sleep and wake, then a hold. (Tried by the PC's owner on 16 Nov
      2026: Start > Power > Sleep. Windows logged "The system is entering
      Away Mode", not a suspend: the screen went off and the PC kept
      running. The hold ten seconds later was heard and answered; it was
      reported from the key state, not by the hook. A real suspend is still
      to see.)
- [x] Hold-to-talk with a blind hook (an administrator's window in front):
      the keys are looked at every 50 ms. With the hook made blind in a
      test, eight short holds were all reported; before, with a look every
      500 ms, the start was a median 537 ms late and one hold was lost.
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

## (h) Chat, vault, brain folder (pulled 16 Nov 2026)

Built on the Mac, type-checked for Windows there, first run here on 16 Nov
2026 (claude 2.1.325, haiku). The app was a private instance with its own
data folder and port, its pages driven over the WebView2 debugging port;
the browser was Edge with a profile of its own on `docs/vault/test-page`.

- [x] `cargo test --workspace`: 308 + 35 + 6 pass (two are not run by
      default: a download, and the real credential store). The frontend's
      48 tests pass. One server test failed once in seven runs before the
      fix to `MockStopKey`; eight runs in a row pass after it.
- [x] The new look: the fonts, Rusty and the sidebar draw as in the Mac's
      screenshots; Setup, Chat, Vault, Activity and Settings open.
- [x] The AI cursor has its colour (fix `43646b9`, release build). Before
      it the cursor was black here as on the Mac: every gradient stop
      computed to `rgb(0, 0, 0)` while `--accent` was `#ff2d95`. After it
      the stops are the pink ones, `point` drew a pink Rusty with his
      label, and a colour saved in Settings reached both monitors'
      overlays at once.
- [x] Chat without the voice: with hold-to-talk off the assistant started
      by itself; a typed message was answered on screen in 2.5 s.
- [x] Attachments: a text file and a picture pasted into the message box
      were copied to `attachments\<session>\`, and the answer named the
      word in the file and the picture's colour (two `Read` steps, no
      permission prompt).
- [x] A read outside the attachments folder stops at a permission prompt
      (`spike_brain --attach`, asked for a file next to the brain folder).
- [x] A sent picture shows as a picture. (It was a "?" tile: the message
      kept only the file's name. Fixed in shared code, so on the Mac too.)
- [x] History lists the conversations and opens one.
- [x] Brain folder, `spike_brain --brain-folder` (the Mac's test folder,
      rebuilt here): the lucky number came from `notes\favourites.md`
      (`Read`), "remember my dentist" wrote `pindot-notes\dentist.md` with
      no prompt (`Glob`, `Write`), and asked to write next to the folder
      the assistant declined without trying. In the app: the folder set
      through `settings_save`, a fresh conversation, and the answer used
      the folder's CLAUDE.md and a note.
- [x] A write outside `pindot-notes`, really tried: `Write` to the top of
      the folder and `Edit` of its CLAUDE.md each stopped at Claude Code's
      dialog and changed nothing. (Pindot did not know these two dialogs
      and waited out the time limit, 15 minutes in the app; it now ends
      the turn at once, 12 s with `spike_brain`. Shared code.) A search
      and a Glob outside the folder stop at a permission prompt too, and a
      Grep inside is allowed.
- [x] The same refusal in the app itself: asked in Chat for outside.txt in
      the top of the brain folder, the turn ended after 4 s with one failed
      `Write` step and "Claude wanted a permission I can't grant by voice,
      so I stopped that request."; no file. (The line above it read "You
      stopped this.", though Pindot had pressed Esc, not the user; it now
      reads "This request was stopped.", from the next build on.)
- [x] The brain folder's "Choose…": the system's folder dialog ("Choose
      your brain folder") was given a folder and the field showed it.
      This and the other file dialogs below were answered on a desktop
      nobody sees (`scripts\hidden_desktop.py`, `scripts\answer-file-
      dialog.ps1`), so nothing came up on the screen of the PC's owner.
- [x] Vault, storage: a login saved through the form is in `vault.json`
      with every value sealed; the key is the Credential Manager item
      `master-key.Pindot Vault`. The ignored test `real_credential_store`
      stores, reads back and removes a key under a test name.
- [x] Vault, the page checks, on Edge: `vault_logins` lists the login on
      `http://localhost:4310` and nothing on `http://127.0.0.1:4310`; a
      password into the username box is refused; a fill on the 127.0.0.1
      page is refused ("this page is not http://localhost:4310"). (Before
      the fix every fill was refused: Edge was "not a browser".)
- [x] Vault, the approval: a username fill opened "Let Claude sign in with
      a saved login?" with the login, the site and the username; Deny (sent
      over CDP) gave "the user did not allow this login".
- [ ] Vault, by hand: "Allow for this session", the text typed into the
      page, the authenticator code, Esc during a fill. Chrome, Brave,
      Vivaldi, Opera and Arc (known by their executables, not tried).
- [x] Vault: "Import passwords…": the file dialog ("Choose a password
      export (CSV)") was given a two-row CSV in Chrome's format and the
      preview read "2 rows read ... 2 new, 0 already saved". A second
      file in Bitwarden's format (one row with an authenticator key) was
      imported for real: "Import finished, 2 imported", both logins
      listed, the one with a key tagged 2FA, every value sealed in
      `vault.json`. The key this made in Credential Manager was removed
      again (no real vault existed).
- [x] The look after the Mac's interface fixes (`c87164e`), in WebView2:
      ticked pink checkboxes with a long sentence beside the box, rings
      with a dot, the app's own dropdowns with a chevron, round colour
      swatches, room for the permission names in Setup, the 2FA tag
      clear of the login's name.
- [x] Settings > Hold-to-talk > Shortcut speaks Windows: the box read
      "Ctrl+Cmd" beside a list and a hint that said "Ctrl+Win". It now
      shows "Ctrl+Win" and "Right Alt" (and says so in its example), and
      still saves the macOS words (`format.ts`, `hotkeyWords`).
- [x] `npm run check` passes again: `no-style-attributes.test.ts` used
      Node's `fs` without Node types (5 errors, on both systems). It reads
      the files through Vite now and still fails on a planted `style=`.
- [x] Settings > Microphone lists the inputs by name without opening them
      (four here, with the default named).
- [ ] A chosen microphone, and dictation (the button, and the hold-to-talk
      key with the message box focused): they need a real voice.
- [x] "Attach files": the system dialog was given a file and the file
      showed as attached (name and size, with Remove).
- [ ] Files dropped onto the window (a real drag needs the pointer).

## (i) The new brains (pulled 17 Nov 2026)

Built and run on the Mac (see CLAUDE.md, "Pindot's own loop"). First
built and started on Windows on 17 Nov 2026; the PC's owner is trying the
local model by hand.

- [x] `cargo test --workspace` passes (`native_loop` included): 358 + 35
      + 12 + 6, 3 + 3 ignored. vitest 56, svelte-check clean.
- [x] LM Studio open with its server off (its "start on launch" is off by
      default) was "No model server is running", which read as "Pindot
      can't see LM Studio". Pindot now lists a local server that is
      installed but off (`start_local::installed`: LM Studio's `lms` in
      `~\.lmstudio\bin`, Ollama's `ollama.exe` in
      `%LOCALAPPDATA%\Programs\Ollama` or on the PATH) and offers "Start LM Studio's server" (`lms
      server start`) or "Start Ollama's server" (`ollama serve`, left
      running). Seen here: LM Studio listed as installed, its start asked
      for while it already ran returned in 2.7 s with the server still up
      and 12 models listed; an empty leftover Ollama folder counts as not
      installed; a cloud provider is refused. The start of a server that
      was really off was done by hand with `lms server start` (the owner
      was using the server, so it was not stopped for a test).
- [ ] Setup and Settings show "Choose a brain" / Brain with four choices;
      Claude Code stays the default and works as before.
- [ ] Keys: `cargo test -p pindot-core --lib real_secret_store -- --ignored`
      saves, reads and deletes a key in Credential Manager under
      "Pindot AI keys TEST (safe to delete)". In Settings, an API key saved
      then deleted leaves no "Pindot AI keys" entry.
- [ ] A local model: install Ollama for Windows, `ollama pull qwen3:4b`,
      start it with `OLLAMA_CONTEXT_LENGTH=32768`. "A model on this
      computer" finds Ollama and lists the model; Test this model passes;
      `cargo test -p pindot-core --test native_loop real_ollama -- --ignored
      --nocapture` passes; by voice, "How many displays do I have?" is
      answered (the Mac: 23 s on a MacBook Air).
- [ ] Codex: `npm install -g @openai/codex`, `codex login`. Settings shows
      "Codex is signed in". `find_codex` must find the native `codex.exe`
      inside the npm package (`...\npm\node_modules\@openai\codex\
      node_modules\@openai\codex-win32-*\vendor\*\bin\codex.exe`);
      the npm `codex.cmd` is a script Pindot cannot start. Then
      `real_codex_chatgpt` (--ignored) passes, and by voice the displays
      question is answered (the Mac: 6.7 s).
- [ ] Codex starts with no console window flashing (it goes through
      `brain::hidden`).
- [ ] Esc during a loop or Codex turn stops it (Codex: the process is
      killed) and the next request works.
- [ ] Chat shows a loop or Codex conversation, and History opens it (the
      file is in `%APPDATA%\Pindot\conversations\`).

## (j) More speech engines (17 Nov 2026)

- [x] Whisper large-v3 turbo, real model, release build, on the test
      recording: "Open text edit and type hello." twice (the only engine to
      get every word). 7.7 s of encoder per request on the Ryzen 5 5600X,
      about 0.7 s of decoding. Downloaded through Pindot's downloader into
      a scratch folder (checksums passed, `onnx/` subfolder created).
      `cargo test --release -p pindot-core --lib real_whisper -- --ignored`.
- [x] Settings shows the cloud choices: OpenRouter's model list loaded live
      (about 30 transcription models), the key fields, the notes on what is
      sent where; "there is no OpenRouter key for speech to text" as the
      reason hold-to-talk is off; saving ElevenLabs for speaking without a
      voice is refused. (A private copy on a hidden desktop; no key was
      saved.)
- [ ] ElevenLabs Scribe, ElevenLabs speech and an OpenRouter model with
      real keys: a spoken request transcribed, a reply spoken, Preview.
- [ ] Whisper with a GPU (DirectML on Windows, CoreML on a Mac): not built;
      on a CPU it is too slow for quick commands.

## Compare with the Mac

`scripts/bench_tools.py` times the tools the same way on both systems (its
header has the commands). Release builds, a small test window with one
button, each call timed around its HTTP request. The Windows column was
measured on the test PC (Ryzen 5 5600X, two 1920x1080 monitors) on 16 Nov
2026. The macOS column was measured the same day on a MacBook (macOS 26,
one built-in display 1470x956 pt at scale 2, VS Code and seven other apps
open), three runs of `spike_mcp` against `spikes/test-target`.
`docs/phase3` has the Mac's screenshot figures from an earlier run on
another display: 50.7 ms back to back, 86.1 ms spaced.

| Median (p95) | Windows | macOS |
|---|---|---|
| `screenshot`, 40 back to back | 66 to 76 ms (89 to 103) | 68 ms (72 to 76) |
| `screenshot`, 12 two seconds apart | 66 to 69 ms (67 to 91) | 121 to 123 ms (126 to 132) |
| `read_window` | 30 to 37 ms (32 to 57) | 1.5 to 3.7 ms (4 to 8) |
| `read_window` with menus | 30 to 34 ms (31 to 73) | 1.5 to 4.0 ms (3 to 9) |
| `find` | 30 to 35 ms (36 to 49) | 2.8 to 5.9 ms (6 to 10) |
| `desktop_context` | 2 ms (3 to 19) | 1.8 to 3.2 ms (4 to 7) |
| `click` by ref, a press | 13 to 16 ms (18 to 34) | 106 ms (108) |
| `claude` ready after start | 2.5 s | 3.1 s (one `--test-turn`); 2.1 s (`docs/phase2`) |
| Speech starts after its text | 35 to 70 ms warmed up; 110 to 230 ms in real use | 6.7 ms warmed up (`voice_check tts`); 43 to 124 ms (`docs/phase2`) |

On the Mac a press by ref takes about 106 ms every time, against 13 to 16
ms on Windows. The press path on macOS was not changed by the Windows work;
where the time goes was not measured. In the first of the three runs one
press of ten failed with AXError -25204 (cannot complete) and the window
logged no click; it did not come back in two more runs and three presses
on a freshly opened window.

## After pulling on a Mac

The Windows work changed code that macOS runs too. `scripts\check-macos.ps1`
showed that it compiles for macOS. Run on a MacBook on 16 Nov 2026, after
pulling 8edb60e:

- [x] `cargo test --workspace`: 202 + 24 + 6 passed, 1 ignored, no
      warnings; vitest 12 passed; svelte-check 0 errors. `cargo build
      --release -p pindot` without warnings.
      (Not done: `scripts/build-mac.sh`. The signed bundle was the running
      Pindot, and rebuilding it under a running app can crash it.)
- [x] Quitting `claude`: Ctrl+C until it goes. One `--test-turn` (haiku,
      Ask) pressed the test window's button by ref (logged by the window),
      the Stop hook and the transcript agreed, and after `exit`: "child
      claude still running: false", no "killing it" warning, no `claude`
      left behind. (Quitting the whole app with a voice brain running was
      not tried: the user's own Pindot was running and was left alone.)
- [x] The speech model is warmed up when it loads: "Phonon loaded ... in
      45.5ms (warm-up 28.1ms)", then the first audio after 6.7 ms
      (`voice_check tts`; the text came back word for word).
      (The full voice loop, `--test-voice`, was not run: its synthetic Fn
      would also reach a Pindot that is already running.)
- [x] The AI cursor lands where it did: the app logs "1 pt per page pixel";
      with `--capturable`, `point` and `click` by ref on the test button
      (centre at 340,285 pt) put the cursor's tip at about 340,283 pt in
      `screencapture`, with the "here" and "Press Pindot Test Button"
      labels. One display only.
- [x] A closed menu's item pressed by ref: Calculator > About Calculator,
      found with `find` and pressed by ref, opened the About window.
      (Whether the first press failed and the retry ran is not logged.)
- [x] A shortcut with `cmd`: `cmd+w` closed that About window and `win+q`
      quit Calculator (`win` is Command on a Mac).
- [x] `python3 scripts/bench_tools.py --app test-target`: figures in the
      table above. The test window needs `--regular` (or request_access
      can't name it) and a `--seconds` longer than the run; the script's
      header now says so.
- [ ] After the Windows fixes to the pull of 16 Nov 2026: a picture sent
      in Chat shows as a picture, not a "?" tile (the message now keeps
      the file's whole path); the vault still knows Safari and Chrome
      (`is_browser` got the Windows executables); a brain folder and an
      attachment are still read (`rule_path_of` was split out, the Unix
      form is unchanged).
- [ ] With a brain folder, ask for a file outside `pindot-notes` ("create
      outside.txt in the top of the folder"): the turn ends at once with
      "Claude wanted a permission I can't grant by voice", not after 15
      minutes (`turn::is_permission_prompt`).
