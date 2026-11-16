// A small, safe Markdown reader for the chat. It builds a tree that Svelte
// renders as plain elements, so no HTML string is ever injected: raw HTML in a
// reply stays visible text, images are not fetched, and a link only shows its
// address (http, https and mailto only) and is never followed from here.

export type Inline =
  | { t: 'text'; text: string }
  | { t: 'code'; text: string }
  | { t: 'strong'; kids: Inline[] }
  | { t: 'em'; kids: Inline[] }
  | { t: 'link'; href: string; kids: Inline[] };

export type Block =
  | { t: 'p'; kids: Inline[] }
  | { t: 'h'; level: number; kids: Inline[] }
  | { t: 'code'; lang: string; text: string }
  | { t: 'list'; ordered: boolean; start: number; items: Block[][] }
  | { t: 'quote'; kids: Block[] }
  | { t: 'table'; head: Inline[][]; rows: Inline[][][] }
  | { t: 'hr' };

/** The address of a link if it is safe to show as a link; otherwise null. */
export function safeHref(raw: string): string | null {
  const s = raw.trim();
  if (/[\u0000-\u001f\u007f\s]/.test(s)) return null;
  try {
    const u = new URL(s);
    if (u.protocol === 'http:' || u.protocol === 'https:' || u.protocol === 'mailto:') return s;
  } catch {
    /* not a URL */
  }
  return null;
}

const ESCAPABLE = '\\`*_{}[]()#+-.!|>~<';

export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let buf = '';
  const flush = () => {
    if (buf) out.push({ t: 'text', text: buf });
    buf = '';
  };
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === '\\' && i + 1 < src.length && ESCAPABLE.includes(src[i + 1])) {
      buf += src[i + 1];
      i += 2;
    } else if (c === '`') {
      let n = 1;
      while (src[i + n] === '`') n++;
      const fence = '`'.repeat(n);
      const end = src.indexOf(fence, i + n);
      if (end < 0) {
        buf += fence;
        i += n;
      } else {
        flush();
        out.push({ t: 'code', text: src.slice(i + n, end).replace(/^ (.*) $/, '$1') });
        i = end + n;
      }
    } else if ((c === '*' || c === '_') && src[i + 1] === c) {
      const end = src.indexOf(c + c, i + 2);
      if (end > i + 2 && src[i + 2] !== ' ') {
        flush();
        out.push({ t: 'strong', kids: parseInline(src.slice(i + 2, end)) });
        i = end + 2;
      } else {
        buf += c + c;
        i += 2;
      }
    } else if ((c === '*' || c === '_') && src[i + 1] !== ' ' && src[i + 1] !== undefined) {
      // An underscore inside a word (snake_case) is not emphasis.
      const prev = src[i - 1];
      const intraword = c === '_' && prev !== undefined && /[A-Za-z0-9]/.test(prev);
      const end = intraword ? -1 : src.indexOf(c, i + 1);
      if (end > i + 1 && src[end - 1] !== ' ') {
        flush();
        out.push({ t: 'em', kids: parseInline(src.slice(i + 1, end)) });
        i = end + 1;
      } else {
        buf += c;
        i++;
      }
    } else if (c === '!' && src[i + 1] === '[') {
      // Images are never loaded: show the text and the address instead.
      buf += '!';
      i++;
    } else if (c === '[') {
      const close = src.indexOf('](', i);
      const end = close < 0 ? -1 : src.indexOf(')', close + 2);
      if (close > i && end > close) {
        const label = src.slice(i + 1, close);
        const href = safeHref(src.slice(close + 2, end).split(/\s+"/)[0]);
        flush();
        if (href) out.push({ t: 'link', href, kids: parseInline(label) });
        else out.push({ t: 'text', text: src.slice(i, end + 1) });
        i = end + 1;
      } else {
        buf += c;
        i++;
      }
    } else if (c === '<' && /^<(https?:\/\/[^\s>]+|[^\s@>]+@[^\s@>]+)>/.test(src.slice(i))) {
      const m = /^<([^>]+)>/.exec(src.slice(i))!;
      const target = m[1].includes('@') && !m[1].includes(':') ? `mailto:${m[1]}` : m[1];
      const href = safeHref(target);
      flush();
      if (href) out.push({ t: 'link', href, kids: [{ t: 'text', text: m[1] }] });
      else out.push({ t: 'text', text: m[0] });
      i += m[0].length;
    } else if (c === 'h' && /^https?:\/\/[^\s<>()]+/.test(src.slice(i)) && !/[A-Za-z0-9]/.test(src[i - 1] ?? ' ')) {
      const m = /^https?:\/\/[^\s<>()]+/.exec(src.slice(i))![0].replace(/[.,;:!?]+$/, '');
      const href = safeHref(m);
      flush();
      if (href) out.push({ t: 'link', href, kids: [{ t: 'text', text: m }] });
      else out.push({ t: 'text', text: m });
      i += m.length;
    } else {
      buf += c;
      i++;
    }
  }
  flush();
  return out;
}

