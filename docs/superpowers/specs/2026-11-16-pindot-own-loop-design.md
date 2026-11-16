# Pindot's own loop: local models and OpenAI-compatible APIs

Status: design approved by Tony on 16 Nov 2026; this spec awaits his review.

## Why

Pindot's only brain today is the user's Claude Code. Tony wants Pindot to
work with other brains too, as Tarsee does: local models and models reached
with an API key. Four brains were asked for. They are built in this order,
one spec each:

1. **A brain interface** (this spec): the voice loop and Chat talk to any
   brain the same way; Claude Code becomes one brain among others.
2. **Pindot's own loop for OpenAI-compatible endpoints** (this spec): local
   servers (Ollama, LM Studio, llama.cpp) and API providers that speak the
   same protocol (OpenAI, OpenRouter, DeepSeek, Groq, any other).
3. The Anthropic API with a key (next spec): a second request format for
   the loop of step 2.
4. The ChatGPT plan through OpenAI's Codex CLI (a later spec): a second
   agent program, driven as Claude Code is.

Decisions taken with Tony (16 Nov 2026):

- Pindot runs the conversation itself (its own loop, in Rust), rather than
  through Codex CLI as Tarsee does. Reasons: the tools are Pindot's own, a
  small prompt matters for local models with a short context, nothing extra
  to install, and replies stream straight to speech.
