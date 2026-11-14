# Deskhand

Deskhand lets the Claude Code CLI you already use see and operate your desktop,
driven by your voice. It is a Tauri 2 app that does three jobs:

- It hosts a local MCP server that gives `claude` desktop tools (the hands).
- It shows an on-screen AI cursor.
- It runs your own signed-in `claude` interactively in a pseudo-terminal (the
  brain).

It never uses an API key and never runs `claude -p`. License: MIT.

**Status:** Phase 0 (spikes), Phase 1 (15 desktop tools, the accessibility
tree and the guardrails) and Phase 2 (the voice loop) are done, macOS only.
There is no main window or tray yet. The evidence is in `docs/phase0/`,
`docs/phase1/` and `docs/phase2/`. Conventions and decisions are in
`CLAUDE.md`.

## Build

You need Rust (stable), the Xcode command line tools and `claude` (signed in
with `claude login`).

```sh
cargo build --workspace
cargo test --workspace
swiftc -O -o spikes/test-target/test-target spikes/test-target/main.swift
```

## Phase 1: the desktop tools (macOS)

`./target/debug/deskhand` starts the MCP server on 127.0.0.1:47823 with the
tools `desktop_context`, `request_access`, `open_app`, `focus_window`,
`read_window`, `find`, `screenshot`, `zoom`, `click`, `type_text`, `key`,
`scroll`, `drag`, `wait` and `point`. Claude must ask for each app with
`request_access`, and you answer in Deskhand's own window ("Allow for this
session" or Deny). Press Esc while Claude is acting to stop everything.

```sh
./target/debug/deskhand --activity            # also open the live activity view
python3 scripts/mcp_call.py init              # call the tools by hand (one session)
python3 scripts/mcp_call.py call desktop_context '{}'
python3 scripts/mcp_call.py call request_access '{"apps":["TextEdit"],"reason":"try it"}'
python3 scripts/mcp_call.py end
```

To use the tools from your own `claude`, run it from a scratch folder with
`--mcp-config "$HOME/Library/Application Support/Deskhand/claude-mcp.json"
--strict-mcp-config --allowedTools mcp__desktop`.

The done-when check runs one real interactive `claude` turn against the app.
It is test-only: `DESKHAND_TEST_AUTO_APPROVE` approves the named apps without
asking you, and the app prints a loud warning while it is set.

```sh
DESKHAND_TEST_AUTO_APPROVE=TextEdit ./target/debug/deskhand --model haiku \
  --permission-mode default --accept-trust --test-turn 'Open TextEdit, ...'
```

The activity log is `~/Library/Application Support/Deskhand/activity.jsonl`.

## Phase 2: the voice loop (macOS)

```sh
cargo build --release
./target/release/deskhand --voice      # hold Fn, speak, release; Esc stops everything
```

Hold Fn and speak. Moonshine (speech to text) and Phonon (text to speech) run
on your Mac; their weights (about 231 MB) download to
`~/Library/Application Support/Deskhand/models/` on first use and are
checked against pinned SHA-256 sums. Your request is typed into one
long-running interactive `claude`, and its replies are spoken. Grant the
Microphone permission when macOS asks. Manual test steps and the test-only
`--test-voice` path are in `docs/phase2/README.md`.

## Phase 0 spikes (macOS)

Run the spikes from Terminal. macOS then asks for permissions on behalf of
Terminal, and you grant them under System Settings > Privacy & Security. The
test window is `spikes/test-target/test-target --x 300 --y 250 --seconds 300 --activate --log /tmp/dh-target.log`.
It logs every click and every Fn event it receives.

**(a) Hold Fn without triggering the Globe action** (needs Input Monitoring
and Accessibility):

```sh
./target/debug/spike_hotkey status              # shows which permissions are granted
./target/debug/spike_hotkey listen --seconds 20 # hold and release Fn: prints "Fn Down" / "Fn Up", the Globe action does not fire
./target/debug/spike_hotkey listen --pass-through --seconds 20   # control: Globe acts again
./target/debug/spike_hotkey selftest --control  # synthetic Fn events, no keyboard needed
```

**(b) AI cursor overlay** (clicks pass through it; needs Accessibility for
the click and Screen Recording for the screenshot):

```sh
./target/debug/deskhand --demo --exit-after 30   # cursor flies to the centre and corners of each monitor
./target/debug/deskhand --demo --capturable      # same, but screenshots can see the cursor
python3 spikes/mcp-call.py screenshot '{}' click '{"x": 347, "y": 250}'   # while the app runs
```

**(c) Screenshot and click from a normal `claude` session:** start
`./target/debug/spike_mcp --seconds 600` or the app. Then run this in a second
terminal:

```sh
mkdir -p .spike-cwd && cd .spike-cwd
claude --mcp-config "$HOME/Library/Application Support/Deskhand/claude-mcp.json" \
       --strict-mcp-config --allowedTools mcp__desktop
```

Then ask: "Take a screenshot and click the Deskhand Test Button."

**(d) Interactive `claude` in a PTY, answer through the Stop hook and the
transcript:**

```sh
./target/debug/spike_brain --prompt 'Reply with exactly the word: pong' --model haiku --accept-trust
./target/debug/spike_brain --desktop --model haiku --accept-trust \
  --prompt 'Take a screenshot with the desktop tool, then click the button labelled Deskhand Test Button, then reply with just: done'
```

Leave out `--accept-trust` the first time to see the folder-trust handling.
Accepting it adds the scratch folder to `~/.claude.json`.

## Windows

Windows is stubbed (trait stubs that return "not supported yet"). See
`docs/windows-test-checklist.md`.
