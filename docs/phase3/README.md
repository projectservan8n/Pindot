# Phase 3 evidence: UI, settings, downloader, onboarding, docs, release build

Run on 14 Nov 2026 on Tony's Mac mini (macOS 26.5, Apple silicon, one
1920x1080 display), by an agent, with no keyboard or microphone use by a
person. "Tested" below means the real thing ran and the result was checked.
Everything else says "not tested, because ...".

Every screenshot here is of Deskhand's own window, taken by window id
(`screencapture -x -o -l<id>`) with `--capturable`, scaled to 640 px. No
other app's screen content was looked at; the full-screen images taken by
the timing run below were never written to disk.

## What works

| Item | State | Evidence |
|---|---|---|
| Main window (Setup, Settings, Activity) in Svelte + TS; overlay and approval stay plain HTML | Tested (debug and release build) | `setup-*.png`, `settings-*.png`, `activity-*.png`, `release-setup.png` |
| Tray icon and menu (status line, Setup, Settings, Activity, Hold-to-talk, Quit Deskhand) | Menu tested: icon and open menu render. Quit item not tested by click, see "Stray clicks" | `tray-menu.png` |
| Startup detection: `claude` found and signed in | Tested in the release build: real `claude` 2.1.325 "Signed in (claude.ai)"; a fake `claude` answering `{"loggedIn":false}` gives the "run `claude login`" box; no `claude` reachable (empty HOME and PATH) gives "not installed" | `release-setup.png`, `startup-not-signed-in.png`, `startup-claude-not-found.png` |
| Permission onboarding (Accessibility, Input Monitoring, Screen Recording, Microphone), read-only preflight | Detection and rendering tested: three show Allowed, Microphone "Not asked yet" (true on this Mac). No prompt was raised, no TCC change | `setup-claude-and-permissions.png` |
| "Open System Settings" buttons | URLs checked read-only: Launch Services maps all four to System Settings, and each anchor exists in the Privacy settings extension. Pressing them: not tested, because this run may not open System Settings | `settings-urls-check.txt` |
| "Ask macOS" (Microphone) | Not tested, because it raises a real system prompt | - |
| Hotkey rebinding | Tested. Record in Settings with synthetic Right Option and Ctrl+Cmd (events posted only to Deskhand's own pid); chord tap selftest 7/7 PASS (Right Option, Left Option no match, Ctrl+Cmd, Ctrl alone, Shift joining cancels, a key cancels, Fn swallowed) | `settings-record-right-option.png`, `settings-record-ctrl-cmd.png`, `hotkey-chords-selftest.txt` |
| Settings save (hotkey, models, port, speak mode, volume), file mode 600 | Tested: saved through the UI, reloaded, `settings.json` is `-rw-------`. A saved port (47826) is used by the release build at the next start: `/health` answered there and `claude-mcp.json` points to it | `settings-saved.png` |
| STT and TTS model choice | Tested as a setting (the lists hold one model each today) | `settings-saved.png` |
| Speak each message vs final reply only (default: each message) | The setting saves and loads (tested). Both behaviours are covered by voice state-machine unit tests; not heard through the real mic, see below | unit tests |
| Token regenerate | Tested: old token 401, new token 200 at once, file `-rw-------` | `settings-token-regenerated.png` |
| Connect / Disconnect to Claude Code | Built, not tested here, because it changes the shared user config. The exact command construction is unit tested. Manual step below | unit tests |
| Model downloader | Tested for real into an empty temp dir: license shown first, 230.5 MB in about 17 s with progress, 11/11 files match the pins by an independent `shasum`, one flipped byte caught and re-fetched by "Check files", resume from a 23 MB `.part` with HTTP 206 | `setup-license-before-download.png`, `setup-download-progress.png`, `setup-download-done.png`, `setup-models-check-files.png`, `setup-models-repaired.png`, `model-download.txt` |
| Fallback mirror (blob.handy.computer) | The real Handy archive was downloaded, pinned and gave the same transcript of the test phrase. The switch-over is unit tested with a local server; not run in the app against the real mirror, because that needs the primary source to fail | unit tests |
| Activity log viewer: filter by session and tool, clear with confirm | Tested | `activity-filter-session.png`, `activity-filter-tool.png`, `activity-clear-confirm.png`, `activity-cleared.png` |
| Docs: README, NOTICE, LICENSE, CONTRIBUTING, `docs/release.md`, Windows checklist | Written | repo root, `docs/` |

## Release build (macOS arm64, unsigned)

`cd app && npx tauri build`: 1 min 21 s with cached dependencies, exit 0.

| Artifact | Size |
|---|---|
| `target/release/bundle/macos/Deskhand.app` | 55.7 MiB (binary 58,383,888 bytes) |
| `target/release/bundle/dmg/Deskhand_0.0.1_aarch64.dmg` | 16.7 MiB (17,539,783 bytes), SHA-256 `afad3819bf2fe771b69e81a2ef512f8312efaaa683f03b308cab96c04466c83c` |

- Info.plist: `dev.deskhand.app`, 0.0.1, `LSUIElement` true, minimum macOS
  13.0, `NSMicrophoneUsageDescription` present.
- Signature: ad-hoc, linker-signed only (`codesign -dv`). ONNX Runtime is
  linked statically (`otool -L` lists system frameworks only).
- `hdiutil verify`: checksum VALID. Mounted read-only: `Deskhand.app` and
  the `Applications` link. Nothing was copied to /Applications.

Launch check (the bundle's binary, temp data and models dirs, tested):
menu-bar app with Activation policy Accessory, Setup window opened on its
own (first start), MCP server on 127.0.0.1:47823, `/health` 200 "ok" with
the token and 401 without, SIGTERM gave "Deskhand exited cleanly" and no
process left. The same for the three other runs above.

### Screenshot time (release build)

70 `screenshot` MCP calls in one session against the release app, timed at
the client (HTTP round trip), with the per-stage times from the server's
debug log (`RUST_LOG=deskhand_core=debug`).

| Run | Median | p95 | Max |
|---|---|---|---|
| 40 back to back | 50.7 ms | 54.9 ms | 83.7 ms (the first call) |
| 30, 2 s apart | 86.1 ms | 91.9 ms | 94.1 ms |
| All 70 | 54.0 ms | 90.7 ms | 94.1 ms |

Server stages, back to back / spaced: capture 31 / 50 ms median (max 55),
mask 1 / 3 ms, resize 10 / 17 ms. No spikes. The Phase 1 figure of about
1.5 s was a debug build, where the mask and resize run unoptimised. The
10-36 s spikes Phase 1 saw once did not come back in 70 release shots; their
cause is not known (a busy machine during that run is the likely one).

## Test-only switches: how they are gated

- Compiled in only with the cargo feature `test-switches` (app feature
  forwards to the core feature). `npx tauri build` never sets it, and the
  Settings window has no control for any of them.
- In this release build, `--test-turn hello --keep-running` printed
  `WARNING: --test-turn, --keep-running ignored: this build has no test switches`
  and `DESKHAND_TEST_AUTO_APPROVE=TextEdit` printed "is set but this build
  has no test switches ...; ignored, every app still asks".
- A test build prints a `!!! TEST BUILD` banner at start, shows a banner in
  the window, and logs every auto-approval loudly.

## Tests

- `cargo test --workspace`: 122 passed, 0 failed (core 104, server
  integration 15, app 3). They include settings load, save, migration and
  bad files; hotkey parsing, sides and exact matching; the exact `claude mcp
  add` and `remove` arguments; checksum failure and re-fetch, resume, a
  server that ignores Range, and the fallback; token file mode 600 and
  tightening a loosened file.
- Frontend: `svelte-check --fail-on-warnings` 0 errors, 0 warnings; vitest
  12/12; `vite build` OK.

## Not tested, and why

- Connect / Disconnect: they change the shared user-scope Claude Code config
  on a machine whose other agent uses the same login. Manual step below.
- Opening System Settings and "Ask macOS": not allowed in this run.
- Voice with the real microphone in Phase 3: Microphone is not granted to
  the tools here and a person has to speak. Phase 2 tested the loop with a
  WAV file through a test build.
- Tray Quit by click: see "Stray clicks". Quitting by signal was tested.
- Signed and notarized builds, Intel or universal builds: not set up.
- Windows: no Windows machine here. Nothing was built or run on Windows;
  see `docs/windows-test-checklist.md`.

## Stray clicks (disclosure)

While testing the tray menu with synthetic clicks, three clicks landed
outside Deskhand:

1. At (1529, 15) in the menu bar: most likely another app's status item
   (macOS 26 moves status items when the frontmost app changes).
2. At (1459, 201) with no Deskhand menu open; Chrome was the topmost window
   there. Its content was not looked at.
3. A parsing slip sent x = 357196.5, which macOS clamps to the right end of
   the menu bar (the clock item).

No menu or window was left open afterwards. Tray click tests were stopped
there.

## Tony's manual test, in order

On a Mac with a keyboard and a microphone:

1. `git clone` the repo, then `cd deskhand/app && npm ci && npx tauri build`.
2. `open ../target/release/bundle/macos/Deskhand.app` (open it with `open`
   or Finder, not from a terminal, so the grants go to Deskhand). The
   menu-bar icon appears and Setup opens.
3. Setup 1 shows "Signed in". If not: `claude login` in a terminal, then
   Check again.
4. Setup 2: give Accessibility, Input Monitoring, Screen Recording and
   Microphone (Ask macOS or Open System Settings, switch Deskhand on), then
   Quit from the tray and open Deskhand again.
5. Setup 3: read the license, Accept and download (230.5 MB), wait for
   "ready", then press "Finish setup".
6. Hold Fn, say "Open TextEdit and type hello world", let go. The overlay
   listens, then thinks; the approval window asks about TextEdit: click
   Allow yourself. It types and speaks its replies. Press Esc during a
   turn to stop it.
7. Settings: Record the hotkey as Right Option, Save, hold Right Option and
   speak. Set Speak to "Only the final reply", Save, and try again.
8. Activity: filter by session and by tool, then Clear and confirm.
9. Connect (your call: it edits your user Claude Code config). Press
   Connect, or run
   `claude mcp add --transport http --scope user desktop http://127.0.0.1:47823/mcp --header "Authorization: Bearer $(cat ~/Library/Application\ Support/Deskhand/mcp-token)"`.
   `claude mcp list` should show `desktop`; in a new `claude`, ask for a
   screenshot. Then Disconnect, or `claude mcp remove --scope user desktop`.
   Tarsee starts `claude` with `--strict-mcp-config`, so a user-scope server
   should not reach it, but check.
10. Settings: Regenerate token; a connected terminal needs Connect again.
11. Tray "Quit Deskhand"; `pgrep -x deskhand` prints nothing.

Windows: the whole of `docs/windows-test-checklist.md`.
