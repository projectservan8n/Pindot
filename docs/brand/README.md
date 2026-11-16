# Pindot brand files

The mark is Rusty, the AI cursor with eyes: the same cursor Pindot shows on
screen, given a face because the assistant has a name.

- `pindot-symbol-*.svg`: Rusty alone, in black, white and pink.
- `pindot-symbol-small-*.svg`: the same shape without eyes, for 24 px and below.
- `pindot-lockup-*.svg`: Rusty with the wordmark. Use `colour-on-light` and
  `colour-on-dark` first; black and white are for one-colour printing.
- `pindot-app-icon*.svg`: the macOS icon (dark tile with a margin).
- `pindot-win-icon*.svg`: the Windows icon (the tile fills the square).
- `pindot-tray*.svg`: the menu bar and tray icon (a template image on macOS).

Colours: pink `#FF2D95` (the overlay's accent), ink `#15151B`, paper `#F5F5F7`.

The wordmark is Poppins SemiBold (SIL Open Font License 1.1) converted to
outlines, so no font file is needed to show it.

## Rebuilding

`python3 build.py` writes every SVG here from a few numbers at the top of the
script (it needs `fonttools` and Poppins SemiBold in `~/Library/Fonts`). The
files in `app/src-tauri/icons/` are renders of these: `icon.png` is the app
icon at 512 px, `icon.ico` is the Windows icon at 256 to 32 px plus the
eyeless cut at 24 and 16 px, and `tray.png` is the tray icon at 44 px.

No trademark search has been done on the name or the mark.
