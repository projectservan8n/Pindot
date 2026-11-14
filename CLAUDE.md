# Deskhand

Open-source (MIT) desktop app for macOS and Windows. Deskhand gives the user's
own Claude Code CLI eyes and hands on the desktop through MCP tools, and the
user drives it by voice. The product name must never contain "Claude".

The spec is the build brief, kept outside this repo. This file records the
decisions and conventions that came out of building it. Phase 0 (spikes) and
Phase 1 (the tool set, the accessibility tree and the guardrails, macOS only)
are done; the evidence is in `docs/phase0/` and `docs/phase1/`.

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
  per-user data dir (`~/Library/Application Support/Deskhand/`, dir 0700,
  files 0600) with the generated `claude-mcp.json` and `claude-settings.json`,
  never in the repo. Model weights are downloaded at runtime, never committed.
- Reference code (Tarsee's `claude-interactive-pool.js` and
  `transcript-tailer.js`) is private and read-only: reimplement it, never copy
  it. Handy (cjpais/Handy) and handy-keys are MIT. Nothing was copied from
  them; our Fn tap is our own code.

## Layout

- `crates/deskhand-core`: everything testable without a UI.
  - `platform/`: OS traits `Capture`, `Input`, `Hotkey`, `StopKey` (global
    Esc), `Accessibility` and `Overlay`. Each has a macOS implementation (`platform/macos`), Windows
    stubs that return `Unsupported` (`platform/windows`) and mocks
    (`platform/mock.rs`).
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
  - `coords.rs`: maps screenshot pixels to global points. `paths.rs`: the data
    dir, the token and its constant-time compare.
  - `src/bin/spike_*.rs`: Phase 0 spikes.
- `app/src-tauri`: the Tauri 2 app: `overlay.rs` (NSPanel AI cursor and
  "controlling input" banner), `approval.rs` (the approval window, answered
  by a human), `test_turn.rs` (`--test-turn`, one real `claude` turn for the
  done-when check) and `lib.rs`. `app/ui`: overlay, approval and activity
  pages (plain HTML/JS).
- `spikes/test-target`: a Swift test window for the Phase 0 spikes.
  `spikes/mcp-call.py` is the Phase 0 one-shot MCP client;
  `scripts/mcp_call.py` keeps one MCP session across calls (`init`, `call
  TOOL 'json'`, `list`, `end`) for manual tool tests.

## Decisions and why

