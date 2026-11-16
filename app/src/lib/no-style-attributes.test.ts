/// <reference types="vite/client" />
import { describe, expect, it } from 'vitest';

// The app's content security policy plus Tauri's nonce make the real (WebKit)
// window ignore every style attribute written in markup, in an HTML file, in
// an innerHTML string, or as a Svelte string `style={...}`. Headless Chrome
// without the policy shows them fine, which is how this slipped through once.
// Use classes, or `style:prop={value}` in Svelte, or `el.style.x = ...` in JS.
//
// The files are read through Vite (`?raw`), not with Node's `fs`: the type
// check (`npm run check`) has no Node types and failed on them, and the names
// come out with forward slashes on Windows too.
const files = import.meta.glob<string>(
  ['../**/*.{svelte,ts,js,html,css}', '../../public/**/*.{svelte,ts,js,html,css}', '!../**/*.test.ts'],
  { query: '?raw', import: 'default', eager: true },
);
const bad = [
  /\bstyle\s*=\s*["'`{]/,
  /\bstyle\s*=\\["']/,
  /setAttribute\(\s*['"]style['"]/,
];

describe('no style attributes in markup', () => {
  // A pattern that matched no file would make the next test pass for nothing.
  it('reads the files of app/src and app/public', () => {
    const names = Object.keys(files);
    // Names are relative to this file: `./api.ts`, `../App.svelte`.
    for (const want of ['/App.svelte', '/views/Chat.svelte', './api.ts', '/app.css', '/public/overlay.html', '/public/overlay.js']) {
      expect(names.some((n) => n.endsWith(want)), `${want} is read`).toBe(true);
    }
    expect(names.some((n) => n.endsWith('.test.ts')), 'the tests themselves are left out').toBe(false);
    expect(files[names.find((n) => n.endsWith('/App.svelte'))!]).toContain('<script');
  });

  it('finds none in app/src or app/public', () => {
    const hits: string[] = [];
    for (const [name, text] of Object.entries(files)) {
      text.split('\n').forEach((line, i) => {
        if (bad.some((re) => re.test(line))) hits.push(`${name}:${i + 1}: ${line.trim().slice(0, 100)}`);
      });
    }
    expect(hits, 'style attributes are dropped by the app\'s security policy; use a class or style:prop').toEqual([]);
  });
});
