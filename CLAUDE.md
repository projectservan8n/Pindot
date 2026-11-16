# Pindot

Open-source (MIT) desktop app for macOS and Windows. Pindot gives the user's
own Claude Code CLI eyes and hands on the desktop through MCP tools, and the
user drives it by voice. The product name must never contain "Claude".

The spec is the build brief, kept outside this repo. This file records the
decisions and conventions that came out of building it. Phases 0 to 3 are
done on macOS (spikes; tools, accessibility tree and guardrails; the voice
loop; the app with setup, settings, downloader, activity viewer and the
release build). The evidence is in `docs/phase0/` to `docs/phase3/`.
Windows was first built and run on 15 Nov 2026: the app, the server, the
overlay, the voice loop (hold-to-talk, speech, the brain) and desktop control
(screenshots, windows, apps, approval, the element tree, mouse and keyboard)
work there. See "Windows: first build and run" below and
`docs/windows-test-checklist.md` for what was seen and what is left.

Pindot was called **Deskhand** until 15 Nov 2026. The captured run outputs
(`docs/phase*/*.log`, `*.txt`) and the screenshots keep the old name, as
recorded; everything else was renamed (crates, bundle id `dev.pindot.app`,
`PINDOT_*` variables, `pindot:*` events, `X-Pindot-Session`).

## Hard rules

- **Brain = the user's interactive `claude` CLI in a PTY.** Never `claude -p`
  or `--print`, never the Agent SDK, never an API key. If interactive mode is
  blocked, stop and ask; do not fall back.
