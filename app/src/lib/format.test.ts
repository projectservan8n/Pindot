import { describe, expect, it } from 'vitest';
import {
  chordFromCodes,
  cursorPresets,
  eyeInk,
  isCursorColor,
  connectText,
  formatBytes,
  hotkeyPresets,
  hotkeyWords,
  passes,
  percent,
  permissionText,
  progressText,
  shortTime,
  timeParts,
  stepLabel,
} from './format';
import { errorText, type ActivityEntry } from './api';

describe('formatBytes', () => {
  it('uses decimal units like the model sizes in the docs', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(165_500_000)).toBe('165.5 MB');
    expect(formatBytes(65_100_000)).toBe('65.1 MB');
    expect(formatBytes(2_500_000_000)).toBe('2.5 GB');
    expect(formatBytes(-1)).toBe('?');
  });
});

describe('percent', () => {
  it('clamps and rounds', () => {
    expect(percent(0, 0)).toBe(0);
    expect(percent(50, 200)).toBe(25);
    expect(percent(300, 200)).toBe(100);
  });
});

describe('chordFromCodes (record a shortcut)', () => {
  it('keeps the side of a single key', () => {
    expect(chordFromCodes(['AltRight'])).toBe('Right Option');
    expect(chordFromCodes(['MetaRight'])).toBe('Right Cmd');
    expect(chordFromCodes(['ControlLeft'])).toBe('Left Ctrl');
  });
  it('makes a side-less chord of two or more keys, in a fixed order', () => {
    expect(chordFromCodes(['MetaLeft', 'ControlLeft'])).toBe('Ctrl+Cmd');
    expect(chordFromCodes(['ControlRight', 'AltLeft'])).toBe('Ctrl+Option');
    expect(chordFromCodes(['ShiftLeft', 'ShiftRight'])).toBe('Shift');
  });
  it('rejects anything that is not modifier-only', () => {
    expect(chordFromCodes(['ControlLeft', 'KeyQ'])).toBeNull();
    expect(chordFromCodes([])).toBeNull();
    expect(chordFromCodes(['Escape'])).toBeNull();
  });
});

describe('hotkeyPresets', () => {
  it('starts with the default for each OS', () => {
    expect(hotkeyPresets('macos')[0].value).toBe('Fn');
    expect(hotkeyPresets('windows')[0].value).toBe('Ctrl+Cmd');
    expect(hotkeyPresets('windows').some((p) => p.value === 'Fn')).toBe(false);
  });
});

describe('status text', () => {
  it('describes every permission state', () => {
    expect(permissionText('granted')).toBe('Allowed');
    expect(permissionText('not_asked')).toBe('Not asked yet');
    expect(permissionText('denied')).toBe('Denied');
    expect(permissionText('unknown')).toBe('Unknown');
  });
  it('describes download progress', () => {
    expect(
      progressText({ kind: 'downloading', file: 'a.ort', done: 1, total: 2, model_done: 82_750_000, model_total: 165_500_000 }),
    ).toBe('Downloading a.ort: 82.8 MB of 165.5 MB');
    expect(progressText({ kind: 'checksum_failed', file: 'a.ort' })).toContain('checksum mismatch');
    expect(progressText({ kind: 'resumed', file: 'a.ort', from: 1_000_000 })).toBe('Resuming a.ort from 1.0 MB');
  });
  it('describes the Claude Code connection', () => {
    expect(connectText({ state: 'connected' })).toMatch(/^Connected:/);
    expect(connectText({ state: 'outdated', reason: 'old token' })).toContain('Press Connect again');
    expect(connectText({ state: 'name_taken', detail: 'x' })).toContain('Remove it first');
  });
});

describe('activity helpers', () => {
  const e: ActivityEntry = { ts: '2026-11-14T09:12:03Z', session: 'ab12', tool: 'click', args: '', outcome: 'ok', ok: true };
  it('filters live entries like the server does', () => {
    expect(passes(e, '', '')).toBe(true);
    expect(passes(e, 'ab12', 'click')).toBe(true);
    expect(passes(e, 'zz', '')).toBe(false);
    expect(passes(e, '', 'type_text')).toBe(false);
  });
  it('shows the time, and the date when not today', () => {
    const d = new Date('2026-11-14T09:12:03Z');
    expect(shortTime(e.ts, d)).toMatch(/^\d\d:\d\d:03$/);
    expect(shortTime(e.ts, new Date('2027-01-01T12:00:00Z'))).toMatch(/^2026-11-1\d \d\d:\d\d:03$/);
    expect(shortTime('not a time')).toBe('not a time');
  });
  it('turns command errors into text', () => {
    expect(errorText(['Port: bad', 'Shortcut: bad'])).toBe('Port: bad\nShortcut: bad');
    expect(errorText('nope')).toBe('nope');
    expect(errorText(new Error('boom'))).toBe('boom');
  });
});

