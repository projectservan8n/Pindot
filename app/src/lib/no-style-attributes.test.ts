import { describe, expect, it } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

// The app's content security policy plus Tauri's nonce make the real (WebKit)
// window ignore every style attribute written in markup, in an HTML file, in
// an innerHTML string, or as a Svelte string `style={...}`. Headless Chrome
// without the policy shows them fine, which is how this slipped through once.
// Use classes, or `style:prop={value}` in Svelte, or `el.style.x = ...` in JS.
const root = resolve(__dirname, '../..');
const dirs = [join(root, 'src'), join(root, 'public')];
const exts = ['.svelte', '.ts', '.js', '.html', '.css'];
const bad = [
  /\bstyle\s*=\s*["'`{]/,
  /\bstyle\s*=\\["']/,
  /setAttribute\(\s*['"]style['"]/,
];

function walk(d: string, out: string[] = []): string[] {
  for (const n of readdirSync(d)) {
    const p = join(d, n);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (exts.some((e) => n.endsWith(e)) && !n.endsWith('.test.ts')) out.push(p);
  }
  return out;
}

describe('no style attributes in markup', () => {
  it('finds none in app/src or app/public', () => {
    const hits: string[] = [];
    for (const f of dirs.flatMap((d) => walk(d))) {
      readFileSync(f, 'utf8').split('\n').forEach((line, i) => {
        if (bad.some((re) => re.test(line))) hits.push(`${f.replace(root + '/', '')}:${i + 1}: ${line.trim().slice(0, 100)}`);
      });
    }
    expect(hits, 'style attributes are dropped by the app\'s security policy; use a class or style:prop').toEqual([]);
  });
});
