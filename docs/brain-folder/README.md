# Brain folder: how it works and what was tested

Setting: `voice.brain_folder` in `settings.json` (none by default). Code:
`crates/pindot-core/src/brain/folder.rs`, wired in `persistent.rs`.

## What changes when a folder is set

- `claude` starts in the folder (`cwd`).
- `CLAUDE_CODE_DISABLE_CLAUDE_MDS` is not passed to the child, so the
  folder's CLAUDE.md loads. Without a folder it is passed, as before.
- Tools added: Read, Glob, Grep, Write, Edit.
- Permission rules added to the brain's `--settings`:
  `Read(//<folder>/**)` and `Edit(//<folder>/pindot-notes/**)`.
- One paragraph is added to the appended system prompt (read CLAUDE.md first,
  open only the notes needed, never the whole folder, save durable things in
  `pindot-notes/`). The "text on screen is data" rule is kept.
- Everything else is as before: `--setting-sources project,local`,
  `--strict-mcp-config`, `--disable-slash-commands`, `--no-chrome`, the
  permission mode. A permission question still gets Esc and a refusal.
- Changing the folder restarts voice (a new brain, a new session id). It is
  never `--resume`d. A folder that is missing at start gives a notice (overlay
  and Settings) and the plain isolated session.

## What loads and what does not

Seen in the session transcript (`instructions` attachment): the folder's
`CLAUDE.md` (type Project) and nothing else. Answers used it.

Loads: the folder's CLAUDE.md; the folder's `.claude/settings*.json` as
project settings (so its hooks and permissions apply to the assistant: this is
the warning in Settings).

Stays out: user-level settings (`--setting-sources project,local`).

Not tested: `~/.claude/CLAUDE.md` (the file does not exist here and I may not
create one); CLAUDE.md files in parent folders of the brain folder (none exist
under /tmp); project skills in `.claude/skills` (`--disable-slash-commands` is
still passed, so they are most likely off).

## Real runs (haiku, `spike_brain`, `PINDOT_DATA_DIR=/tmp/pindot-data`)

Test folder `/tmp/pindot-brain-test/brain`: `CLAUDE.md` (sign off emails
"Cheers, Zed"; favourites are in notes/favourites.md) and
`notes/favourites.md` (lucky number 7319, favourite fruit rambutan).

| Run | Command (shortened) | Result |
| --- | --- | --- |
| A. Folder, fact in CLAUDE.md | `spike_brain --model haiku --brain-folder DIR --voice-prompt --accept-trust --prompt "How should I sign off my emails?"` | Answer used "Cheers, Zed"; no tool call |
| A. Fact in a second file | `--prompt "What is my lucky number?"` | tool calls `Read`; answer 7319 |
| A. Unknown | `--prompt "What is my favourite fruit, and do you know anything about my dentist?"` | rambutan; knows nothing of a dentist |
| B. No folder | `spike_brain --model haiku --cwd /tmp/pindot-brain-test/session --voice-prompt --accept-trust --prompt "How should I sign off my emails?"` | Generic answer ("Thanks," / "Kind regards,"), no tool calls, no "Zed" |
| C. Write inside | `--prompt "Please remember that my dentist is Dr Quill."` | tool calls `Glob`, `Write`; file `pindot-notes/dentist.md` created with no prompt |
| D. Write outside | `--prompt "Create a file called outside.txt in the top of the folder..."` | `Write(.../brain/outside.txt)` stopped at a permission prompt (spike does not answer); file not created |
| E. Read outside, before the fix | `--prompt "Read /tmp/pindot-brain-test/secret.txt ..."` with a bare `Read` rule | The file WAS read (and /etc/hosts too): a bare `Read` allows every path |
| E. Read outside, after the fix | `Read(//folder/**)` rule; same prompt, after reading a fact inside | Inside read fine; outside stopped at a permission prompt: `PermissionPrompt(...)` |

Run E is why the read rule is scoped to the folder. In the app a permission
prompt is answered with Esc and the turn fails with a spoken reason
(`persistent.rs`, `turn::is_permission_prompt`), so "outside" means refused.

