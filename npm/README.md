# pindot

This package installs **[Pindot](https://pindot.click)**, a free, open-source
(MIT) desktop app for macOS and Windows. Hold a key, say what you want, and
an AI does it on your computer and answers out loud. You can also type to it
in the chat window. While it works, its own on-screen cursor shows you every
click.

```sh
npx pindot
```

That one command downloads the ready-made Pindot for your computer from the
project's [GitHub releases](https://github.com/projectservan8n/Pindot/releases),
checks it against the SHA-256 pinned in this package, installs it and opens
it. Pindot's Setup window then walks you through the rest. Run the same
command again later to update.

## What it does on each system

- **Windows 11** (Windows 10 should work but hasn't been tried): runs
  Pindot's installer silently. It installs for
  your user only, so it needs no administrator rights. You get a Start menu
  entry and an entry under Installed apps.
- **macOS 13 or later, Apple silicon**: copies `Pindot.app` out of the disk
  image into `/Applications`, or into `~/Applications` if you can't write to
  `/Applications`.

Pindot itself is not inside this package. The package holds only the
installer script and the pinned checksums, and a file that doesn't match its
checksum is deleted before anything runs.

## Commands

```sh
npx pindot                    # install or update, then open
npx pindot install --no-open  # install or update only
npx pindot install --dir <folder>
npx pindot install --reinstall
npx pindot uninstall
npx pindot --version
```

Quit Pindot (its tray or menu-bar icon > Quit Pindot) before you update or
uninstall it.

Updating or uninstalling keeps your settings, voice models and saved logins:
- **Windows:** `%APPDATA%\Pindot`
- **macOS:** `~/Library/Application Support/Pindot`

Delete that folder to remove them too.

## What you need

- Node.js 20 or later, only for this installer. Pindot doesn't use Node.
- A brain for Pindot to use. That can be Claude Code, your ChatGPT plan
  through Codex, a model running on your own computer, or an API key. See
  [What you need](https://github.com/projectservan8n/Pindot#what-you-need).

Pindot is an independent project, not made by, endorsed by or affiliated
with Anthropic or OpenAI.
