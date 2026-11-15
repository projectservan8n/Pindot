# Pindot

Hold a key, say what you want, and your own Claude Code does it on your
desktop and answers out loud.

Pindot works with Claude Code. It is an independent open-source project
(MIT), not made by, endorsed by or affiliated with Anthropic. "Claude" and
"Claude Code" are Anthropic's names for their products.

**Status:** version 0.0.1, macOS only so far (built and tested on macOS 26.5,
Apple silicon). Windows has honest stubs and a test checklist, nothing more.
The release build is unsigned. Evidence for each phase is in `docs/phase0/`
to `docs/phase3/`; decisions and conventions are in `CLAUDE.md`.

Pindot was called Deskhand until 15 Nov 2026. Updating from a Deskhand
build moves its data folder to the new name on first start (settings, token
and models come along); macOS asks for its permissions once more because the
app's identity changed.

## What it is

A small menu-bar app (Tauri 2) that gives the `claude` command you already
use two new abilities:

- **Hands and eyes.** A local MCP server named `desktop` with 15 tools:
  `desktop_context`, `request_access`, `open_app`, `focus_window`,
  `read_window` (the accessibility tree as text, with refs), `find`,
  `screenshot`, `zoom`, `click`, `type_text`, `key`, `scroll`, `drag`,
  `wait` and `point` (show where something is without clicking).
- **A voice.** Hold the hold-to-talk key, speak, let go. Speech becomes text
  on your Mac, goes to your signed-in `claude`, and the replies are spoken.

An AI cursor on screen glides to every target before Claude clicks, and a
banner shows whenever Claude is controlling input. Esc stops everything.

## How it works

```
hold key -> microphone -> Moonshine (speech to text, on this Mac)
         -> typed into ONE interactive `claude`, running in a hidden terminal (PTY)
         -> claude calls Pindot's MCP tools (127.0.0.1 only, bearer token)
         -> Pindot acts on approved apps, shows the AI cursor, logs every call
         -> each reply Claude writes -> Phonon (text to speech, on this Mac) -> speaker
```

- The "brain" is the real interactive `claude` CLI on your own `claude
  login`, the same program you type into. Pindot never uses `claude -p`,
  the Agent SDK or an API key, and blanks `ANTHROPIC_API_KEY` for the child
  so a stray key cannot switch it to API billing.
- That `claude` runs in its own empty folder (`<data dir>/session`) with
  `--setting-sources project,local`, so your personal Claude Code settings,
  hooks and permission mode do not apply to it. Pindot gives it its own
  settings file (a Stop hook back to Pindot) and `--permission-mode
  default` with only the desktop tools allowed.
- Pindot reads Claude's answers from the session transcript that Claude
  Code writes, not by scraping the terminal screen.
- After 10 idle minutes the next request starts a fresh session, to keep the
  context small.

## Install

There is no signed download yet. Build it from source (below), then open
`target/release/bundle/dmg/Pindot_0.0.1_aarch64.dmg` and drag Pindot to
Applications, or run `target/release/bundle/macos/Pindot.app` in place.

Because the app is not signed or notarized, macOS blocks the first open:
open System Settings > Privacy & Security and press "Open Anyway" for
Pindot (or Control-click the app and choose Open). `docs/release.md` has
the signing and notarization steps for a real release.

You also need Claude Code installed and signed in: run `claude login` once
in a terminal. Pindot never signs in for you.

On first start Pindot puts an icon in the menu bar (no Dock icon) and
opens the **Setup** window. Setup walks through:

1. **Claude Code**: found or not, signed in or not. If not, it tells you to
   run `claude login` in a terminal and press Check again.
2. **Permissions** (below).
3. **Voice models**: about 231 MB, downloaded once. The licenses are shown
   before anything downloads; every file is checked against a pinned
   SHA-256. A stopped download resumes; a damaged file is fetched again.
4. **Terminal mode** (optional, below).
5. **Finish**: hold-to-talk starts once everything it needs is in place.

