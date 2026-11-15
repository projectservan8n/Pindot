# Phase 2 evidence: the voice loop (macOS)

Machine: Mac mini, macOS 26.5, arm64, one display. Release build
(`cargo build --release`). claude 2.1.325, `--model haiku`, signed in with a
Claude account (`claude login`). Every click and keystroke happened in
TextEdit documents the tests opened; each was closed with "Delete" (never
saved) and TextEdit was quit afterwards. Phase 2 spent 3 nested `claude`
turns in total (one of them interrupted on purpose).

"Tested" means the real thing ran and the result was checked. Unit tests
against mocks are listed separately.

## Pipeline

Hold Fn → record (cpal) → release → Moonshine v2 Small streaming (English,
`transcribe-rs`) → the text is typed into ONE persistent interactive
`claude` in a PTY → the transcript tailer reports tool calls (overlay
"Working") and each assistant message → Phonon 31m (English, `ptts`) speaks
it → the Stop hook ends the turn.

## Files

| File | What it shows |
|---|---|
| `test-utterance.wav` | The synthetic test phrase "Open TextEdit and type hello" (`say` + `afconvert`, 16 kHz mono). The only audio in git |
| `voice-check.txt` | Audio devices (cpal), model download and checksum verify, STT on the WAV, TTS with duration, RMS and a transcribe-back, one play at 20% volume |
| `run1-voice.log` | Done-when run: hold-speak-release produced a TextEdit document with "hello" and two spoken replies |
| `run1-textedit-check.txt` | That document read afterwards through `read_window`, independently of the run |
| `run2-voice-interrupt.log` | Interrupt run: Esc mid-turn, then a second request in the same session |
| `run2-textedit-check.txt` | The document from run 2's second request, read afterwards |
| `run3-not-signed-in.log` | `claude` installed but signed out (a stand-in script that answers `auth status` with `loggedIn: false`) |
| `notice-not-signed-in.png` | The overlay pill showing that notice (taken with `--capturable`, cropped to the pill) |
| `isolation.txt` | The settings-isolation choice and the evidence that login still works |

## What was tested

| Part | Status |
|---|---|
| Model download at first run, size + SHA-256 check, verify on every load | Tested (`voice-check.txt`, every run log: "models checked in 0.5-0.6 s") |
| STT on the test WAV | Tested. Heard "Open text edit n type hello" ("and" comes out as "n"; Claude understood it) |
| TTS: duration, non-silence, transcribe-back, one play at low volume | Tested (`voice-check.txt`, and every spoken reply in runs 1-2 was transcribed back correctly) |
| Hotkey: synthetic Fn down/up through the real event tap | Tested (runs 1-2: "the Fn tap reported the synthetic press") |
| Persistent `claude` in a PTY with the voice prompt, our MCP config, Stop hook, allowed tools | Tested (runs 1-2) |
| Overlay states listening → thinking → acting → speaking → idle | Tested as events (runs 1-2 list them with times). The pill was seen on screen for a notice only (`notice-not-signed-in.png`) |
| Speak each assistant message as it lands (EachMessage) | Tested: run 1 spoke the first message 5.8 s before the Stop hook |
| Global Esc during a voice turn: cancels the action, interrupts the turn | Tested (run 2: Esc swallowed, 0 reached apps; turn interrupted with Esc in 122 ms; no Stop hook; the next request worked in the same session and was not merged with the interrupted one) |
| Not signed in → "run `claude login`" notice, retry with a growing delay | Tested with a stand-in `claude` (run 3) |
| Clean exit on Ctrl+C / SIGTERM: tap, audio, `claude`, server closed | Tested (SIGINT to the running app; no process left) |
| Real microphone | Not tested: the mic was never opened. cpal lists "USB PnP Audio Device" as the default input. Needs Tony (below) |
| Physical Fn key, physical Esc, real speech | Not tested: needs a person |
| Hotkey during speech stops it (barge-in); Esc stops speech | Unit tests only (no turn was spoken while a key was pressed) |
| Esc fallback: kill + `--resume` when Claude Code doesn't confirm in 3 s | Not tested live: Esc always worked. Code path only |
| New session after 10 idle minutes | Unit test only |
| `claude` not installed | Unit test only |

