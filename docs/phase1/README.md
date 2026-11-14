# Phase 1 evidence: tools, accessibility tree and guardrails (macOS)

Machine: Mac mini, macOS 26.5, arm64, one display 1920x1080 pt at scale 1
(screenshots are 1280x720 px). Debug build. All clicking and typing happened
in TextEdit windows the tests opened themselves; every document was closed
with "Delete" (never saved) and TextEdit was quit afterwards.

"Tested" below means the real thing ran and the result was checked. Unit
tests against mocks are listed separately.

## Files

| File | What it shows |
|---|---|
| `approval-window.png` | The approval window (real, not answered: it timed out and denied) |
| `activity-view.png` | The live activity view (`deskhand --activity`) |
| `banner-controlling-input.png` | The "Claude is controlling input · press Esc to stop" banner, taken with `--capturable` |
| `ai-cursor-before-click.png` | The AI cursor on the target just before a click, `--capturable` |
| `window-screenshot-tool.jpg` | `screenshot(window_id)` result |
| `click-xy-mapped.jpg` | A click at x,y in screenshot pixels landed where intended |
| `scroll-up.jpg`, `drag-select.jpg` | `scroll` and `drag` results in the test document |
| `display-screenshot-masked-640.jpg` | `screenshot()` of the whole display in normal (non-capturable) mode: every non-approved app greyed, no Deskhand windows, no AI cursor |
| `runA-early-activity.txt` | Approval timeout denies, the gate refuses `open_app`/`read_window` before approval, then `open_app` |
| `runB-tool-tests-activity.txt` | 80 activity rows of the direct tool tests (`scripts/mcp_call.py`) |
| `runC-failed.txt` | The first done-when attempt, which failed: the two bugs it found (see below) |
| `runD-done-when.txt` | The done-when run that passed: a real interactive `claude` turn |
| `runD-result-window.jpg` | The run D document afterwards, read independently: the typed line is there |
| `lock-refusal-live.txt` | Live lock test: a second MCP session is refused while the first holds the lock in `wait` |
| `sheet-read-right-after-cmd-w.txt` | `read_window` straight after `cmd+w` sees the Save sheet (after the race fix) |

## Tools

| Tool | Status |
|---|---|
| `desktop_context` | Tested |
| `request_access` | Tested with `DESKHAND_TEST_AUTO_APPROVE` and with the real window timing out (denied after 120 s). The Allow/Deny buttons were not clicked: a human has to (see below) |
| `open_app`, `focus_window` | Tested |
| `read_window` | Tested on the TextEdit Open panel, a document window, the menu bar and a Save sheet; pagination tested in unit tests |
| `find` | Tested (menu items such as Format > Make Plain Text, buttons) |
| `screenshot` | Tested (display and window); masking verified |
| `zoom` | Tested. On a 1x display it can only return native pixels |
| `click` | Tested by ref (buttons, menu items, a sheet's Delete) and by x,y |
| `type_text`, `key` | Tested (`return` with repeat, `cmd+z`, `cmd+w`, `cmd+q`). The secure-field refusal is unit tested only |
| `scroll`, `drag`, `point` | Tested |
| `wait` | Tested (3000 ms and 800 ms waits, holding the lock; the Esc test cancelled a wait) |

## Guardrails

| Guardrail | Status |
|---|---|
| Per-app approval each session, human-answered window | Window shown and timeout-denies tested; the buttons need a human |
| Extra warning for shells/IDEs, file managers, settings, password managers | Unit tested (`server/approval.rs`); not shown live |
| Gate (refs, hit test, keyboard focus, screenshot mask) | Tested: unapproved app refused, our own windows refused, mask verified |
| Global Esc | Tested with a synthetic HID-level Esc (`--test-esc-after`): the key was consumed (0 Esc events reached apps), the action was cancelled, the next call refused, the halt lifted by the prompt hook. Physical key not tested |
| Indicator and AI cursor glide | Tested (screenshots with `--capturable`) |
| One controlling session | Unit tested; tested live (`lock-refusal-live.txt`: a second MCP session got "Another Claude session (52e01c70) is controlling this computer" while the first was in a 3 s `wait`) |
| 127.0.0.1, bearer token, Origin | Tested live: Origin header gives 403, no token 401, good token 200; also unit tested |
| Activity log and view | Tested; the file is 0600 |

## Done-when run

**Run D, 14 Nov 2026 01:23 UTC: passed** (`runD-done-when.txt`). Real
interactive `claude` 2.1.325 in a PTY on `claude login` (no `-p`, no API
key), `--model haiku --permission-mode default --allowedTools mcp__desktop`,
`--mcp-config`/`--settings` files, cwd `.spike-cwd`. TextEdit was approved by
the test-only `DESKHAND_TEST_AUTO_APPROVE`.

- Tools called, in order: ToolSearch (Claude Code loads deferred MCP tools),
  `desktop_context`, `request_access`, `open_app`, `read_window` (the Open
  panel, menus on, page 1 of 2), `click` ref e35 "New Document",
  `read_window`, `click` ref e199 (the text area), `type_text` "Hello from
  Deskhand phase 1", `read_window` (filter all, to check).
- **Screenshots: 0.** Stop hook after 17.3 s; the UserPromptSubmit and Stop
  hooks and the transcript agree; `claude` exited cleanly.
- Checked independently afterwards: `read_window` of the document shows
  `textarea value="Hello from Deskhand phase 1"` (`runD-result-window.jpg`).
  The document was then closed with Delete and TextEdit quit.

**Run C failed** (`runC-failed.txt`) and found two bugs, both fixed and
verified by run D:

1. Claude Code treated the fast typed prompt as a paste and dropped the
   Enter, so the prompt sat unsent. `type_prompt` now waits for the echo and
   400 ms of quiet before Enter, and `drive_turn` presses Enter once more if
   no UserPromptSubmit hook arrives in 6 s. Teardown sends Ctrl+C before
   `/exit`, so `/exit` can't be appended to an unsent prompt.
2. Claude Code 2.1.325 sends tool calls as stateless MCP 2026-07-28 requests
   (no transport session), so every call was a new session and the approval
   was lost at once. A Deskhand session is now one Claude session, keyed by
   an `X-Deskhand-Session` header in the brain's MCP config (unit test
   `stateless_calls_share_the_claude_session_named_by_the_header`, which
   fails without the fix).