describe('cursor colour helpers', () => {
  it('accepts only #rrggbb', () => {
    expect(isCursorColor('#ff2d95')).toBe(true);
    expect(isCursorColor('#FF2D95')).toBe(true);
    for (const bad of ['#fff', 'ff2d95', '#gg2d95', '#ff2d95ff', '']) expect(isCursorColor(bad)).toBe(false);
  });
  it('keeps the default first and every preset valid', () => {
    expect(cursorPresets[0].value).toBe('#ff2d95');
    expect(cursorPresets.every((p) => isCursorColor(p.value))).toBe(true);
  });
  it('uses dark eyes on light and mid colours, light eyes on dark ones', () => {
    for (const p of cursorPresets) {
      expect(eyeInk(p.value)).toBe(p.name === 'Graphite' ? '#f5f5f7' : '#15151b');
    }
    expect(eyeInk('#000000')).toBe('#f5f5f7');
    expect(eyeInk('#ffffff')).toBe('#15151b');
  });
});

describe('stepLabel', () => {
  it('gives the desktop tools plain labels', () => {
    expect(stepLabel('mcp__desktop__request_access')).toBe('Ask to use an app');
    expect(stepLabel('mcp__desktop__read_window')).toBe('Read window');
    expect(stepLabel('mcp__desktop__click')).toBe('Click');
    expect(stepLabel('mcp__desktop__vault_fill')).toBe('Sign in with a saved login');
  });
  it('leaves built-in tools as they are and tidies unknown MCP tools', () => {
    expect(stepLabel('Read')).toBe('Read');
    expect(stepLabel('Glob')).toBe('Glob');
    expect(stepLabel('mcp__other__do_thing')).toBe('Do thing');
  });
});

describe('hotkeyWords (a shortcut in the words of this OS)', () => {
  // The settings keep macOS words; a Windows PC has no Cmd or Option key.
  it('says Win and Alt on Windows', () => {
    expect(hotkeyWords('Ctrl+Cmd', 'windows')).toBe('Ctrl+Win');
    expect(hotkeyWords('Right Option', 'windows')).toBe('Right Alt');
    expect(hotkeyWords('Ctrl+Option', 'windows')).toBe('Ctrl+Alt');
    expect(hotkeyWords('Left Cmd+Shift', 'windows')).toBe('Left Win+Shift');
    expect(hotkeyWords('Right Ctrl', 'windows')).toBe('Right Ctrl');
  });
  it('covers every Windows preset, so the box and the list say the same', () => {
    for (const p of hotkeyPresets('windows')) {
      expect(p.label.startsWith(hotkeyWords(p.value, 'windows')), `${p.value} -> ${p.label}`).toBe(true);
    }
  });
  it('leaves the text alone on macOS, and what the user is still typing', () => {
    expect(hotkeyWords('Ctrl+Cmd', 'macos')).toBe('Ctrl+Cmd');
    expect(hotkeyWords('Right Option', 'macos')).toBe('Right Option');
    expect(hotkeyWords('Ctrl+Wi', 'windows')).toBe('Ctrl+Wi');
    expect(hotkeyWords('', 'windows')).toBe('');
  });
});

describe('timeParts (a time for a narrow table column)', () => {
  // The Activity and Vault tables give the time 48 px: "2026-11-16 20:49:58"
  // on one line ran into the Session column. The day goes on its own short line.
  const at = (y: number, mo: number, d: number, h = 9, mi = 5, s = 7) => new Date(y, mo - 1, d, h, mi, s).toISOString();
  const now = new Date(2026, 10, 17, 12, 0, 0);
  it('gives only the time for today', () => {
    expect(timeParts(at(2026, 11, 17), now)).toEqual({ time: '09:05:07', day: '' });
  });
  it('adds a short day for another day this year, and the year for another year', () => {
    expect(timeParts(at(2026, 11, 16, 20, 49, 58), now)).toEqual({ time: '20:49:58', day: '16 Nov' });
    expect(timeParts(at(2026, 1, 3), now)).toEqual({ time: '09:05:07', day: '3 Jan' });
    expect(timeParts(at(2025, 12, 31), now)).toEqual({ time: '09:05:07', day: '31 Dec 2025' });
  });
  it('keeps text it cannot read', () => {
    expect(timeParts('not a time', now)).toEqual({ time: 'not a time', day: '' });
  });
});