Unit tests (mocks): 14 for the voice state machine, runner and speaker,
including listening/thinking/acting/speaking, interrupt, Esc, idle reset,
stale transcripts, empty speech and turn failure. `cargo test --workspace`:
81 passed (67 core unit, 14 server integration), 0 failed.

## Latency (release build)

| Stage | Run 1 | Run 2 (press 2) |
|---|---|---|
| Speech to text (1.82 s of audio) | 224 ms | 134 ms |
| Claude turn (prompt typed → Stop hook) | 15.4 s | 11.2 s |
| Text to speech, first audio | 124 ms, then 43 ms | 99 ms |
| Hotkey release → first spoken audio | 10.0 s (EachMessage, first message) | 11.4 s (one message, at Stop) |
| `claude` ready after start | 2.2 s | 2.1 s |
| Model verify (SHA-256), then load STT + TTS | 0.62 s, then 152 + 21 ms | 0.48 s, then 119 + 21 ms |

Almost all of the wait is the Claude turn (tool calls on the desktop); the
local models add about a quarter of a second.

## Models (settings; never in git)

| | Speech to text | Text to speech |
|---|---|---|
| Model | Moonshine v2 Small streaming, English (`quantized_26_07_30` ONNX build) | Phonon 31m, English (q8 GGUF + default voice) |
| Runtime | transcribe-rs 0.3.12 (ONNX Runtime via ort 2.0.0-rc.13) | ptts 0.7.1 |
| License | MIT | Apache-2.0 |
| Source (pinned commit) | huggingface.co/moonshine-ai/moonshine-voice-assets @ e318117 | huggingface.co/gradium/phonon-31m-en @ 92741c5 |
| Size | 165.5 MB (7 files) | 65.1 MB (4 files) |
| Checksums | SHA-256 per file in `crates/pindot-core/src/voice/models.rs` | same |

Stored in `~/Library/Application Support/Pindot/models/<id>/`. Settings
live in `voice-settings.json` in the data dir (`stt_model`, `tts_model`,
`tts_voice`, `speak_mode` `each_message`/`at_stop`, `idle_reset_secs`,
`volume`, `model`), with `PINDOT_*` environment overrides.

## Test path (off by default, logged loudly)

Since Phase 3 these switches exist only in a test build:
`cargo build --release -p pindot --features test-switches`. A normal or
release build ignores them with a warning.

`pindot --test-voice WAV [--test-voice-esc] [--model haiku]
[--turn-timeout S] [--keep-running]` replaces the microphone with the WAV
and presses Fn synthetically (through the real tap; if the tap doesn't see
it, the press is injected directly and the log says so). Spoken replies go to
a temp WAV and play at 20% volume. Prints `test-voice: RESULT PASS` or
`CHECK OUTPUT`. Run 1:

    PINDOT_TEST_AUTO_APPROVE=TextEdit ./target/release/pindot \
      --test-voice docs/phase2/test-utterance.wav --model haiku

## Manual tests for Tony

Start it with `./target/release/pindot --voice` from a terminal in
`~/pindot` (build first with `cargo build --release`).

1. **Permissions** for the binary `~/pindot/target/release/pindot` (or
   the terminal app that starts it): Microphone (the mic opens on each Fn
   hold, so macOS asks on the first one: allow it, then hold again; not
   granted or tested yet), Accessibility and Input Monitoring (Fn and Esc
   taps, clicking and typing), Screen Recording (screenshots).
2. **Real voice**: hold Fn, say "Open TextEdit and type hello", release.
   Expect the pill to go Listening → Thinking → Working, an approval window
   for TextEdit (click Allow), then the text typed and a spoken reply.
3. **Barge-in**: while it is speaking, hold Fn: speech stops at once and it
   listens again.
4. **Esc**: ask for something long ("Open TextEdit and type the alphabet
   slowly"); press Esc while it works. Expect the banner to say Stopped,
   speech to stop, and no more actions. Then ask again: it should work.
5. **Idle reset**: wait 10 minutes and ask "What did I ask you before?":
   it should not know (new session).
6. Quit with Ctrl+C in the terminal. Close the TextEdit documents without
   saving.

Do not test "not signed in" with `claude logout` on the Mac mini: Tarsee
uses the same login. Run 3 covers it with a stand-in script
(`PINDOT_CLAUDE_BIN=<script>`).
