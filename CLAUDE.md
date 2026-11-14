# Deskhand

Open-source (MIT) desktop app for macOS and Windows. Deskhand gives the user's
own Claude Code CLI eyes and hands on the desktop through MCP tools, and the
user drives it by voice. The product name must never contain "Claude".

The spec is the build brief, kept outside this repo. This file records the
decisions and conventions that came out of building it. Phase 0 (spikes) is
done; see `docs/phase0/` for the evidence.

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
  - `platform/`: OS traits `Capture`, `Input`, `Hotkey`, `Accessibility` and
    `Overlay`. Each has a macOS implementation (`platform/macos`), Windows
    stubs that return `Unsupported` (`platform/windows`) and mocks
    (`platform/mock.rs`).
  - `server/`: the rmcp streamable-HTTP MCP server named `desktop`. It also
    hosts the Stop-hook receiver.
  - `brain/`: the PTY session (`pty.rs`), config-file writers (`config.rs`)
    and transcript reading (`transcript.rs`).
  - `coords.rs`: maps screenshot pixels to global points. `paths.rs`: the data
    dir, the token and its constant-time compare.
  - `src/bin/spike_*.rs`: Phase 0 spikes.
- `app/src-tauri`: the Tauri 2 app, with `overlay.rs` (NSPanel AI cursor) and
  `lib.rs`. `app/ui`: the overlay page (plain HTML/JS).
- `spikes/test-target`: a Swift test window that is the only thing tests may
  click or type into. `spikes/mcp-call.py` is a hand-rolled MCP client.

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

## Known risks (open)

- **User settings leak into Deskhand sessions.** Tony's auto mode, effort level
  and so on showed up in the nested TUI. `--settings` adds to the user
  settings rather than replacing them. Options for Phase 2: pass
  `--setting-sources` and/or `--permission-mode`, chosen on purpose.
- The physical Fn/Globe key and multi-monitor overlay have only been tested
  with synthetic events and one display.
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
- Tests must only click or type inside our own test window
  (`spikes/test-target`). Kill every spike, tap, overlay and child `claude`
  when done.
- Keep nested `claude` test turns few and one-line; they spend the user's
  subscription. Use `--model haiku` for tests.
- Commits: local only, as Tony Herrera <tony@opusautomations.com>, ending with
  a `Co-Authored-By:` trailer for the model that wrote the change. No remote
  until Tony says so.
