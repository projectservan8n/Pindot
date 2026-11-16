import { describe, expect, it } from 'vitest';
import { parseInline, parseMarkdown, safeHref, type Block, type Inline } from './markdown';

/** Every string in the tree, to check what could ever reach the page. */
function texts(n: unknown, out: string[] = []): string[] {
  if (Array.isArray(n)) n.forEach((x) => texts(x, out));
  else if (n && typeof n === 'object') {
    for (const [k, v] of Object.entries(n)) {
      if ((k === 'text' || k === 'href') && typeof v === 'string') out.push(v);
      else texts(v, out);
    }
  }
  return out;
}
const kinds = (n: unknown, out = new Set<string>()): Set<string> => {
  if (Array.isArray(n)) n.forEach((x) => kinds(x, out));
  else if (n && typeof n === 'object') {
    const t = (n as { t?: string }).t;
    if (t) out.add(t);
    Object.values(n).forEach((v) => kinds(v, out));
  }
  return out;
};

describe('safe links', () => {
  it('allows http, https and mailto only', () => {
    expect(safeHref('https://example.com/a?b=1')).toBe('https://example.com/a?b=1');
    expect(safeHref('http://example.com')).toBe('http://example.com');
    expect(safeHref('mailto:a@b.co')).toBe('mailto:a@b.co');
    for (const bad of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,<b>', 'file:///etc/passwd', 'tauri://localhost', '//evil.com', '/relative', 'java\nscript:alert(1)', ' javascript:x', 'https://a b']) {
      expect(safeHref(bad), bad).toBeNull();
    }
  });
  it('turns an unsafe link into plain text, not a link', () => {
    const k = parseInline('[click](javascript:alert(1))');
    expect(kinds(k).has('link')).toBe(false);
    expect(texts(k).join('')).toContain('javascript:alert(1)');
  });
  it('makes a link from a safe one and from a bare address', () => {
    const k = parseInline('see [docs](https://example.com/x) and https://a.dev/y.');
    const links = k.filter((x): x is Extract<Inline, { t: 'link' }> => x.t === 'link');
    expect(links.map((l) => l.href)).toEqual(['https://example.com/x', 'https://a.dev/y']);
  });
});

describe('no raw html, no images', () => {
  it('keeps a script tag as visible text', () => {
    const b = parseMarkdown('<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>');
    expect(kinds(b)).toEqual(new Set(['p', 'text']));
    expect(texts(b).join('|')).toContain('<script>alert(1)</script>');
    expect(texts(b).join('|')).toContain('<img src=x onerror=alert(1)>');
  });
  it('never produces an image node and does not load one', () => {
    const b = parseMarkdown('![alt](https://evil.example/pixel.png)');
    expect(kinds(b).has('img')).toBe(false);
    expect(JSON.stringify(b)).not.toContain('"img"');
  });
  it('node kinds are a closed set', () => {
    const md = '# T\n\n- a\n- b\n\n1. x\n\n> q\n\n```js\nlet a = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n**b** *i* `c` [l](https://x.io)';
    const allowed = new Set(['p', 'h', 'code', 'list', 'quote', 'table', 'hr', 'text', 'strong', 'em', 'link']);
    for (const k of kinds(parseMarkdown(md))) expect(allowed.has(k), k).toBe(true);
  });
});

describe('structure', () => {
  it('reads a code block verbatim, including markup inside', () => {
    const b = parseMarkdown('Here:\n\n```html\n<b>**not bold**</b>\n```\nafter');
    const c = b.find((x): x is Extract<Block, { t: 'code' }> => x.t === 'code')!;
    expect(c.lang).toBe('html');
    expect(c.text).toBe('<b>**not bold**</b>');
    expect(b.at(-1)?.t).toBe('p');
  });
  it('an unclosed fence runs to the end without throwing', () => {
    const b = parseMarkdown('```\nabc\ndef');
    expect(b).toEqual([{ t: 'code', lang: '', text: 'abc\ndef' }]);
  });
  it('reads bullet, numbered and nested lists', () => {
    const b = parseMarkdown('- one\n- two\n  - inner\n- three\n\n3. c\n4. d');
    const l = b[0] as Extract<Block, { t: 'list' }>;
    expect(l.ordered).toBe(false);
    expect(l.items).toHaveLength(3);
    expect(l.items[1].some((x) => x.t === 'list')).toBe(true);
    const n = b[1] as Extract<Block, { t: 'list' }>;
    expect(n.ordered && n.start).toBe(3);
  });
  it('reads headings, quotes, rules and tables', () => {
    const b = parseMarkdown('## Hi\n\n> quoted\n\n***\n\n| a | b |\n|:-|-:|\n| 1 | 2 |');
    expect(b.map((x) => x.t)).toEqual(['h', 'quote', 'hr', 'table']);
    expect((b[0] as { level: number }).level).toBe(2);
    expect((b[3] as Extract<Block, { t: 'table' }>).rows).toHaveLength(1);
  });
  it('does not treat snake_case or a lone star as emphasis', () => {
    expect(texts(parseInline('use my_var_name here 2 * 3'))).toEqual(['use my_var_name here 2 * 3']);
  });
  it('handles odd and hostile input without throwing', () => {
    for (const s of ['', '[', '](', '[a](', '**', '`', '```', '> '.repeat(50), '- '.repeat(50), '* * *', '|', '|---|', '\u0000', '[a]('.repeat(2000), '- a\n'.repeat(5000)]) {
      expect(() => parseMarkdown(s), s.slice(0, 10)).not.toThrow();
    }
  });
});