- A loop brain gets the **desktop tools only** (as Claude Code's Ask mode):
  no shell, no file editing, no web, no brain folder.
- **One brain at a time**, chosen in Setup or Settings, used by voice and
  Chat alike. Switching starts a fresh conversation.
- Choosing a model runs a **short test**; a failure is explained and the
  user may keep the model anyway.

## What stays exactly as it is

- The Claude Code brain: interactive `claude` in a PTY, never `-p`, `--print`
  or the Agent SDK, `ANTHROPIC_API_KEY` blanked for the child, settings
  isolation, permission modes, the brain folder. Existing users who never
  open the new setting see no change.
- The MCP server, its tools, approvals, the input lock, Esc, the overlay,
  the activity log and the Vault rules. A loop brain reaches the tools
  through the same server and the same checks.
- The voice state machine (`voice/machine.rs`) and Chat's reader
  (`chat/transcript.rs`).

## 1. The brain interface

`crates/pindot-core/src/brain/mod.rs` gains:

```rust
pub trait Brain: Send {
    fn send(&self, text: &str);
    fn interrupt(&self);
    fn new_session(&self);
}
```

A brain reports through the existing `InputSink` with the `machine::Input`
events the voice loop already handles: `SessionReady`, `BrainUnavailable`,
`TranscriptPrompt`, `TranscriptText`, `TranscriptToolUse`,
`TranscriptToolResult`, `Stop` and `TurnFailed`.

- `PersistentBrain` (Claude Code) implements `Brain` unchanged.
- `LoopBrain` (new, section 2) implements it too.
- `VoiceRunner`/`LiveIo` hold a `Box<dyn Brain>` instead of a
  `PersistentBrain`. `app/src-tauri/src/voice.rs` builds the one the
  settings name.

## 2. Pindot's own loop (`crates/pindot-core/src/brain/native/`)

### Parts

| File | Does |
|---|---|
| `providers.rs` | The presets: id, name, base URL, whether a key is needed, the ports to look for (local ones), and the key's usual prefix (to catch a key pasted for the wrong provider). |
| `openai.rs` | One Chat Completions client over HTTP (`ureq`): streaming (server-sent events), tool calls, images. |
| `mcp_client.rs` | An MCP client of Pindot's own server on 127.0.0.1: `tools/list` and `tools/call`, with the bearer token and an `X-Pindot-Session` header of the loop's session id. |
| `run.rs` | One turn: the loop below. |
| `context.rs` | Fits the conversation into the model's context. |
| `log.rs` | Writes the conversation file Chat reads. |
| `probe.rs` | Finds local servers, lists models, reads what a model can do. |
| `test_model.rs` | The model test. |
| `secrets.rs` | API keys in the OS credential store. |
| `mod.rs` | `LoopBrain`: a worker thread taking `send`, `interrupt`, `new_session`, as `PersistentBrain` does. |

Presets: Ollama (`http://127.0.0.1:11434/v1`), LM Studio
(`http://127.0.0.1:1234/v1`), llama.cpp (`http://127.0.0.1:8080/v1`; Tarsee
also uses 9931 for newer builds: check the current llama.cpp before
shipping), OpenAI, OpenRouter, DeepSeek, Groq, and Custom (any base URL,
key optional). Read each API's current documentation when implementing;
do not trust memory for URLs and field names.

### One turn

1. Append the user's message, write it to the log and emit
   `TranscriptPrompt` (the voice loop waits for it). Chat attachments arrive in the prompt as
   paths (`attachments::split_prompt`): a picture goes in as an image part
   when the model can see, a text file of up to 32 KB goes in as text, and
   anything else is replaced by a short note that this brain can't open it.
2. Send the system prompt, the conversation and the tool list, streaming.
3. Read the stream. When an assistant message is complete, write it to the
   log and emit `TranscriptText` (the voice loop speaks it as today).
4. For each tool call: emit `TranscriptToolUse`, call the tool through the
   MCP client, emit `TranscriptToolResult`, append the result. Text goes in
   a `tool` message; an image (`screenshot`, `zoom`) goes in a `user`
   message right after the tool messages, because many servers accept text
   only in `tool` messages.
5. Repeat from 2 until a message has no tool calls; then emit
   `Stop { session_id, last_message }`.

Limits: at most 25 tool calls per turn (then stop, with a spoken "I've
stopped after 25 steps"), and the existing 15-minute turn cap
(`persistent::TURN_TIMEOUT`).

Esc: `interrupt` sets a cancel flag. Reading the stream stops at the next
chunk, pending tool calls are skipped, `[Request interrupted by user]` is
written to the log and the turn ends. A tool call already running is
stopped by the server's own Esc handling, as today.

The system prompt is `voice::voice_system_prompt` (name, rules, talk style,
the user's notes) plus one line on the tools. Tool names and descriptions
come from `tools/list`. Target: under 6k tokens of prompt and tools; the
plan measures it.

### Sessions and the conversation file

Each conversation has a UUID, sent as `SessionReady { session_id }` at the
start, after `new_session`, and after the voice loop's idle reset. The
conversation is written to `<data dir>/conversations/<id>.jsonl` (folder
0700, files 0600) in the subset of Claude Code's transcript lines that
`chat::transcript::parse_line` reads:

- `{"type":"user","uuid":...,"message":{"role":"user","content":"..."}}`
- `{"type":"assistant","uuid":...,"message":{"role":"assistant","content":[{"type":"text","text":"..."},{"type":"tool_use","id":"...","name":"mcp__desktop__click","input":{...}}]}}`
- `{"type":"user","uuid":...,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"...","content":"...","is_error":false}]}}`

Tool names are written with the `mcp__desktop__` prefix, so Chat shows them
as it shows Claude Code's. Images are not written to the file (a
`[screenshot]` note instead). The chat history index (`chat::history`)
stores the file's path, so History opens these conversations as it does
Claude Code's.

### Fitting the context

The context size comes from the server when it says (Ollama, LM Studio,
llama.cpp), else from Settings, else 32k. Before each request, if the
estimate (4 characters a token) is over 85% of it: drop the image of every
screenshot but the latest (a note stays), then shorten old tool results to
their first 300 characters, then drop the oldest whole turns. A server
error saying the context is too long trims harder and retries once; a
second failure ends the conversation with "This conversation got too long
for <model>; I'll start a fresh one" and a new session.

### Seeing pictures

`probe.rs` asks the server what the model can do: Ollama's `/api/show`
(its capabilities include vision and tools), LM Studio's model list (its
type says whether it can see), OpenRouter's model list (input
modalities). Anything else: not known. Settings > Brain has
Vision: Automatic, On, Off. A model that can't see gets no `screenshot`
or `zoom` tools in its tool list, and its prompt says to use
`read_window` and `find`.

### Keys

API keys are stored with `keyring-core` (as the Vault's key): service
"Pindot AI keys", account = provider id; macOS Keychain, Windows Credential
Manager. Never in `settings.json`, the logs, the activity log, error
messages or the conversation files. Through the UI a key can be saved,
replaced and deleted, never shown. A key is sent only over https, or over
http to a loopback address.

### When things go wrong

Plain messages, through `BrainUnavailable` (cannot start) or `TurnFailed`
(one turn):

| Case | Message |
|---|---|
| Local server not running | "Ollama isn't running. Start it, or pick another brain in Settings." |
| 401 or 403 | "<Provider> refused the API key. Check it in Settings > Brain." |
| 402 | "Your <Provider> account is out of credit." |
| 429 | "<Provider> is busy or rate-limiting. Try again in a moment." |
| Model not found | "<Model> isn't available on <server>. Pick another in Settings > Brain." |
| No first token in 120 s (a local model loading counts) | "No answer from <model> in two minutes." |

A local server that is down is looked for again 20 s later, then less often
up to every 5 minutes, as Claude Code is when it is signed out. No retries
on 4xx; one retry on a dropped connection.

## 3. Settings

A new top-level block in `settings.json` (a missing block means Claude
Code, so existing files need no migration):

```json
"brain": {
  "kind": "claude_code",
  "provider": "ollama",
  "base_url": "http://127.0.0.1:11434/v1",
  "model": "qwen3:8b",
  "vision": "auto",
  "context_tokens": null,
  "tested": [
    { "base_url": "...", "model": "...", "ok": true, "note": "", "at": 1794840000 }
  ]
}
```

`tested` keeps one result per base URL and model, the 20 most recent.

`kind` is `claude_code` or `loop`. Claude Code's own settings (model,
effort, permissions, brain folder) stay where they are and apply only to
it. A change of brain restarts the voice loop with a fresh conversation.
Validation: `base_url` is http or https; `vision` is auto, on or off;
`context_tokens` is empty or 2048 to 2,000,000.