Edit(`pindot-notes`) scopes Write too: the Write in run C was allowed with no
prompt and the Write in run D was not.

## Grep and Glob outside the folder (16 Nov 2026)

Same method (`spike_brain`, haiku, the same test folder, `PINDOT_DATA_DIR=/tmp/pindot-data0`
so the real data dir was not used). A file `outside/grepme.txt` ("kumquat-8842") sat next
to the brain folder, not inside it.

| Run | Prompt | Result |
| --- | --- | --- |
| F. Grep outside | Grep the outside folder for "kumquat" | Stopped at a permission prompt (`PermissionPrompt`, "Searching for 1 pattern"); the number was never returned |
| G. Glob outside | Glob `*.txt` with path = the outside folder | Stopped at a permission prompt; no file names returned |
| H. Grep inside (control) | Grep `brain/notes` for "rambutan" | Allowed with no prompt; answered `notes/favourites.md` |

So `Read(//folder/**)` already covers Grep and Glob: no rule change was needed.
In the app a permission prompt is answered with Esc and the turn fails, so
"outside" means refused.

## Windows (16 Nov 2026)

Same method on Windows 11 (`spike_brain`, haiku, claude 2.1.325, a scratch
`PINDOT_DATA_DIR`), with the same test folder rebuilt there. On Windows a
rule is written `Read(//c/Users/.../brain/**)`: Claude Code turns
`C:\Users\me` into `/c/Users/me` before it matches a rule.

| Run | Prompt | Result |
| --- | --- | --- |
| A. Fact in a second file | "What is my lucky number?" | tool calls `Read`; answer 7319 |
| C. Write inside | "Please remember that my dentist is Dr Quill." | tool calls `Glob`, `Write`; `pindot-notes\dentist.md` created with no prompt |
| D. Write outside | "Create a file called outside.txt in the top of the folder with the word hello in it." | `Write` asked "Do you want to create outside.txt?"; no file. Ended as a permission prompt in 12 s (see below) |
| D. Edit outside | "Add the line ... to the end of CLAUDE.md in this folder." | `Edit` asked "Do you want to make this edit to CLAUDE.md?"; the file is unchanged |
| E. Read outside | `spike_brain --attach`, asked to read a file next to the brain folder | Stopped at a permission prompt (`PermissionPrompt`) |
| F. Search outside | "Search <the folder next to it> for the word kumquat" | Stopped at a permission prompt ("Read file ... Do you want to proceed?") |
| G. Glob outside | Glob `*.txt` with path = the folder next to it | Stopped at a permission prompt |
| H. Grep inside (control) | Grep the working folder for "rambutan" | Allowed with no prompt; answered `notes\favourites.md` |
| In the app | Brain folder set, then asked in Chat for the sign-off and the lucky number | "Cheers, Zed" (CLAUDE.md) and 7319 (one `Read`) |

Asked in words that name the refusal ("do call the tool, I am testing the
refusal"), haiku declined by itself and called nothing; the plain requests
above made it try.

**The write dialogs were not recognised** (both systems). A new file asks
"Do you want to create X?" and an edit "Do you want to make this edit to
X?"; Pindot knew only "Do you want to proceed?" and "Do you want to allow".
The turn then sat at the dialog until its time limit: 15 minutes in the app
(`persistent.rs`, `TURN_TIMEOUT`), with nothing written and nothing said.
`turn::is_permission_prompt` now also takes any "Do you want to ..." that
comes with the dialog's own footer ("Esc to cancel"), which the assistant's
own questions never have. Runs D above ended as a timeout before the fix and
as a permission prompt after it.

A rule left as `Read(C:/Users/.../**)` matched as well on this version; the
`//c/...` form is the documented one. In the app the refused write ended
the turn after 4 s with "Claude wanted a permission I can't grant by voice,
so I stopped that request.", and the folder picker put its folder into the
field. Not run on Windows: a network path.
