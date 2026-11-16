# Pindot design brief

**Point of view.** A small, friendly tool that works on your own computer: warm paper (or deep ink) surfaces, one hot pink used sparingly, and Rusty shown where something is alive (empty chat, status pill) instead of sprinkled as decoration. Sidebar is ink in both themes, so the brand is always anchored left. Interface text is plain and short.

**Identity (fixed).** Ink `#15151B`. Rusty pink `#FF2D95` is the brand fill: primary buttons, the active-item dot, badges, the mark. The cursor-colour setting never touches the app accent. Rusty mark from `docs/brand`, with moving eyes (`Rusty.svelte`). Wordmark in Poppins SemiBold.

**Type.** Schibsted Grotesk (variable 400-900) for all UI; Poppins SemiBold for the wordmark only; system mono for code and paths. Scale: 13 / 14 / 15 (body) / 17 / 22 / 30 px. Nothing below 13px. Titles 650 weight, tight tracking (-0.015em); body 1.5 line height.

**Colour tokens** (`app/src/app.css`, light then dark): page `#F4F2EE` / `#1B1B22`; card `#FFF` / `#23232C`; text `#15151B` / `#ECECF2`; muted `#5C5C69` / `#A8A8B8`; link and pink text `#B0005D` / `#FF7BBD`; field edge `#857F74` / `#74748A`; ok, bad, warn each with a wash. Pink as a text colour is never used on light (3.4:1 fails); buttons are pink with ink text (5.25:1).

**Space, radius, elevation.** 4px steps (4 8 12 16 24 32). Controls 34px high. Radii 6 (controls), 10 (cards), 14 (chat bubbles). Two shadows only: a hairline for cards, one lift for approval cards.

**Controls.** Primary: pink fill, ink text. Secondary: card fill, strong edge. Ghost: accent text. Danger: red edge. Focus: 2px ring in a pink that passes 3:1 on both themes, offset 2px. Fields have a 3:1 edge.

**Motion.** 140ms ease for hover and press. Rusty blinks every 6s when idle, looks side to side when thinking or acting, widens when listening. The listening dot and mic ring follow the mic level. All of it stops under `prefers-reduced-motion` (a static outline replaces the ring).

## Contrast, measured (WCAG, computed in Python)

Text on page, card and wash backgrounds: 5.6 to 16.3 in light, 6.3 to 14.6 in dark (lowest: error red on its wash, 5.58 light; accent on wash, 6.31 dark). Ink on pink 5.25. Sidebar muted text on ink 7.85. Field edges 3.4 to 4.1. Focus ring 4.78 light, 6.01 dark.

## Antislop findings (skill run on the served mock, then by hand against v3.2.23)

| # | Rule | Finding | Status |
|---|------|---------|--------|
| 1 | R-06 fonts | Old stack was system fonts; no banned fonts now (Schibsted Grotesk, Poppins). Audit: slopFonts none | Fixed |
| 2 | R-02 | No em dashes in UI text or docs. Audit: emDash 0, grep 0 | Fixed |
| 3 | Part 1 look | Purple/blue accent `#2d6cdf` replaced by brand pink. Gradients 0, glass 0, glows 0, emoji 0 | Fixed |
| 4 | Part 1 layout | Empty chat was centred boilerplate; now left aligned, Rusty plus one plain line | Fixed |
| 5 | Copy | Wordy empty, dictation and Setup lines shortened | Fixed |
| 6 | Contrast | Old 11px footer text, 12px small text; now 13px minimum. axe: no violations, contrastToReview 0 | Fixed |
| 7 | Field edges | Input borders under 3:1 in first pass (1.8); added `--field-line` | Fixed |
| 8 | Layout | Vault use-history table overflowed its row; Activity "Clear" wrapped | Fixed |
| 9 | R-03 tap size | Buttons are 34px high (36 for nav), under the 44px web rule. This is a mouse-and-keyboard desktop window; 26px "small" buttons stay for inline actions | Not fixed, owner call |
| 10 | Mobile overflow | 320px overflow (70 elements) | Not applicable: the window is 820px wide at minimum, no mobile target |
| 11 | Controls | Audit "no effect" on Chat (already on it) and Copy (clipboard, no DOM change) | False positive, checked by hand |
| 12 | Console | One TypeError in the mock-only run, from a command the mock does not answer | Not the app; mock gap |
| 13 | R-10 glass | None used | Pass |

## Fonts and licences

Schibsted Grotesk (Schibsted Media Group) and Poppins (Indian Type Foundry), both SIL Open Font License 1.1, which allows bundling. Latin subsets, woff2, in `app/src/fonts/` with `OFL.txt`. 40 KB total. Nothing loads from the network.