- **MCP server**: rmcp 3.10 `StreamableHttpService` behind our own axum guard.
  It binds 127.0.0.1 only, on default port 47823 (`DESKHAND_PORT` overrides
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
  Each panel receives the same `deskhand:cursor` events in global points and
  draws the cursor only while it is over its own monitor.
  `NSWindowSharingType::None` keeps the cursor out of every screenshot,
  including ours. Verified on macOS 26.5 with `screencapture` and xcap; Apple
  has changed this behaviour before, so re-check it on every macOS major
  release. `--capturable` turns it off for tests.
- **Fn hotkey**: a CGEventTap at the session level on `flagsChanged`, keycode
  0x3F, `MaskSecondaryFn`. It returns NULL to swallow Fn so the Globe action
  never fires. If the tap is disabled by timeout or user input, it re-enables
  itself. Synthetic test events carry the user-data marker 0x0DE5_0F11.
- **Overlay UI is plain HTML/JS** for now. The brief's Svelte + TypeScript UI
  starts with the tray and settings windows (Phase 3); the overlay page has no
  framework need.

- **Tools (Phase 1)**: `desktop_context`, `request_access`, `open_app`,
  `focus_window`, `read_window`, `find`, `screenshot`, `zoom`, `click`,
  `type_text`, `key`, `scroll`, `drag`, `wait`, `point`. Descriptions steer
  to refs first (`read_window`/`find`) and screenshots last. Every result
  that carries on-screen text says it is untrusted data. Descriptions tell
  Claude to ask the user before destructive actions, purchases, sending
  messages or typing credentials; `type_text` refuses a focused secure field.
- **Approval is answered by a human in our UI** (Tony's decision). Each app
  needs approval once per MCP session. The window shows the apps, extra red
  warnings for shells/IDEs, file managers, system settings and password
  managers, and Claude's reason labelled "not verified". Buttons: Deny and
  "Allow for this session"; Esc denies, Enter does nothing, closing denies,
  no answer in 120 s denies. Synthesising a click on Allow is not allowed,
  even in tests (an automated agent was blocked from doing it, rightly).
- **`DESKHAND_TEST_AUTO_APPROVE=TextEdit` is test-only** (Tony's decision):
  off by default, prints a loud warning at start and on every use, and is
  never read in the normal path.
- **The gate**: actions only touch approved apps. Refs carry their app;
  `click`/`scroll`/`drag` at x,y run a hit test on the window list first
  (Deskhand's own prompt/activity windows block, the menu bar needs an
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
  after `request_access` approved). Every `claude` Deskhand starts gets an
  `X-Deskhand-Session: <its --session-id>` header in its MCP config
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
  5 MB), one line per tool call; `deskhand --activity` opens a live view.
- **Permission mode for nested sessions**: `--permission-mode default` plus
  `--allowedTools mcp__desktop` (Tony's decision). claude 2.1.325's help
  lists acceptEdits, auto, bypassPermissions, manual, dontAsk and plan, but
  accepts `default` silently and shows "manual mode on". Undocumented: if it
  stops being accepted, switch to `manual` and tell Tony.
- **Sheets**: a Save sheet has its own CGWindowID but no AX window; the AX
  lookup falls back to the document window that holds the sheet. Right
  after `cmd+w` the sheet is still sliding in (its AX frame isn't the CG one
  yet, or it isn't in the tree yet), so an overlapping sheet counts and
  `window_tree` retries the lookup 3 times, 150 ms apart.

## Known risks (open)

- **User settings leak into Deskhand sessions.** Tony's auto mode, effort level
  and so on showed up in the nested TUI. `--settings` adds to the user
  settings rather than replacing them. Options for Phase 2: pass
  `--setting-sources` and/or `--permission-mode`, chosen on purpose.
- The physical Fn/Globe key, the physical Esc and multi-monitor overlay
  have only been tested with synthetic events and one display. The approval
  window's buttons have not been clicked by a human yet. See
  `docs/phase1/README.md` for the manual steps.
- Stateless MCP clients that Deskhand did not start and that send no
  `X-Deskhand-Session` header all share one session; their approvals last
  until Deskhand quits. Fine for one external client, wrong for two.
- Submitting a prompt through the PTY: Claude Code takes fast input as a
  paste and can drop an Enter that arrives during it. `type_prompt` waits
  for the echo and quiet; `drive_turn` presses Enter once more if the
  `UserPromptSubmit` hook has not come in 6 s. `exit` sends Ctrl+C before
  `/exit` so it can never be appended to an unsent prompt.
- Screenshots in a debug build take about 1.5 s (mask + resize); one run
  saw 10-36 s spikes that did not come back. Measure in release.
- The Windows app crate is not compiled yet. `cargo check` for
  `x86_64-pc-windows-msvc` passes for `deskhand-core`, but `tauri-build` needs
  `llvm-rc` (or a Windows host) to embed the icon resource. See
  `docs/windows-test-checklist.md`.

## Conventions

- Rust 2024 edition, workspace dependencies in the root `Cargo.toml`, and
  `anyhow` in binaries with `thiserror` (`PlatformError`) in traits.
- Before adding or upgrading a crate, read its current source or docs in
  `~/.cargo/registry/src`. Don't trust memory for rmcp, tauri-nspanel,
  portable-pty or `claude` flags.
- Send long command output to a log and read the tail:
  `cargo build > /tmp/deskhand-build.log 2>&1; tail -30 /tmp/deskhand-build.log`.
- Tests may only click or type inside windows the test opened itself (our
  Swift test window, or a TextEdit document the test created and discards
  unsaved). Never in an app the user already has open. Kill every spike,
  tap, overlay and child `claude` when done.
- Keep nested `claude` test turns few and one-line; they spend the user's
  subscription. Use `--model haiku` for tests.
- Commits: local only, as Tony Herrera <tony@opusautomations.com>, ending with
  a `Co-Authored-By:` trailer for the model that wrote the change. No remote
  until Tony says so.