## Manual tests for Tony

Build first: `cd ~/deskhand && cargo build` (since Phase 3, step 2 needs a
test build: `cargo build -p deskhand --features test-switches`, because
`DESKHAND_TEST_AUTO_APPROVE` is ignored without it). Only one Deskhand can run at a
time (it binds 127.0.0.1:47823).

1. **Approval Allow and Deny (needs a human click).**
   `./target/debug/deskhand` (no test variables). In another terminal:
   `python3 scripts/mcp_call.py init`, then
   `python3 scripts/mcp_call.py call request_access '{"apps":["TextEdit"],"reason":"manual test"}'`.
   The approval window appears. Click "Allow for this session": the call
   returns "Approved for this session: TextEdit". Run the call again with
   `"apps":["Calculator"]` and click Deny: it returns denied. Then try
   `"apps":["Terminal"]`: the window should show a red warning; press Esc
   (denies). Check that Enter does nothing. `mcp_call.py end`, quit the app
   with Ctrl+C.
2. **Physical Esc.** Start the app with
   `DESKHAND_TEST_AUTO_APPROVE=TextEdit ./target/debug/deskhand`, then
   `mcp_call.py init`, `call request_access '{"apps":["TextEdit"],"reason":"esc"}'`,
   `call open_app '{"name":"TextEdit"}'`, `call wait '{"ms":10000}'` and
   press Esc on the keyboard while it waits. Expect: the banner goes,
   the call returns "Stopped: the user pressed Esc", TextEdit did not
   see the Esc, and the next `call wait '{"ms":100}'` is refused. Press
   Esc again when nothing runs: it must reach apps normally.
3. **Second monitor.** With a second display attached (ideally a different
   scale), run `call desktop_context '{}'`: both displays listed. Put a
   TextEdit window on the second display, `call screenshot '{"display":1}'`,
   then `call click` at a point in that image: the AI cursor and the click
   must land on the right spot, and the banner should show on both displays.
4. **Permissions.** On a fresh Mac (or after removing Deskhand from
   Privacy & Security), start the app: it must say which permission is
   missing (Accessibility, Screen Recording, Input Monitoring) and the tools
   must fail with that message, not crash.
5. **Windows.** See `docs/windows-test-checklist.md`, section (e).