- The child process gets `ANTHROPIC_API_KEY=""`, `TERM=xterm-256color` and
  `COLORTERM=truecolor`. We remove `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and
  the other parent-session variables so a nested `claude` does not think it is
  a sub-agent.
- Never `--dangerously-skip-permissions`. Tools are allowed with
  `--allowedTools mcp__desktop`. That flag is variadic, so it goes last on the
  command line.
- Never touch the user's global Claude config while developing. Pass the
  server with `--mcp-config <file> --strict-mcp-config` and hooks with
  `--settings <file>`, and run nested sessions from `.spike-cwd/`. The
  product's "Connect to Claude Code" button (`claude mcp add --scope user`) is
  a user action, not something tests run.
- Secrets: the MCP bearer token is generated at runtime. It lives in the
  per-user data dir (`~/Library/Application Support/Pindot/`, dir 0700,
  files 0600) with the generated `claude-mcp.json` and `claude-settings.json`,
  never in the repo. Model weights are downloaded at runtime, never committed.
- Reference code (Tarsee's `claude-interactive-pool.js` and
  `transcript-tailer.js`) is private and read-only: reimplement it, never copy
  it. Handy (cjpais/Handy) and handy-keys are MIT. Nothing was copied from
  them; our Fn tap is our own code.

## Layout

- `crates/pindot-core`: everything testable without a UI.
  - `platform/`: OS traits `Capture`, `Input`, `Hotkey`, `StopKey` (global
    Esc), `Accessibility` and `Overlay`. Each has a macOS implementation
    (`platform/macos`), a Windows one (`platform/windows`: `keyhook`,
    `permissions`, `capture`, `input`, `apps` and `uia`) and mocks
    (`platform/mock.rs`). `platform/windows_logic.rs` holds the Windows
    decisions that need no Windows API (what the microphone switches mean,
    what the key hook does with each key, what a window is, how an app is
    named and found, which keys a shortcut presses), so they are unit-tested
    on every OS.
  - `server/`: the rmcp streamable-HTTP MCP server named `desktop` and the
    hook receivers (`/hooks/stop`, `/hooks/prompt`). `tools.rs` holds the 15
    tools; `session.rs` the per-session approvals, refs and hit test;
    `control.rs` the input lock, Esc halt and in-flight actions;
    `approval.rs` the `Approver` trait and app warnings; `activity.rs` the
    JSONL activity log; `tree.rs` the AX tree as text; `shot.rs` capture,
    masking and zoom.
  - `brain/`: the PTY session (`pty.rs`), config-file writers (`config.rs`),
    transcript reading (`transcript.rs`) and one driven turn (`turn.rs`: Stop
    hook, Esc interrupt, permission-prompt and exit detection).
    `persistent.rs` is the voice brain: one `claude` kept running across
    turns on a worker thread; `tailer.rs` follows its transcript JSONL.
  - `voice/`: the voice loop. `machine.rs` is the pure state machine
    (inputs in, effects out, unit-tested), `runner.rs` drives it with real
    I/O, `audio.rs` the cpal mic/speaker and the test WAV mic, `engines.rs`
    Moonshine (transcribe-rs) and Phonon (ptts), `models.rs` the pinned model
    catalog, download and SHA-256 check, `speaker.rs` the speech queue.
    `src/bin/voice_check.rs` checks devices, models, STT and TTS by hand.
  - `coords.rs`: maps screenshot pixels to global points. `paths.rs`: the data
    dir, the token (create, regenerate, 0600) and its constant-time compare.
  - `settings.rs`: `settings.json` (versioned, migration, env overrides,
    validation). `connect.rs`: the `claude mcp add/remove` arguments and the
    read-only connection status. `platform/hotkey.rs`: the hold-to-talk
    binding (parse, labels, the pure chord tracker). `platform/macos/
    permissions.rs`: read-only permission checks and the button-only
    requests. `platform/macos/window.rs`: show a window without focus.
  - `src/bin/spike_*.rs`: Phase 0 spikes.
- `app/src-tauri`: the Tauri 2 app: `overlay.rs` (NSPanel AI cursor and
  "controlling input" banner), `approval.rs` (the approval window, answered
  by a human), `ui.rs` (tray, the main window and every command it calls),
  `test_turn.rs` (`--test-turn`, one real `claude` turn for the done-when
  check), `voice.rs` (the voice loop and the `--test-voice` path) and
  `lib.rs`. `app/src`: the main window in Svelte 5 + TypeScript (views
  Setup, Settings, Activity by URL hash; `lib/format.ts` is unit-tested
  with vitest). `app/public`: the overlay (with the voice pill) and approval
  pages, plain HTML/JS. Vite builds both into `app/dist`, which Tauri embeds
  at compile time: rebuild the frontend before `cargo build -p pindot`.
- `spikes/test-target`: a Swift test window for the Phase 0 spikes.
  `spikes/mcp-call.py` is the Phase 0 one-shot MCP client;
  `scripts/mcp_call.py` keeps one MCP session across calls (`init`, `call
  TOOL 'json'`, `list`, `end`) for manual tool tests.

## Decisions and why

- **MCP server**: rmcp 3.10 `StreamableHttpService` behind our own axum guard.
  It binds 127.0.0.1 only, on default port 47823 (`PINDOT_PORT` overrides
  it). Any `Origin` header gets 403, a `Host` other than
  127.0.0.1/localhost gets 403, and a missing or wrong bearer token gets 401.
- **Coordinates**: tools speak in pixels of the latest screenshot, whose long
  edge is at most 1280 px. The server maps them to global points per display.
  `click` refuses to run before a screenshot exists.
- **Stop hook**: an HTTP hook in the `--settings` file posts to
  `/hooks/stop` with the bearer header. The payload carries `session_id`,
  `transcript_path` and `last_assistant_message`. We treat the transcript as
  the truth and wait until its last assistant turn matches the hook. Note that
  if any settings level defines `allowedHttpHookUrls`, it must include our URL.
- **Folder trust**: in claude 2.1.325 the trust dialog's default answer is
  "No, exit". `PtySession` answers it only when the caller passes
  `accept_trust`; it presses Down and checks that `❯` is on "Yes" before
  pressing Enter. Accepting writes a project entry into `~/.claude.json`. The
  product must ask the user in our UI first.
- **TUI parsing** is only used for readiness and dialogs, never for content.
  The TUI places words with `CSI n G` (cursor column) and sends `ESC ( B`.
  `strip_ansi` turns C/G moves into spaces, and matching uses `squash()`
  (lowercase, no whitespace).
- **MCP tools are deferred** in 2.1.325: the model calls `ToolSearch` before
  the first `mcp__desktop__*` call. It works, but costs one extra step.
- **Overlay**: one NSPanel per monitor (tauri-nspanel 2.1.1), borderless and
  non-activating, at ScreenSaver level, joining all spaces and full-screen
  auxiliary, with `ignores_mouse_events`. The app runs with the Accessory
  policy and `activate_ignoring_other_apps(false)`, so it never takes focus.
  Each panel receives the same `pindot:cursor` events in global points and
  draws the cursor only while it is over its own monitor.
  `NSWindowSharingType::None` keeps the cursor out of every screenshot,
  including ours. Verified on macOS 26.5 with `screencapture` and xcap; Apple
  has changed this behaviour before, so re-check it on every macOS major
  release. `--capturable` turns it off for tests.
- **Overlay feedback** (Tony's request): tools tell the overlay what they
  did through `Overlay::action(CursorAction)` → `pindot:action`: a ripple
  for a real click, an inward ring for an accessibility press, chevrons for
  scroll, key caps for `key`, a typing chip (the character count only, never
  the text), a spotlight for `point`, and a drag that the cursor follows
  live (`drag_travel_ms` matches `MacInput::drag`). The cursor glides on an
  arc, banks into the motion and leaves a short trail; with Reduce Motion
  all of that becomes crossfades. `--demo` plays every action on each
  monitor without touching any app.
- **Fn hotkey**: a CGEventTap at the session level on `flagsChanged`, keycode
  0x3F, `MaskSecondaryFn`. It returns NULL to swallow Fn so the Globe action
  never fires. If the tap is disabled by timeout or user input, it re-enables
  itself. Synthetic test events carry the user-data marker 0x0DE5_0F11.
- **Overlay UI is plain HTML/JS**; the main window (Phase 3) is Svelte +
  TypeScript. The overlay page has no framework need.

- **Tools (Phase 1)**: `desktop_context`, `request_access`, `open_app`,
  `focus_window`, `read_window`, `find`, `screenshot`, `zoom`, `click`,
  `type_text`, `key`, `scroll`, `drag`, `wait`, `point`. Descriptions steer
  to refs first (`read_window`/`find`) and screenshots last. Every result
  that carries on-screen text says it is untrusted data. Descriptions tell
  Claude to ask the user before destructive actions, purchases, sending
  messages or typing credentials; `type_text` refuses a focused secure field.
- **Clicks leave the user's pointer alone whenever they can** (Tony: "it
  has its own cursor"): a plain left click is AXPress, on the ref
  (`can_press`) or on the control under the point (`actionable_at`, only
  `PRESS_ROLES`, up to three ancestors, an approved app's only); a
  right-click is AXShowMenu the same way. The fallback is a real click,
  scroll or drag, after which the user's pointer is put back
  (`put_pointer_back`) unless they moved it meanwhile. A failed
  accessibility action is reported, never retried as a real click: it can
  report an error after it worked, and a click at the same spot could land
  on the new dialog.
- **Approval is answered by a human in our UI** (Tony's decision). Each app
  needs approval once per MCP session. The window shows the apps, extra red
  warnings for shells/IDEs, file managers, system settings and password
  managers, and Claude's reason labelled "not verified". Buttons: Deny and
  "Allow for this session"; Esc denies, Enter does nothing, closing denies,
  no answer in 120 s denies. Synthesising a click on Allow is not allowed,
  even in tests (an automated agent was blocked from doing it, rightly).
- **`PINDOT_TEST_AUTO_APPROVE=TextEdit` is test-only** (Tony's decision):
  off by default, prints a loud warning at start and on every use, and is
  never read in the normal path.
- **Character keys go through the main thread**: for `key` with a letter
  (`cmd+v`, `h`), enigo looks the keycode up in the keyboard layout
  (`TSMGetInputSourceProperty`), and macOS 26 kills the process (SIGTRAP in
  `dispatch_assert_queue`) unless that runs on the main queue. `MacInput::key`
  queues such combos on the main dispatch queue; if the main thread doesn't
  start them within 2 s they are cancelled, never pressed late. Named keys
  (return, arrows) stay on the worker thread. A binary without a main run
  loop (tests, CLIs) gets the timeout error for letter keys.
- **The gate**: actions only touch approved apps. Refs carry their app;
  `click`/`scroll`/`drag` at x,y run a hit test on the window list first
  (Pindot's own prompt/activity windows block, the menu bar needs an
  approved app's menu, the Apple menu is never allowed); `type_text`/`key`
  need keyboard focus in an approved app. Screenshots grey out every window
  of a non-approved app and the bare desktop.
- **macOS 26's Dock owns a window covering the whole display** (layer 20,
  alpha 1). The hit test asks AX what is under the point when it meets a
  full-display system window, and the screenshot mask looks through it.
  Before this fix, every click was refused ("on a window of Dock") and every
  window was un-greyed.
- **"Session" means one Claude session, not one MCP connection.** MCP
  2026-07-28 removed transport sessions (SEP-2567) and Claude Code 2.1.325
  sends each tool call as its own stateless request, so state keyed on the
  MCP connection was lost between calls (run C: `open_app` refused right
  after `request_access` approved). Every `claude` Pindot starts gets an
  `X-Pindot-Session: <its --session-id>` header in its MCP config
  (`claude-mcp-brain.json`); approvals, refs, the latest screenshot and the
  lock live in `ServerState::session_for(key)` until `end_session(key)` when
  that `claude` exits. A legacy client with `Mcp-Session-Id` (e.g.
  `scripts/mcp_call.py`) keeps one session per connection; a stateless
  client with neither shares one "shared" session for the server's life.
- **One controlling session**: the input lock is taken on the first action
  and released when that session ends, or after 120 s idle (`LOCK_STALE`).
  A second session gets a clear refusal.
- **Esc stops everything**: an active session tap, armed only while an
  action runs, swallows Esc (no app sees it), cancels the action, releases
  input and broadcasts `ControlEvent::EmergencyStop` (the voice loop's hook
  to stop speech; `brain::turn::drive_turn` sends Esc to `claude` to
  interrupt the turn). Further actions are refused until the user's next
  prompt (`UserPromptSubmit` hook) or the Stop hook, or 60 s.
- **Indicator**: a banner "Claude is controlling input · press Esc to stop"
  at the top of every display while an action runs, and the AI cursor glides
  to the target before every click, scroll and drag.
- **Activity log**: `activity.jsonl` in the data dir (0600, rotated at
  5 MB), one line per tool call; `pindot --activity` opens a live view.
- **Permission mode for nested sessions** (voice: Ask unless the user picks Auto in Settings): `--permission-mode default` plus
  `--allowedTools mcp__desktop` (Tony's decision). claude 2.1.325's help
  lists acceptEdits, auto, bypassPermissions, manual, dontAsk and plan, but
  accepts `default` silently and shows "manual mode on". Undocumented: if it
  stops being accepted, switch to `manual` and tell Tony.
- **Sheets**: a Save sheet has its own CGWindowID but no AX window; the AX
  lookup falls back to the document window that holds the sheet. Right
  after `cmd+w` the sheet is still sliding in (its AX frame isn't the CG one
  yet, or it isn't in the tree yet), so an overlapping sheet counts and
  `window_tree` retries the lookup 3 times, 150 ms apart.

### Phase 2: voice (see `docs/phase2/README.md`)

- **Settings isolation** (Nico's brief: chosen on purpose, login must keep
  working): every `claude` we start gets `--setting-sources project,local`
  and runs in our own empty folder `<data dir>/session` (0700), so
  `~/.claude/settings.json` (Tony's auto mode, xhigh effort, hooks) no longer
  applies; our `--settings` file still does and sets
  `permissions.defaultMode: "default"`, plus `--permission-mode default`.
  `claude login` (OAuth) still works: proven by real turns. Rejected:
  `--bare` (breaks OAuth), `--safe-mode` (drops hooks and MCP).
  `--restricted` is stricter and untested: Tony's call.
- **Token budget** (measured, `--model haiku`): the full Claude Code
  session cost ~40k tokens before the first word, resent on every tool
  call: ~31k of built-in tool schemas (Artifact alone ~15k), the user's
  skills, global CLAUDE.md, Claude in Chrome and agent lists. Every
  `claude` we start now gets `--tools ""` (or Auto's small set),
  `--no-chrome`, `--disable-slash-commands` and
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` (undocumented; found in the 2.1.308
  binary, checked with a real turn): ~5k per call in Ask, ~10k in Auto.
  With no built-in tools the MCP tools load up front, so no `ToolSearch`
  round trip. `--no-chrome` also closes a hole: a voice turn used Claude in
  Chrome to open a site without Pindot's approval window.
- **Claude settings** (Tony's choice): the Settings window picks the voice
  brain's model (`--model`: CLI default, haiku, sonnet, opus, fable),
  thinking effort (`--effort`) and permissions. **Ask** (default): only
  our tools. **Auto**: `--permission-mode auto` with Bash, Read, Edit,
  Write, Glob, Grep, WebFetch and WebSearch; it can only be chosen through
  a warning (runs as the user, the classifier can be wrong or tricked,
  Claude Code recommends isolated environments). App approvals and Esc
  still apply. The brain writes its own `claude-settings-brain.json`.
- **Trusting our own session folder**: it is empty and Pindot's, so the
  voice brain answers the folder-trust dialog with Yes for it (and only it).
  Claude Code itself records that answer in `~/.claude.json` once.
- **The brain stays interactive `claude` in a PTY**: never `-p`, the Agent
  SDK or an API key; `ANTHROPIC_API_KEY` is blanked; sign-in is checked
  with `claude auth status --json`, reading only `loggedIn` and
  `authMethod`. Signed out or not installed: a notice says to run
  `claude login` (we never handle login) and the brain retries 20 s → 5 min.
- **One persistent `claude`** for voice (`brain/persistent.rs`): prompts are
  typed into the same session; a new session starts after 10 idle minutes
  (only if the session was used). If `claude` exits it is resumed with
  `--resume`.
- **Speak each assistant message as it lands** (`SpeakMode::EachMessage`,
  default): reliable in run 1 (the first message was spoken 5.8 s before the
  Stop hook). The transcript is the source; the Stop hook's
  `last_assistant_message` is spoken if the transcript lags. `at_stop` is a
  setting. Side effect: intermediate narration ("An Open dialog appeared,
  so I'll click New Document") is spoken too.
- **"Acting"** comes from the transcript tailer: a `tool_use` sets Working,
  its `tool_result` sets Thinking.
- **Interrupts**: Fn while speaking stops speech and listens (barge-in). Fn
  or Esc while Claude works sends Esc to the PTY and waits up to 3 s for
  "Interrupted"; if it doesn't come, kill and `--resume` (same session). Esc
  is never sent to an idle TUI: two Escs open the rewind menu.
- **Global Esc during voice**: the Esc tap is armed for the whole voice
  interaction (`Control::arm_for_voice`), not just during an action; Esc
  cancels the action, stops speech and interrupts the turn
  (`ControlEvent::EmergencyStop` → `Input::Esc`).
- **Models are settings, never in git**: pinned to a Hugging Face commit with
  a size and SHA-256 per file (`voice/models.rs`), downloaded on first use to
  `<data dir>/models/<id>`, verified on every load. Moonshine v2 Small
  streaming EN (MIT, 165.5 MB) and Phonon 31m EN (Apache-2.0, 65.1 MB).
- **Phonon voices** (Tony's request): `voice/phonon_voices.rs` pins the 37
  English voices of huggingface.co/gradium/phonon-voices@da6c9ec
  (Apache-2.0, 256 KB each, size + SHA-256 from the Hub's LFS ids). Each
  is a one-file `ModelSpec`, so the model downloader fetches and checks it
  into `<models>/phonon-voices/`. Settings downloads a voice when the user
  picks it and has Preview; the voice loop never downloads one and falls
  back to the default voice if the file is missing or damaged.
- **Parakeet** (optional STT, Tony's request): Parakeet TDT 0.6B v3 int8
  (NVIDIA, CC BY 4.0, ONNX by istupakov@8f23f0c, 4 files, 670.6 MB),
  through transcribe-rs's `onnx::parakeet`. `engines::load_stt` picks the
  engine by the files in the folder (`nemo128.onnx` = Parakeet). Measured on
  the test phrase: Moonshine "Open text edit n type hello", 108 ms, 0.7 GB
  peak; Parakeet "Open text edit and type hello.", 80 ms, 1.9 GB peak, 1.7 s
  load (with the hash check). Moonshine stays the default.
- **Clicky features** (Tony's request, after reading farzaa/clicky and
  jasonkneen/openclicky, both MIT; ideas only, no code copied):
  - Companion cursor (`voice.companion`): while a voice request is in progress, the app
    sends `pindot:companion {x, y, level}` (pointer from
    `platform::pointer_location`, mic RMS from `audio::mic_level`) at
    ~60 Hz, changes only, until 2.5 s after idle. The AI cursor springs
    beside the user's pointer, the tag shows a waveform (listening) or
    dots (thinking), it flies off for an action, holds 1.5 s on the target
    (4 s after `point`), then flies home on an arc.
  - Captions (`voice.captions`): `SpeakerEvent::FirstAudio` carries the
    text and `SpeakerEvent::Generated` the audio's real length (generation
    runs ahead of playback); the overlay reveals whole words in proportion
    to playback time, 0.12 s ahead (14 chars/s until the length is known;
    Phonon speaks about 13). Beside the cursor, or above the voice pill.
    Letter-by-letter typing felt behind the voice (Tony).
  - Tap-to-talk (`voice.tap_to_talk`, in `Machine`): a press under 300 ms
    listens hands-free; the next press sends; 60 s cap; Esc cancels.
  - Do or teach, decided per request (Tony: "no checkboxes, the best of
    both worlds"): the voice prompt says a command is done, while "teach
    me", "show me how" or "how do I..." means point and explain one step
    at a time without clicking or typing. A "Show me" setting existed
    briefly and was removed; an old `assist` field is ignored.
  - `--demo` starts with a companion scene (made-up pointer, nothing real
    moves).
- **Test path** (off by default, loud banner): `--test-voice WAV` replaces
  the mic with a WAV and presses Fn synthetically through the real tap;
  `--test-voice-esc` adds an Esc mid-turn and a second request; replies go
  to a temp WAV, play at 20% volume and are transcribed back.
  `voice_check` covers devices, models, STT and TTS without `claude`.
- **Clean exit**: Ctrl+C / SIGTERM quit through Tauri's exit path (voice
  loop, tap, audio and `claude` closed, server stopped); a 15 s watchdog
  forces the exit if that hangs.

### Phase 3: app, settings, setup, release (see `docs/phase3/README.md`)

- **One main window** (`main`, Svelte) with Setup, Settings and Activity,
  opened from the tray menu (Setup…, Settings…, Activity…, a hold-to-talk
  switch, Quit). It opens by itself, without taking focus
  (`orderFrontRegardless`), when Setup is needed: `claude` missing or
  signed out, the server down, or Setup never finished.
- **Settings file**: `settings.json` (0600), `version: 1`. Phase 2's
  `voice-settings.json` is folded in on first load and renamed
  `voice-settings.migrated.json`; unknown fields are kept; an unreadable
  file is kept as a backup and the defaults are used; `PINDOT_*`
  environment variables override but are never saved. A port change
  applies after a restart; hotkey, model and speak-mode changes restart the
  voice loop at once.
- **Hotkey binding**: one to four modifier keys, matched exactly; any other
  key or an extra modifier cancels the hold (the user is typing a
  shortcut). Defaults: `Fn` on macOS, `Ctrl+Win` on Windows. The macOS tap
  reads left and right from the flagsChanged device bits. Only Fn is
  swallowed (so Globe never fires); other modifiers pass through to apps,
  because swallowing Cmd or Ctrl would break every shortcut. Fn is refused
  on Windows, Shift alone is refused, and a lone side-less Ctrl/Option/Cmd
  gets a caution. Record in Settings captures the chord inside our window.
- **Connect / Disconnect** run `claude mcp add --transport http --scope user
  desktop http://127.0.0.1:<port>/mcp --header "Authorization: Bearer
  <token>"` and `claude mcp remove --scope user desktop`, only when the
  user presses the button. Built and unit-tested (argument list, quoting,
  token masking, status parsing); never run on the dev machine because they
  change the shared user config. The status is read from `~/.claude.json`
  without writing; the `mcpServers` shape it expects is assumed from the
  docs, not seen from a real `claude mcp add`.
- **Token regenerate**: a new token is written 0600, the running server
  swaps it at once (`SharedToken`), the config files are rewritten, and
  the voice brain restarts with the new one. The old token gets 401.
- **Model downloader**: per-file pins (size + SHA-256), `.part` files
  resumed with HTTP Range (206); a checksum mismatch deletes the `.part`
  and retries from zero (never resumes onto bad bytes), with backoff; when
  the retries run out, the fallback. Moonshine's fallback is Handy's
  archive on blob.handy.computer: a different (older) build with its own
  pins, unpacked into the same folder. Phonon has no mirror. The license
  text is shown before the download. "Check files" re-hashes everything
  and repairs a damaged file. The status line uses sizes only (fast); every
  load and "Check files" hash in full.
- **Permissions**: the Setup screen only reads (`AXIsProcessTrusted`,
  `CGPreflightListenEventAccess`, `CGPreflightScreenCaptureAccess`,
  `authorizationStatusForMediaType`). The request calls run only from
  "Ask macOS". The voice loop does not start while Microphone,
  Accessibility or Input Monitoring is missing, because creating a tap or
  opening the mic without a grant can itself show a prompt. "Open System
  Settings" opens `x-apple.systempreferences:com.apple.preference.security?Privacy_<Pane>`.
- **Test-only switches are compile-time gated** (Tony's rule): the
  `test-switches` cargo feature (app feature `test-switches` enables
  `pindot-core/test-switches`). Without it, `PINDOT_TEST_AUTO_APPROVE`
  is ignored with a warning, and `--test-turn`, `--test-voice`,
  `--test-voice-esc`, `--test-esc-after`, `--accept-trust`,
  `--permission-mode`, `--turn-timeout` and `--keep-running` print
  "WARNING: ... ignored: this build has no test switches". `npx tauri
  build` and plain `cargo build` never enable it. A test build prints
  "!!! TEST BUILD" at start and shows a banner in the main window. The
  settings UI has no control for any of them. `--capturable` and `--demo`
  are not test switches (they weaken nothing) but log a NOTE line.
- **Kept from earlier phases** (Nico's brief): the interactive brain on the
  user's `claude login`; `--setting-sources project,local`, our own session
  folder and `--permission-mode default`; `--restricted` not adopted (open
  question in the README); human approvals; the shared session for clients
  without our header is stated in the README guardrails.

### After Phase 3: the rename and the assistant

- **Rename to Pindot** (Tony's request). The data folder moves once:
  `paths::data_dir` renames `.../Deskhand` to `.../Pindot` when the new one
  doesn't exist yet (settings, token, models, activity log, session folder),
  so nothing is downloaded or set up again. The new bundle id means macOS
  asks for the four permissions once more; `tccutil reset All
  dev.deskhand.app` clears the old entries. `claude` sees a new session
  folder, so the voice brain answers its folder-trust dialog once more and
  old voice sessions can't be resumed. A `claude mcp add` registration keeps
  working: same server name, port and token.
- **Assistant name and talk style** (Tony's request; default name Rusty).
  `voice.assistant_name` (1-24 characters, one line), `voice.talk_style`
  (friendly, brief, professional, playful) and `voice.talk_notes` (the
  user's own words, up to 300 characters) in `settings.json`.
  `voice::voice_system_prompt` builds the prompt: "Your name is X.", the
  rules, the style's line, then the notes last, framed as tone and wording
  only so they never loosen a rule. A change restarts the voice loop. The
  name also goes to the overlay banner ("Rusty is controlling input"): each
  overlay page emits `pindot:overlay-ready` once it listens, and the app
  answers with `pindot:assistant` (and sends it again on save), so the
  overlay still only listens and emits, with no command. Preview says
  "Hi, I'm X." A blank or broken name in a hand-edited file goes back to
  Rusty; long notes are cut.

### Windows: first build and run (15 Nov 2026)

Done on a Windows 11 PC (two 1920x1080 monitors at 100%, claude 2.1.323
then 2.1.325). What was run and seen is in
`docs/windows-test-checklist.md`.

- **Linking ONNX Runtime**: `ort-sys` 2.0.0-rc.13's prebuilt static library
  (ONNX Runtime 1.28.0) needs the STL of a recent Visual Studio 2022. With
  only VS 2019 Build Tools (MSVC 14.29) the link fails with about 40
  unresolved `__std_find_trivial_*`, `__std_search_*`, `__std_remove_*`
  symbols, all in `libort_sys`. Without changing the repo: unpack
  Microsoft's `onnxruntime-win-x64-1.28.0.zip` and build with
  `ORT_LIB_LOCATION=<its lib folder>` and `ORT_PREFER_DYNAMIC_LINK=1`; the
  build script then links `onnxruntime.dll` and copies it next to the exe.
  Which of the two a Windows release ships is an open decision.
- **Microphone**: Windows has no per-app grant for a desktop app and never
  prompts. Three switches decide (Settings > Privacy & security >
  Microphone): "Microphone access" (HKLM), "Let apps access your
  microphone" (HKCU) and "Let desktop apps access your microphone" (HKCU,
  `NonPackaged`), each a `Value` of `Allow` or `Deny` under
  `...\CapabilityAccessManager\ConsentStore\microphone`. An app is listed
  there only after it has used the microphone. `permission_report` reads
  the three: any `Deny` is Denied (with a note naming the switch), a missing
  value counts as on, an unreadable one is Unknown. Only Denied or
  Restricted keeps hold-to-talk from starting on Windows
  (`windows_logic::blocks_voice`): nothing prompts, so trying costs
  nothing. Before this the report was always Unknown, which blocked voice
  for good, and "Ask Windows" did nothing; the Setup screen has no Ask
  button on Windows now.
- **Hold-to-talk and Esc**: one `WH_KEYBOARD_LL` hook per listener, on its
  own thread with a message loop (`platform/windows/keyhook.rs`). The
  hotkey swallows nothing. When a chord with Win or Alt starts it presses
  the unassigned key `0xE8` once (AutoHotkey's mask key), so letting go
  opens neither the Start menu nor a window's menu bar. AltGr's made-up
  Left Ctrl (scan code `0x21D`) is ignored, so Right Alt works on AltGr
  layouts. The Esc stop returns 1 for a real Esc and reports one press per
  hold. Keys Pindot injects carry `INPUT_MARKER` (`0x50494E44`) in
  `dwExtraInfo` and are ignored by both; the Windows `Input` must stamp its
  keys with it. A hook in a normal process does not see keys typed into an
  elevated window.
- **The hook heals itself**: Windows removes a low-level hook it finds
  slow, with no notice, and one voice loop came up with a hook that was
  never called (its thread used no CPU cycles while keys were pressed). A
  single 1.5 s stall does not get a hook removed on this build; what did
  was not found. Every 500 ms the hook thread now corrects the tracked
  modifiers from `GetAsyncKeyState` (`WinChord::resync`) and takes the hook
  out and puts it back, unless a hold is in progress or a key was pressed
  in the last 150 ms. It logs when Windows had removed the hook, and each
  hold-to-talk Down/Up/Cancel is logged. A missed key-up (lock screen, UAC
  prompt, an administrator's window) no longer leaves a modifier "held".
- **Desktop control** (`capture`, `apps`, `input`; all raw Win32 through
  `windows-sys`, no xcap or enigo):
  - **A point is a physical pixel**, on every monitor, and each display's
    scale is reported as 1 (`windows_logic::display_info`). The first rule
    divided each monitor by its own scale, as the overlay did: a 150%
    monitor to the right of a 100% one then began at 1280 "points", inside
    the first one's 0..1920, and a click meant for one could land on the
    other. The overlay page works in CSS pixels and is told how many
    points make one on its monitor (`MonitorRect::unit`: 1 on macOS, the
    monitor's scale on Windows); its window is placed in pixels. Only 100%
    monitors were tried: there nothing changed, and the scaled case is
    covered by unit tests alone.
  - Screenshots are one GDI `BitBlt` per monitor; our own windows are left
    out by the compositor (`WDA_EXCLUDEFROMCAPTURE`).
  - The window list is `EnumWindows`, front to back. The desktop
    (`Progman`, `WorkerW`) is skipped. The taskbar is listed as "Taskbar"
    with pid -1 and layer 20: no app owns it, so it can never be approved,
    even when File Explorer (the same process) is. Click-through windows
    (`WS_EX_TRANSPARENT | WS_EX_LAYERED`: our overlay, a game's, a
    recorder's) get alpha 0, so the hit test and the mask look through
    them. Before that rule every click would have been refused as "on a
    window of" whichever overlay is on top. A store app's frame
    (`ApplicationFrameWindow`) is traced to the process that owns its
    `Windows.UI.Core.CoreWindow`.
  - **One name per app**, running or not, so one approval covers both: the
    Start menu's name when a Start entry names the executable or the
    process's application id (`start_name_for`), else the executable's
    file description, else its file name. `bundle_id` is the executable's
    name in lower case. Pindot's own process is always "Pindot". Names
    containing "pindot" in the executable can never be approved
    (`is_pindot`), which a test window called `pindot-test-target` found.
  - The Start menu's list is `Get-StartApps` through a hidden PowerShell
    (about a second, read once, kept ten minutes, read again when a name
    is not found). An app opens with `explorer.exe shell:AppsFolder\<id>`;
    one that is running is brought forward instead. A name matches exactly,
    or as whole words when only one app fits ("Chrome").
  - Bringing a window forward: `AttachThreadInput` to the foreground
    thread, then `SetForegroundWindow`; if Windows still refuses, one Alt
    tap and again.
  - Input is `SendInput` with `INPUT_MARKER` on every event. Text goes as
    Unicode (accents, CJK and emoji arrive intact). In a shortcut **`cmd`
    is pressed as Ctrl** (a Mac shortcut such as cmd+s means ctrl+s here)
    and the new `win` modifier is the Windows key; the tool descriptions
    keep their Mac examples and `desktop_context` says how Windows differs.
  - **The element tree** (`uia`, the `windows` crate's COM bindings): a
    window is walked one level at a time; each `FindAllBuildCache` call
    fetches a node's children with the shown properties cached, so a node
    costs one trip to the other process and the walk can stop at the
    limits between trips (a whole-subtree cache could not, and a browser's
    tree is huge). Only the control view is walked. Roles become the macOS
    names `server::tree` is written in (`uia_role`; a test ties the
    mapping to `tree::is_interactive`). A password's value is never read.
    Worker threads join the multithreaded apartment before their first
    call; element handles are shared between them.
  - **Pressing an element** does what one click would
    (`windows_logic::press_action`): a tab or a radio button is selected;
    a row that offers only Select is selected (Settings' navigation list,
    where that navigates); anything else is Invoke, else Toggle, Select,
    Expand/Collapse, else the legacy default action. A row that offers
    Invoke as well as Select gets no press and is clicked for real, as
    every row is on macOS: on a file in File Explorer a click selects and
    Invoke opens (a test's press opened the file, and would have run a
    program), on a tile of Word's start page a click opens and Select only
    marks, and nothing tells the two apart. A press runs on a helper
    thread given two seconds (Invoke can block while the dialog a button
    opens is up).
  - **Direction marks are left out** of names and values
    (`windows_logic::visible_text`): File Explorer writes a date with
    U+200E and U+200F between its parts, which `find("15 Nov 2026")` did
    not match.
  - **Password fields**: `focused_is_secure` asks UI Automation's
    `IsPassword` on the focused element, and keeps the classic check (an
    Edit control with `ES_PASSWORD`) in case UI Automation fails. Tried on
    a classic password box only, not in a browser.
  - **A browser's page is not in the tree until asked for.** Chromium and
    Firefox build a page's accessibility tree only once a client asks, so
    the first walk of such a window found the browser's frame and no page
    (Edge: 6 elements, then 11 a second later). `uia::window_tree` walks a
    browser window again, up to four times 350 ms apart, until a document
    has content (`web_tree_pending`); a window whose page never comes is
    not waited for again for 30 s. The first read of a fresh Edge window
    now lists the page (0.45 s).
  - **A dialog is seen twice**: Windows shows an owned dialog as its own
    window and again inside its owner's tree, so `find` listed "Don't
    save" twice. `find` keeps the first of two hits with the same role,
    name and place in one app (`tree::same_element`).
  - **An administrator's window**: Windows drops input a normal process
    sends to it, and `SendInput` still reports success. Before a click,
    scroll, drag, key or text, the process that owns the target is checked
    (its token's elevation; a process we may not even open counts too)
    and the tool gets a clear error with nothing sent
    (`input_is_blocked`). `spike_hotkey status` lists such windows.
  - **A click at x,y presses the control under it** when a click presses
    that kind of control (`actionable_at`, `click_presses`: buttons,
    links, checkboxes, radio buttons, menu items; the element at the point
    or one of its three nearest ancestors), so the user's pointer stays
    put, as on macOS. A control can report a press that did nothing, so a
    second click at the same place within 30 s is a real one, and stays
    one while the clicks keep coming (`LastPress`); `desktop_context` tells
    Claude.
  - **A right-click asks the control for its context menu**
    (`IUIAutomationElement3::ShowContextMenu`, the counterpart of
    `AXShowMenu`), so the pointer stays put, but only where that was seen
    to bring the menu up (`shows_menu_on_request`: File Explorer's lists,
    web content, XAML controls) and the keyboard can reach the control. A
    classic Win32 or WinForms control answers "done" and shows nothing, so
    there, and in any framework not tried, the mouse right-clicks. At x,y
    it is asked only of a whole item (a row, a tab, a button, a link:
    `menu_shows_at`); in a text field or on a canvas the place matters, so
    the mouse right-clicks. The second-click rule holds here too.
  - **A closed menu's items are listed** for a classic menu bar (an
    `HMENU`: Win32, MFC, WinForms `MainMenu`), with their path, shortcut,
    and whether they are greyed out or checked, as macOS lists them. One
    is pressed by posting its command to the window (`MenuCommand`). The
    menu would refuse a greyed-out item and a posted command would not,
    so the item's state is read first (`PlatformError::Disabled`; the
    server does not open the menu to try again). Menus that are not an
    `HMENU` (ribbons, WinUI and web menus) show their items only while
    open.
  - Seen: every tool by hand against a test window (by x,y and by ref);
    two real turns (`--test-turn`, haiku): from screenshots, "hello"
    typed and a button pressed in 14.6 s with two screenshots; with the
    tree, text typed and the button pressed twice in 18.1 s with none.
    Edge with its own profile on a local page: a field typed into, a
    checkbox, a button and a link pressed by ref, and typing into the
    page's password field refused. Paint: opened from the Start menu,
    104 controls in 0.46 s, the pencil picked by ref, two lines drawn by
    drag where they were aimed, Alt+F4, "Don't save" pressed. And the
    whole loop: a synthesised spoken request through `--test-voice`
    (Parakeet, haiku) opened a test window's app, typed and pressed, and
    said so: RESULT PASS, 14.7 s from key release to the first sound.
    Later, by hand through the tools: Settings (a store app whose frame is
    an `ApplicationFrameWindow`) opened by name, 39 controls in 0.2 s, a
    navigation row pressed by ref and its page read, Alt+F4. File Explorer
    on a test folder: 70 controls in 0.17 s, with each file's name, date,
    type and size, the folders of the side pane and the toolbar with what
    is disabled; a file selected by ref (Cut, Copy, Rename and Delete came
    alive); a double-click on a row that another app's window covered was
    refused, rightly. Edge on example.com: the page listed on the first
    read (0.48 s), its link pressed by ref, the next page read (74
    controls). Calculator is not installed on the test PC and `open_app`
    said so. Word: opened by name, a blank document made from its start
    page, 126 controls in 0.38 s with the ribbon, text typed, Bold pressed
    by ref, Alt+F4 and "Don't Save"; nothing saved. In the full app with
    the overlay up: a click at x,y on a button pressed it and the pointer
    stayed where it was, the same x,y again was a real click, and the
    items of a test window's classic menu were listed and run by ref (a
    greyed-out one refused).
- **The overlay is a tool window** titled "Pindot cursor". The window
  library makes it an "app window", which Windows forces into Alt+Tab and
  the taskbar; `platform::windows::window::hide_from_switcher` changes
  that after the window is built (`overlay_ex_style`).
- **No console windows.** A console program started by a windowed app gets
  a new, visible console on Windows, so `claude --version`, `claude auth
  status` (at start, on every Setup refresh and voice restart) and `claude
  mcp add/remove` each flashed a black window in a release build. They
  start with `CREATE_NO_WINDOW` now (`brain::hidden`). Launching from a
  terminal hides the flash, so test a release build from Explorer. Seen:
  the release build started through `explorer.exe`, every new visible
  window watched for 25 s (start, the two `claude` checks, the voice
  brain): only Pindot's own windows appeared.
- **Held while off**: when hold-to-talk is off for a reason (a model is
  missing, Setup is not finished) nothing listened for the key, so holding
  it did nothing at all, which read as "the hotkey is broken". On Windows a
  key hook now runs in that state only to put the reason on the overlay's
  pill ("Hold-to-talk is off: ..."). Not when the user switched
  hold-to-talk off, and not on macOS, where creating a tap without the
  permissions can itself show a prompt.
- **One instance**: starting Pindot twice would give two overlays, two
  tray icons and a copy without its port. macOS brings the running app
  forward by itself; on Windows the first instance holds a named mutex and
  waits on a named event, and a second start signals the event, passes on
  its right to the foreground (`AllowSetForegroundWindow`) and ends
  (`platform/windows/instance.rs`). The names come from the data folder
  (`windows_logic::instance_name`; FNV-1a, so every build agrees), so a
  test instance with its own `PINDOT_DATA_DIR` still runs beside the real
  one. Not for `--demo`, a start without the server, or the test paths.
- **A minimized main window is restored** when it is asked for (the tray
  menu, a second start): `show()` and `set_focus()` leave it minimized on
  Windows, so nothing appeared.
- **`scripts\build-windows.ps1`** is the Windows `build-mac.sh`: the
  release app and a per-user NSIS installer, with ONNX Runtime linked as a
  DLL and shipped next to `pindot.exe` (decided: the static library needs a
  recent VS 2022, the DLL links anywhere). The DLL reaches the bundler
  through `--config`, not `tauri.conf.json`, because a resource listed
  there must exist for every `cargo build` on every OS. See
  `docs/release.md`, which also says what a silent install, a start of
  the installed copy and a silent uninstall showed. The installer carries
  Microsoft's C++ runtime too (four DLLs, 14.40 or later, the newest on
  the build PC): `onnxruntime.dll` needs it and a fresh PC may not have
  it, so Pindot would not start there. Two instances that ask
  WebView2 for different options
  (one with a remote-debugging port) cannot share the app's WebView2 data
  folder: the second fails at startup.
- **Finding `claude`**: `claude.exe` on the PATH, then under the home
  folder's usual places (a windowed app may get a shorter PATH). An npm
  install is a `claude.cmd` script: Pindot cannot start a script in its
  terminal, and `cmd /c` would hand the prompt to cmd's quoting rules, so
  Setup says to install the native app instead of "not installed".
- **Checked without the user's pointer or keys**: the Esc stop key
  (`spike_hotkey esc` with a test window in front and unmarked Esc presses:
  armed, both reported and none reached the window; disarmed, the press
  reached it); the approval window in a release build (answered with Deny
  over CDP, never Allow); the tray icon and its menu (UI Automation finds
  the icon in the hidden-icons flyout and presses it; the menu is read
  with `MN_GETHMENU` because UI Automation shows it empty; "Quit Pindot"
  exited cleanly).
- **The pseudo-console asks a question first**: portable-pty 0.9 opens
  ConPTY with `PSEUDOCONSOLE_INHERIT_CURSOR`, so the first thing it sends
  is `ESC[6n` (where is the cursor?), and it passes on nothing `claude`
  prints until the host answers. Unanswered, the session looked "ready"
  after 2 s with 100 bytes of output and every prompt timed out. The pty
  reader now answers `ESC[1;1R`, on Windows only (a Unix pty never asks,
  and there a query would be `claude`'s own).
- **Quitting `claude`**: with `--disable-slash-commands`, `/exit` gets
  "Unknown command: /exit" and `claude` stays, so every exit waited out its
  grace and killed it. `ClaudeSession::exit` now presses Ctrl+C until
  `claude` goes ("Press Ctrl-C again to exit"), up to four times 400 ms
  apart, then kills after the grace. 1.3 s on Windows. Not re-run on macOS.
- **Folder trust and transcripts** work as on macOS: the dialog is drawn
  the same, and the transcript lands in
  `%USERPROFILE%\.claude\projects\C--Users-<name>-AppData-Roaming-Pindot-session\`.
  `find_transcript` searches by session id, so the slug is never built.
- **Overlay**: transparent, click-through, topmost and non-activating on
  both monitors, and absent from screen captures by default
  (`WDA_EXCLUDEFROMCAPTURE`). The overlay windows are titled "Tauri App"
  and have `WS_EX_APPWINDOW`, so they may show in Alt+Tab: not checked.
- **Driving the app in tests**: `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=
  --remote-debugging-port=<port>` exposes the main window and each overlay
  page over CDP, so a test can click and screenshot Pindot's own pages
  without moving the user's pointer. Never set it in normal use.
- **Speed**: a debug build takes 11.5 s to the first spoken audio (ptts
  unoptimised); a release build 0.75 s. Test voice in release builds only.
- **The speech model is warmed up when it loads** (`PhononTts::load`
  speaks one throwaway chunk; nothing is played). The 0.75 s was a cold
  start: the first utterance of a process took about 0.5 s to begin and
  later ones 66 ms (the Mac's figures: 124 ms, then 43 ms). Now the first
  reply starts in 35 to 70 ms and loading takes 0.6 s instead of 0.1 s.
  `voice_check tts` prints the first audio of a second utterance too. The
  change is in shared code and was not run on macOS.
- **Measured against the Mac's figures** (`docs/phase3`), release builds: a
  screenshot 66 ms median (Mac 51 back to back, 86 spaced), `read_window`
  30 ms and a press by ref 16 ms on a small window, `claude` ready in
  2.5 s (Mac 2.1 s). See `docs/windows-test-checklist.md`.

## Known risks (open)

- **Settings isolation leans on flags.** Skills, Claude in Chrome and
  CLAUDE.md are now off by flag and env var (see "Token budget"); the old
  "MCP servers: 1 connected" with an empty strict config was most likely
  Claude in Chrome. `CLAUDE_CODE_DISABLE_CLAUDE_MDS` is undocumented and
  may change: re-check the baseline on each claude update.
- **Moonshine source**: the pinned `quantized_26_07_30` folder is the last
  one with a single `frontend.ort`, which transcribe-rs 0.3.12 needs. If
  Moonshine AI prunes it, the pinned commit URL should keep working unless
  the repo history is rewritten; a fallback mirror is `blob.handy.computer`.
- **STT accuracy**: the test phrase comes out as "Open text edit n type
  hello"; Claude copes, but short words get lost.
- **The real microphone was never opened** in tests (only the WAV path), nor
  the physical Fn key or real speech.
- **ort downloads ONNX Runtime (~148 MB) at build time** (not at run time).
- The physical Fn/Globe key, the physical Esc and multi-monitor overlay
  have only been tested with synthetic events and one display. The approval
  window's buttons have not been clicked by a human yet. See
  `docs/phase1/README.md` for the manual steps.
- Stateless MCP clients that Pindot did not start and that send no
  `X-Pindot-Session` header all share one session; their approvals last
  until Pindot quits. Fine for one external client, wrong for two.
- Submitting a prompt through the PTY: Claude Code takes fast input as a
  paste and can drop an Enter that arrives during it. `type_prompt` waits
  for the echo and quiet; `drive_turn` presses Enter once more if the
  `UserPromptSubmit` hook has not come in 6 s. `exit` types nothing: it
  presses Ctrl+C until `claude` quits (see "Quitting `claude`").
- Screenshots in a debug build take about 1.5 s (unoptimised mask and
  resize); one Phase 1 run saw 10-36 s spikes that did not come back. The
  release build takes 54 ms median, 91 ms p95, 94 ms max over 70 shots
  (capture 31-54 ms, mask 1-5 ms, resize 9-18 ms), no spikes; details in
  `docs/phase3/README.md`. Measure screenshot speed in release builds only.
- **Build with `scripts/build-mac.sh`**, not plain `npx tauri build`: it
  signs with the Developer ID (or Apple Development) identity found in the
  keychain (`APPLE_SIGNING_IDENTITY` overrides), with the hardened runtime
  and `Entitlements.plist` (microphone). The designated requirement is
  then the bundle id plus the team id, so macOS keeps the four permission
  grants across rebuilds. An ad-hoc build changes signature every time
  and loses them; `tccutil reset All dev.pindot.app` clears stale
  entries. The identity is never written into the repo.
- **macOS 26 menu bar**: status items are drawn by Control Center, not by
  our process, and move when the frontmost app changes. Tests must re-read
  the tray position and confirm our menu window is open before clicking.
- **Windows**: `find_claude` looks only for `claude.exe`; an npm install
  gives `claude.cmd` (see the checklist).
- **Windows, tried on few apps**: a test window, Edge (a local page and
  one public site), Paint, Settings, File Explorer on a test folder and
  Word on a blank document. Not a chat or mail app, or a heavy web app.
  The items of a closed menu are listed only for a classic menu bar.
- **Windows, checked by hand** on 16 Nov 2026 by the PC's owner, in the
  release build: hold-to-talk with the physical Ctrl+Win and the real
  microphone (20 holds), "Allow for this session" in the approval window,
  Paint opened and drawn in on request, the physical Esc stopping a turn
  (interrupted in 100 ms), and the tray menu. Speech began 110 to 230 ms
  after its text.
- **Windows, not checked**: a monitor above 100%
  (the mapping is unit-tested, not run), sleep and wake (nothing is held
  open across it: the microphone and the speaker are opened for each use,
  and the key hook is put back every half second), a click refused on an administrator's window (the check
  flags such windows; no click was aimed at one), a window that is
  invisible but not click-through (it would block clicks under it), and
  a hand on the installer (its pages and the uninstaller's were walked by
  sending their buttons' commands, see `docs/release.md`). The token and
  config files get no ACL (they rely on `%APPDATA%` being per-user).
- **Shared code changed on Windows was not run on a Mac**: quitting
  `claude` with Ctrl+C, the `win` modifier, `PlatformError::Disabled`, the
  speech warm-up and the overlay's `unit`. `scripts\check-macos.ps1`
  type-checks both crates for `aarch64-apple-darwin` from Windows, the
  macOS-only files included, with stand-ins for the C compiler and the
  archiver (the mirror of `check-windows.sh`). It passes, which proves
  that the Rust compiles and nothing more.
- `scripts/check-windows.sh` type-checks `pindot-core` and the app for
  `x86_64-pc-windows-msvc` from the Mac, with stand-ins for the C compiler,
  archiver and `llvm-rc` (ring and the icon resource need Windows headers),
  so it proves only that the Rust compiles. Re-run it after changes to
  Windows paths. See `docs/windows-test-checklist.md`.

## Conventions

- Rust 2024 edition, workspace dependencies in the root `Cargo.toml`, and
  `anyhow` in binaries with `thiserror` (`PlatformError`) in traits.
- Before adding or upgrading a crate, read its current source or docs in
  `~/.cargo/registry/src`. Don't trust memory for rmcp, tauri-nspanel,
  portable-pty or `claude` flags.
- Send long command output to a log and read the tail:
  `cargo build > /tmp/pindot-build.log 2>&1; tail -30 /tmp/pindot-build.log`.
- Tests may only click or type inside windows the test opened itself (our
  Swift test window, or a TextEdit document the test created and discards
  unsaved). Never in an app the user already has open. Kill every spike,
  tap, overlay and child `claude` when done.
- Keep nested `claude` test turns few and one-line; they spend the user's
  subscription. Use `--model haiku` for tests.
- Commits: local only, as Tony Herrera <tony@opusautomations.com>, ending with
  a `Co-Authored-By:` trailer for the model that wrote the change. No remote
  until Tony says so.