## 4. Setup and Settings

Setup's first step becomes **Choose a brain**:

- **Claude Code**: as today (installed? signed in?).
- **A model on this computer**: Pindot looks for Ollama, LM Studio and
  llama.cpp on their usual ports (loopback only, a short timeout each) and
  lists the models each one has. None found: a line on how to start one,
  and Check again.
- **An API key**: pick a provider, paste the key, pick a model from the
  provider's list.

Settings gets a **Brain** section with the same choices, the vision and
context controls, the model test and its result. With a loop brain, the
Claude section (model, effort, permissions) and the brain folder say they
apply to Claude Code only.

Status lines and notices name the brain ("Ollama · qwen3:8b") where they
now say Claude Code. The sidebar's "for Claude Code" subtitle is removed.

### The model test

Run when a model is chosen, and from a Test button. It runs the loop
directly, not through the voice loop, with a session id of its own that is
ended on the server afterwards (`ServerState::end_session`), and writes no
conversation file. Two turns:

1. "Reply with the single word: ready." Passes if the reply contains
   "ready".
2. "Call the desktop_context tool, then say how many displays there are."
   Passes if the model calls `desktop_context` (read-only, needs no
   approval) and then answers.

The result (passed, or which step failed and why in one sentence, such as
"It answered but didn't call the tool: this model may not support tools")
is saved in `brain.tested` for that base URL and model, and shown in
Setup and Settings. A model that failed can be kept, with the warning shown
next to it.

## 5. Docs and rules

- **CLAUDE.md**, hard rules: the Claude Code brain stays the interactive
  `claude` CLI in a PTY (never `-p`, `--print`, the Agent SDK, or an API
  key for Claude Code). Pindot's own loop is for the other providers, and
  their keys live only in the OS credential store. Plus a "Pindot's own
  loop" section with the decisions above.
- **CONTRIBUTING.md**: the "no API key" line changes to say that Claude Code
  must stay interactive and that keys for other providers go through
  `secrets.rs` only.
- **README**: "What you need" lists the three kinds of brain; Privacy says
  where each sends data (a local model: nothing leaves the computer; an API
  provider: what Claude would see goes to that provider instead); a short
  section on choosing a brain and the model test.
- **docs/development.md**: the loop's parts and how to test against a local
  server.

## 6. Testing

Unit tests (no network, no real model):

- A fake Chat Completions server in the tests (`axum`, already a
  dependency) playing scripted streams: a plain reply; a tool call then a
  reply; two tool calls in one message; an image result to a model that
  can see; a stream cut off; 401, 402, 429 and a context-too-long error.
- The loop against Pindot's real server bound to 127.0.0.1:0 with the mock
  platform (`platform/mock.rs`): tools are listed and called with the
  session header, and the approval gate still refuses an unapproved app.
- Esc during a stream and between tool calls.
- Context fitting: the order things are dropped in.
- The tool list without `screenshot` and `zoom` when vision is off.
- The conversation file: every line it writes parses with `parse_line` into
  the expected `ChatItem`s.
- Settings: the missing block reads as Claude Code; validation.
- Keys: an in-memory store in tests; a key never appears in a log line,
  an error or a conversation file.

By hand on this Mac: Ollama with a small model that uses tools, driving the
Swift test window (`spikes/test-target`, auto-approved in a test build):
press its button by ref, from a spoken request and from Chat. Then the
same with one API provider if Tony has a key to spare.

## Not in this spec

The Anthropic API (spec 3), Codex CLI (spec 4), shell, file or web tools
for a loop brain, the brain folder for a loop brain, a backup brain, a
different brain per conversation, and streaming speech word by word (each
complete message is spoken, as today).