The menu-bar menu has Setup, Settings, Activity, a hold-to-talk switch and
Quit.

## Permissions (macOS)

| Permission | What for | Needed by |
|---|---|---|
| Accessibility | Reading windows (the accessibility tree), clicking, typing, the Esc stop key | All tools, hold-to-talk |
| Input Monitoring | Seeing the hold-to-talk key and Esc | Hold-to-talk |
| Screen Recording | The `screenshot` and `zoom` tools | Screenshots only |
| Microphone | Hearing you while you hold the key | Hold-to-talk |

Setup shows the state of each one. The check is read-only and never shows a
macOS prompt. Each row has "Ask macOS" (shows the system prompt) and "Open
System Settings" (opens the right list, where you switch Pindot on). Some
permissions take effect only after Pindot restarts. If you start the
binary from a terminal, macOS asks on behalf of the terminal app instead.

Windows needs no grant for input or capture; the microphone has a privacy
switch under Settings > Privacy & security > Microphone.

## Voice mode

Hold the hold-to-talk key (default: **Fn** on macOS, **Ctrl+Win** on
Windows), speak, let go. A small pill near the top of the screen shows
Listening, Thinking, Working and Speaking.

- Hold the key while it is speaking to cut it off and talk again.
- Press **Esc** at any time: it cancels the action in flight, stops speech
  and interrupts Claude's turn. The Esc key press is swallowed.
- The first time Claude wants an app, an approval window asks you. Nothing
  is touched until you press "Allow for this session".
- Settings > Speak chooses between speaking every message Claude writes as
  it arrives (default) and only the final reply.

Speech to text is Moonshine v2 Small (English, MIT). Text to speech is
Phonon 31m (English, Apache-2.0). Both run on your Mac.

## Terminal mode

You can use the desktop tools from any `claude` session in a terminal,
without voice. Settings (or Setup) > "Connect to Claude Code" runs:

```sh
claude mcp add --transport http --scope user desktop http://127.0.0.1:47823/mcp \
  --header "Authorization: Bearer <token>"
```

This changes your Claude Code user settings (`~/.claude.json`), so it only
runs when you press the button. "Copy command" copies the same line with
your token so you can run it yourself. "Disconnect" runs `claude mcp remove
--scope user desktop`. After you regenerate the token or change the port,
press Connect again. Pindot must be running for the tools to work.

To try the tools in one session only, without changing any settings:

```sh
claude --mcp-config "$HOME/Library/Application Support/Pindot/claude-mcp.json" \
  --strict-mcp-config --allowedTools mcp__desktop
```

## Settings

