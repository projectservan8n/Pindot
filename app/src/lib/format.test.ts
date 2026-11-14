import { describe, expect, it } from 'vitest';
import {
  chordFromCodes,
  connectText,
  formatBytes,
  hotkeyPresets,
  passes,
  percent,
  permissionText,
  progressText,
  shortTime,
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
