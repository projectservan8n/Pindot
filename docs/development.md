# Developing Pindot

This is for people who build, test or change Pindot. If you only want to use
it, the [README](../README.md) is enough.

Decisions and the reasons behind them are in [`CLAUDE.md`](../CLAUDE.md).
Evidence for each build phase is in `docs/phase0/` to `docs/phase3/`. How to
contribute: [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## How it works

```
hold key -> microphone -> Moonshine (speech to text, on this machine)
         -> typed into ONE interactive `claude`, running in a hidden terminal (PTY)
         -> claude calls Pindot's MCP tools (127.0.0.1 only, bearer token)
         -> Pindot acts on approved apps, shows the AI cursor, logs every call
         -> each reply Claude writes -> Phonon (text to speech, on this machine) -> speaker
```

- With Claude Code as the brain (the default), the brain is the real
  interactive `claude` CLI on the user's own `claude login`, the same program
  they type into. (For a local model or an API key, see "Pindot's own loop"
  below.) Pindot never uses `claude -p`,
  the Agent SDK or an API key, and blanks `ANTHROPIC_API_KEY` for the child
  so a stray key cannot switch it to API billing.
- That `claude` runs in its own empty folder (`<data dir>/session`) with
  `--setting-sources project,local`, so the user's personal Claude Code
  settings, hooks and permission mode do not apply to it. Pindot gives it its
  own settings file (a Stop hook back to Pindot) and `--permission-mode
  default` with only the desktop tools allowed (Ask), or a small set of
  built-in tools with `--permission-mode auto` (Auto, chosen through a
  warning in Settings).
- Pindot reads Claude's answers from the session transcript that Claude Code
  writes, not by scraping the terminal screen.
- After 10 idle minutes the next request starts a fresh session, to keep the
  context small.
- Chat types into the same persistent session as the voice. Replies come
  from the transcript under `~/.claude`, which Pindot only reads.
- Attachments are copied to `<data dir>/attachments/<session>/` (up to 8
  files, 10 MB each, 5 MB for pictures, 25 MB in all; folders older than 30
  days are deleted). Claude may read only that folder, through a scoped
  `Read(//.../attachments/<session>/**)` rule, never a bare `Read`.

### The MCP tools

A local MCP server named `desktop` with these tools: `desktop_context`,
`request_access`, `open_app`, `focus_window`, `read_window` (the
accessibility tree as text, with refs), `find`, `screenshot`, `zoom`,
`click`, `type_text`, `key`, `scroll`, `drag`, `wait`, `point`, and for the
Vault `vault_logins` and `vault_fill`.

## Pindot's own loop (local models and APIs)

When Settings > Brain is a local model or an API key, the voice loop and
Chat talk to `LoopBrain` instead of `claude`, through the same `Brain`
trait (`crates/pindot-core/src/brain/mod.rs`). Its parts, in
`crates/pindot-core/src/brain/native/`:

| File | Does |
|---|---|
| `mod.rs` | `LoopBrain`: worker thread, sessions, retries when the server is down. |
| `providers.rs` | The presets (Ollama, LM Studio, llama.cpp, OpenAI, OpenRouter, DeepSeek, Groq, Custom). |
| `settings.rs` | The `brain` block of settings.json. |
| `secrets.rs` | API keys in the OS credential store ("Pindot AI keys"). |
| `openai.rs` | Chat Completions: messages, the streamed reply, errors in plain words. |
| `mcp_client.rs` | The loop's MCP client of Pindot's own server (`X-Pindot-Session`). |
| `run.rs` | One turn: model, tool calls, results, repeat (25 calls at most). |
| `context.rs` | Trimming the conversation to fit the model's context. |
| `log.rs` | `<data dir>/conversations/<id>.jsonl`, in the line format Chat reads. |
| `probe.rs` | Finding local servers, their models and what each can do. |
| `test_model.rs` | The two-turn model test. |

`crates/pindot-core/tests/native_loop.rs` runs the loop against Pindot's
real server (mock platform parts) and a fake Chat Completions server.

To try it with a real model: install [Ollama](https://ollama.com), pull a
model that uses tools (for example `ollama pull qwen3:8b`), start it with
`OLLAMA_CONTEXT_LENGTH=32768 ollama serve`, then Settings > Brain > A model
on this computer, and Test this model.

## Data folder, settings and environment variables

The data folder is `~/Library/Application Support/Pindot/` on macOS and
`%APPDATA%\Pindot\` on Windows (folder 0700, files 0600 on macOS). It holds
`settings.json`, the bearer token (`mcp-token`), the generated
`claude-mcp*.json` and `claude-settings*.json`, `activity.jsonl` (rotated at
5 MB), `chats.json`, `vault.json`, the `models/` folder and the voice
brain's `session/` folder. Earlier `voice-settings.json` files are moved into
`settings.json` automatically.

Environment variables override settings without saving them:
`PINDOT_PORT`, `PINDOT_HOTKEY`, `PINDOT_STT_MODEL`, `PINDOT_TTS_MODEL`,
`PINDOT_TTS_VOICE`, `PINDOT_SPEAK_MODE`, `PINDOT_VOLUME`, `PINDOT_DATA_DIR`,
`PINDOT_MODELS_DIR`, `PINDOT_CLAUDE_BIN` (path to `claude`).

Pindot was called Deskhand until 15 Nov 2026. A Deskhand data folder is
moved to the new name on first start (settings, token and models come
along); macOS asks for its permissions once more because the app's identity
changed.

## Build from source

### macOS

You need macOS 13 or later, the Xcode command line tools, Rust 1.90 or later
(2024 edition) and Node.js 22 or later (tested with Rust 1.100 and Node 24).

```sh
git clone https://github.com/projectservan8n/pindot.git && cd pindot
cargo test --workspace                  # Rust tests
cd app && npm ci
npm run check && npm test               # type-check and frontend tests
npx tauri build                         # release .app and .dmg
```

The bundle lands in `target/release/bundle/macos/Pindot.app` and
`target/release/bundle/dmg/`. The first build downloads ONNX Runtime (about
148 MB) through the `ort` crate.

`scripts/build-mac.sh` builds the `.app` signed with the Developer ID (or
Apple Development) identity it finds in your keychain
(`APPLE_SIGNING_IDENTITY` overrides). Use it when you rebuild often: macOS
keys the four permission grants to the signature, and an ad-hoc build gets a
new one every time, so it asks again. `tccutil reset All dev.pindot.app`
clears stale entries. `docs/release.md` has the notarization steps.

For a quick debug run: `npm run build` in `app/`, then `cargo run -p pindot`
from the repo root. The frontend is embedded at compile time, so rebuild it
first. Try voice in release builds only: a debug build takes over ten
seconds to start speaking.

### Windows

Install Rust (MSVC toolchain), Node.js, the Visual Studio C++ build tools
(2019 or 2022), WebView2 (part of Windows 11) and Claude Code. Then:

```powershell
scripts\build-windows.ps1              # the app and its installer
scripts\build-windows.ps1 -NoBundle    # the app only
```

That gives `target\release\pindot.exe` and
`target\release\bundle\nsis\Pindot_<version>_x64-setup.exe`, a 10 MB
installer for the current user (no administrator rights).

To work on it:

```powershell
cargo test --workspace
cd app; npm ci; npm run check; npm test; npm run build; cd ..
cargo build --release -p pindot        # target\release\pindot.exe
```

The script links ONNX Runtime as a DLL and ships it next to `pindot.exe`.
Plain `cargo` links it statically, and with the older Visual Studio 2019
build tools that link step fails in `libort_sys` (unresolved
`__std_find_trivial_*` symbols): the prebuilt ONNX Runtime needs a newer C++
runtime library. Either install the 2022 build tools, or link the DLL
yourself: unpack `onnxruntime-win-x64-1.28.0.zip` from the ONNX Runtime
releases on GitHub and set these before running `cargo`:

```powershell
$env:ORT_LIB_LOCATION = "C:\path\to\onnxruntime-win-x64-1.28.0\lib"
$env:ORT_PREFER_DYNAMIC_LINK = "1"
```

`onnxruntime.dll` is then copied next to `pindot.exe` and must stay there.

`docs/windows-test-checklist.md` lists what was seen on Windows and what is
left to check.

## Developer tools

- `python3 scripts/mcp_call.py init`, then `call desktop_context '{}'`,
  `call request_access '{"apps":["TextEdit"],"reason":"try it"}'`, `end`:
  call the tools by hand while Pindot runs (one MCP session). End the
  session when done: it holds the input lock.
- `python3 scripts/bench_tools.py`: times the tools the same way on macOS and
  Windows (instructions at the top of the file).
- `cargo run --release -p pindot-core --bin voice_check`: audio devices,
  model download and checksum, speech to text on a WAV, text to speech.
- Phase 0 spikes, each run from a terminal (macOS then asks for permissions
  on behalf of the terminal):
  `target/debug/spike_hotkey status | listen --seconds 20 | selftest --control`
  (hold Fn without the Globe action), `pindot --demo --exit-after 30` (the
  AI cursor on each monitor; add `--capturable` to see it in screenshots),
  `spike_mcp --seconds 600` (the MCP server alone) and
  `spike_brain --prompt 'Reply with exactly the word: pong' --model haiku`
  (interactive `claude` in a PTY, answer through the Stop hook).
- The Swift test window:
  `swiftc -O -o spikes/test-target/test-target spikes/test-target/main.swift`,
  run with `--regular` (so `request_access` can name it) and `--seconds N`.
- To try the Vault safely, serve `docs/vault/test-page/` on loopback and save
  a login for `http://localhost:4310`.
- `docs/ui-pass/tools/` renders the app's pages for screenshots.

## Test-only switches

The automated tests use switches that skip the human: auto-approving apps
(`PINDOT_TEST_AUTO_APPROVE`), a WAV file in place of the microphone and a
synthetic hotkey press (`--test-voice`), and one scripted `claude` turn
(`--test-turn`). They are compiled in only with `cargo build -p pindot
--features test-switches`. Such a build prints "!!! TEST BUILD" and shows a
banner in its window. Normal and release builds ignore these switches with a
printed warning, and the settings window has no way to reach them.

## Known limitations

- **Shared session for outside clients.** The `claude` that Pindot starts
  sends a Pindot session header, so its app approvals end with it. MCP
  clients that do not send that header (for example a terminal `claude`
  connected through "Connect to Claude Code") all share one Pindot session:
  an app approved in one of them is approved for the others until Pindot
  quits, and they share the input lock. Fine for one terminal at a time; not
  right for several at once.
- Claude Code 2.1.325 writes thinking blocks to the transcript with the text
  blanked, so Chat shows thinking only when a transcript carries it.
- Past conversations open read only; resuming one is not built yet.

## Open questions

- **`--restricted`**: Claude Code's stricter isolation flag is not used; it
  is untested here. Pindot uses `--setting-sources project,local` plus its
  own session folder instead. Whether to adopt it is open.
- Settings isolation is partial: user and plugin skills still appear in the
  voice session's slash menu.
- `--permission-mode default` is accepted by Claude Code 2.1.325 but is not
  in its help text; if it stops working, Pindot should switch to `manual`.