Stored in `settings.json` in the data dir (`~/Library/Application
Support/Pindot/` on macOS, `%APPDATA%\Pindot\` on Windows), file mode
600. Earlier `voice-settings.json` files are moved in automatically.

- **Assistant**: its name (Rusty by default), how it talks to you
  (friendly, brief, professional or playful) and, optionally, your own words
  on it ("Call me Sam"). Your words shape the tone only; the safety rules in
  its prompt stay. The name also shows on the "controlling input" banner.
- **Hold-to-talk shortcut**: one to four modifier keys held together, e.g.
  `Fn`, `Right Option`, `Ctrl+Cmd`, `Right Alt`, `Ctrl+Win`. Pick a preset,
  type it, or press Record and hold the keys. Pressing any other key while
  holding cancels the hold, so normal shortcuts still work. A lone Ctrl,
  Option or Cmd gets a warning (they are part of many shortcuts).
- **Speech to text and text to speech models**, **speak mode**, **volume**.
- **MCP port** (default 47823; applies after a restart).
- **Regenerate token**: the old token stops working at once.

Environment variables override settings without saving them:
`PINDOT_PORT`, `PINDOT_HOTKEY`, `PINDOT_STT_MODEL`,
`PINDOT_TTS_MODEL`, `PINDOT_TTS_VOICE`, `PINDOT_SPEAK_MODE`,
`PINDOT_VOLUME`, `PINDOT_DATA_DIR`, `PINDOT_MODELS_DIR`,
`PINDOT_CLAUDE_BIN` (path to `claude`).

## Guardrails

- **Per-app approval** for every session, answered by you in Pindot's own
  window. Terminals and IDEs (equal to shell access), file managers, system
  settings and password managers get an extra red warning. No answer in
  120 s, Esc or closing the window all mean Deny.
- **Only approved apps are touched.** A click, scroll or drag at a point is
  checked against the window under it first; typing needs an approved app
  in focus; screenshots grey out every window of an app you did not
  approve; `type_text` refuses password fields.
- **Esc stops everything**, as above.
- **Visible control**: the banner "Claude is controlling input · press Esc
  to stop" and the AI cursor moving to each target before it clicks.
- **One controlling session at a time** (an input lock).
- **The MCP server** listens on 127.0.0.1 only, needs the bearer token,
  and refuses any request that carries a browser `Origin` header.
- **Untrusted screen text**: tool results and the voice prompt tell Claude
  that text on screen is data, never instructions, and to ask you before
  anything destructive, any purchase, sending a message or typing
  credentials.
- **Activity log**: every tool call is written to `activity.jsonl` in the
  data dir. Open Activity from the menu bar to filter by session or tool,
  or clear it (it asks first).

**Known limitation: shared session for outside clients.** The `claude` that
Pindot starts for voice sends a Pindot session header, so its approvals
end with it. MCP clients that do not send that header (for example a
terminal `claude` connected through "Connect to Claude Code") all share one
Pindot session: an app approved in one of them is approved for the others
until Pindot quits, and they share the input lock. Fine for one terminal
at a time; not right for several at once.

## Privacy

- **Nothing leaves your machine except what your own `claude` CLI sends.**
  Your voice is turned into text on your Mac and never saved; the spoken
  replies are made on your Mac. When Claude calls a tool, the result (window
  text, a screenshot) goes back to `claude`, and `claude` sends it to
  Anthropic as part of your conversation, exactly as if you had used Claude
  Code yourself. Pindot has no server, no account and no telemetry.
- **Which usage it draws from**: Pindot runs the interactive `claude` CLI,
  the same way you do in a terminal, on the account you signed in with
  `claude login`. With a Claude subscription it counts against your plan's
  normal Claude Code usage limits. (On 14 May 2026 Anthropic announced that
  `claude -p` and the Agent SDK would move to a separate monthly credit; it
  put that on hold on 15 June 2026. Pindot uses neither, so it is not
  affected either way.) If your `claude login` is an Anthropic Console
  account, usage is billed to that account, as for your own terminal use.
- **It runs the interactive CLI on your `claude login`.** It never asks for,
  stores or sets an API key, and never handles your sign-in.
- The only other network traffic is the one-time model download from
  Hugging Face (with Handy's mirror, blob.handy.computer, as a fallback for
  the speech-to-text model).
- Stored locally, readable by you only: the bearer token (`mcp-token`), the
  config files that contain it, `settings.json`, `activity.jsonl` (rotated at
  5 MB) and the models. Claude Code keeps its own transcripts of the
  sessions under `~/.claude/projects/`.

## Troubleshooting

- **"Claude Code is not signed in"**: run `claude login` in a terminal, then
  Setup > Check again.
- **Hold-to-talk does nothing**: the status line at the bottom of the
  Pindot window says why (permissions, models, `claude`), and Setup >
  Finish lists what is missing. Check Input Monitoring and Accessibility,
  then quit and reopen Pindot. If you started it from a terminal, grant the
  permissions to that terminal app.
- **"The MCP server is not running ... port"**: another program uses port
  47823. Change the port in Settings, restart Pindot, press Connect again.
- **Terminal `claude` gets 401 from `desktop`**: the token was regenerated.
  Press Connect to Claude Code again.
- **Every click is refused**: approve the app first (Claude calls
  `request_access`; answer the window). Pindot's own windows are never
  clickable by Claude.
- **A damaged or partial model**: Setup > Voice models > Check files
  re-checks every SHA-256 and downloads a bad file again.
- **The Esc key "does nothing" in other apps**: it is only swallowed while
  Claude is acting or a voice turn runs.
- **Logs**: start `Pindot.app/Contents/MacOS/pindot` from a terminal to
  see its log; `--setup` and `--activity` open those views.

## Build from source

### macOS (tested)

You need macOS 13 or later, the Xcode command line tools, Rust 1.90 or later
(2024 edition) and Node.js 22 or later (tested with Rust 1.100 and Node 24).

```sh
git clone <this repo> pindot && cd pindot
cargo test --workspace                  # Rust tests
cd app && npm ci
npm run check && npm test               # type-check and frontend tests
npx tauri build                         # release .app and .dmg (unsigned)
```

The bundle lands in `target/release/bundle/macos/Pindot.app` and
`target/release/bundle/dmg/`. For a quick debug run: `npm run build` in
`app/`, then `cargo run -p pindot` from the repo root (the frontend is
embedded at compile time, so rebuild it first). The first build downloads
ONNX Runtime (about 148 MB) through the `ort` crate.

### Windows (not tested; nothing was run on Windows)

Install Rust (MSVC toolchain), Node.js, the Visual Studio C++ build tools,
WebView2 (part of Windows 11) and Claude Code. Then the same commands; the
bundle targets are listed in `docs/release.md`. Today the hotkey, input,
capture and accessibility parts are stubs that return "not supported yet",
so the app cannot drive Windows. `docs/windows-test-checklist.md` lists
what to build and check.

### Developer tools (macOS)

- `python3 scripts/mcp_call.py init`, then `call desktop_context '{}'`,
  `call request_access '{"apps":["TextEdit"],"reason":"try it"}'`, `end`:
  call the tools by hand while Pindot runs (one MCP session).
- `cargo run --release -p pindot-core --bin voice_check`: audio devices,
  model download and checksum, speech to text on a WAV, text to speech.
- Phase 0 spikes, each run from a terminal (macOS then asks for permissions
  on behalf of the terminal):
  `target/debug/spike_hotkey status | listen --seconds 20 | selftest --control`
  (hold Fn without the Globe action), `pindot --demo --exit-after 30`
  (the AI cursor on each monitor), `spike_mcp --seconds 600` (the MCP
  server alone) and `spike_brain --prompt 'Reply with exactly the word: pong'
  --model haiku` (interactive `claude` in a PTY, answer through the Stop hook).
  The Swift test window is
  `swiftc -O -o spikes/test-target/test-target spikes/test-target/main.swift`.
  Evidence and details: `docs/phase0/` to `docs/phase3/`.

### Test-only switches

The automated tests use switches that skip the human: auto-approving apps
(`PINDOT_TEST_AUTO_APPROVE`), a WAV file in place of the microphone and a
synthetic hotkey press (`--test-voice`), and one scripted `claude` turn
(`--test-turn`). They are compiled in only with `cargo build -p pindot
--features test-switches`. Such a build prints "!!! TEST BUILD" and shows
a banner in its window. Normal and release builds ignore these switches
with a printed warning, and the settings window has no way to reach them.

## Open questions

- **`--restricted`**: Claude Code's stricter isolation flag is not used; it
  is untested here. Pindot uses `--setting-sources project,local` plus its
  own session folder instead. Whether to adopt it is open.
- Settings isolation is partial: user and plugin skills still appear in the
  voice session's slash menu.
- `--permission-mode default` is accepted by Claude Code 2.1.325 but is not
  in its help text; if it stops working, Pindot should switch to `manual`.

## License

MIT, see `LICENSE`. Third-party models and their licenses are listed in
`NOTICE`. Contributions: see `CONTRIBUTING.md`.