const isFence = (l: string) => /^ {0,3}(```+|~~~+)/.test(l);
const isHr = (l: string) => /^ {0,3}([-*_])( *\1){2,} *$/.test(l);
const bullet = (l: string) => /^( *)([-*+]) +(.*)$/.exec(l);
const numbered = (l: string) => /^( *)(\d{1,9})[.)] +(.*)$/.exec(l);
const isTableSep = (l: string) => /^ *\|?( *:?-{1,}:? *\|)+ *:?-*:? *\|? *$/.test(l) && l.includes('-');

function cells(l: string): string[] {
  let s = l.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, '|'));
}

export function parseMarkdown(src: string, depth = 0): Block[] {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const out: Block[] = [];
  let i = 0;
  const startsBlock = (l: string) =>
    isFence(l) || /^ {0,3}#{1,6} /.test(l) || isHr(l) || /^ {0,3}>/.test(l) || !!bullet(l) || !!numbered(l);
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const f = /^ {0,3}(```+|~~~+)\s*([^\s`]*)/.exec(line);
    if (f) {
      const fence = f[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !new RegExp(`^ {0,3}${fence[0]}{${fence.length},}\\s*$`).test(lines[i])) body.push(lines[i++]);
      i++; // the closing fence (or the end)
      out.push({ t: 'code', lang: f[2].replace(/[^A-Za-z0-9_+-]/g, '').slice(0, 20), text: body.join('\n') });
      continue;
    }
    const h = /^ {0,3}(#{1,6}) +(.*?)( +#+)? *$/.exec(line);
    if (h) {
      out.push({ t: 'h', level: h[1].length, kids: parseInline(h[2]) });
      i++;
      continue;
    }
    if (isHr(line)) {
      out.push({ t: 'hr' });
      i++;
      continue;
    }
    if (/^ {0,3}>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^ {0,3}>/.test(lines[i])) body.push(lines[i++].replace(/^ {0,3}> ?/, ''));
      out.push({ t: 'quote', kids: depth < 6 ? parseMarkdown(body.join('\n'), depth + 1) : [{ t: 'p', kids: [{ t: 'text', text: body.join('\n') }] }] });
      continue;
    }
    const first = bullet(line) ?? numbered(line);
    if (first) {
      const ordered = !bullet(line);
      const indent = first[1].length;
      const items: Block[][] = [];
      const start = ordered ? Number(first[2]) : 1;
      while (i < lines.length) {
        const m = (ordered ? numbered(lines[i]) : bullet(lines[i]));
        if (!m || m[1].length !== indent) break;
        const body: string[] = [m[3]];
        i++;
        // Continuation: deeper-indented lines (nested lists, wrapped text).
        while (i < lines.length) {
          const l = lines[i];
          if (!l.trim()) {
            if (i + 1 < lines.length && /^ {2,}\S/.test(lines[i + 1]) && lines[i + 1].search(/\S/) > indent) {
              body.push('');
              i++;
              continue;
            }
            break;
          }
          if (l.search(/\S/) > indent) {
            body.push(l.slice(Math.min(l.search(/\S/), indent + 2)));
            i++;
          } else break;
        }
        items.push(depth < 6 ? parseMarkdown(body.join('\n'), depth + 1) : [{ t: 'p', kids: [{ t: 'text', text: body.join('\n') }] }]);
      }
      out.push({ t: 'list', ordered, start, items });
      continue;
    }
    if (line.includes('|') && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      const head = cells(line).map(parseInline);
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && lines[i].trim() && lines[i].includes('|')) rows.push(cells(lines[i++]).map(parseInline));
      out.push({ t: 'table', head, rows });
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && (para.length === 0 || !startsBlock(lines[i]))) para.push(lines[i++].trim());
    out.push({ t: 'p', kids: parseInline(para.join('\n')) });
  }
  return out;
}
