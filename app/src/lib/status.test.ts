import { describe, expect, it } from 'vitest';
import type { AppInfo } from './api';
import { statusOf } from './status';

const info = (over: Partial<AppInfo> = {}): AppInfo => ({
  version: '1', os: 'macos', data_dir: null, settings_path: null, load_note: '', server_url: null, server_error: null,
  voice: 'running', voice_note: null, hotkey_label: 'Ctrl+Option+Space', capturable: true, test_switches: false, ...over,
});

describe('statusOf', () => {
  it('is starting before the app answers', () => {
    expect(statusOf(null, null).kind).toBe('starting');
  });
  it('follows the voice display when running', () => {
    expect(statusOf(info(), 'idle').kind).toBe('ready');
    expect(statusOf(info(), null).kind).toBe('ready');
    expect(statusOf(info(), 'listening').kind).toBe('listening');
    expect(statusOf(info(), 'thinking').kind).toBe('thinking');
    expect(statusOf(info(), 'speaking').kind).toBe('speaking');
    const a = statusOf(info(), 'acting');
    expect(a.kind).toBe('acting');
    expect(a.label).toContain('Controlling');
  });
  it('names a missing model and says what to do', () => {
    const s = statusOf(info({ voice: 'off', voice_note: 'the speech-to-text model is not downloaded' }), 'idle');
    expect(s.kind).toBe('blocked');
    expect(s.label).toBe('Needs a voice model');
    expect(s.blocked).toBe(true);
    expect(s.detail).toContain('Setup');
  });
  it('names a missing permission', () => {
    const s = statusOf(info({ voice: 'off', voice_note: 'Microphone permission is missing' }), null);
    expect(s.label).toBe('Needs a permission');
  });
  it('names Claude Code problems', () => {
    expect(statusOf(info({ voice: 'off', voice_note: 'Claude Code is not signed in (run `claude login`)' }), null).label).toBe('Needs Claude Code');
  });
  it('a failed start is blocked with the reason', () => {
    const s = statusOf(info({ voice: 'failed', voice_note: 'no audio device' }), null);
    expect(s.kind).toBe('blocked');
    expect(s.detail).toContain('no audio device');
  });
  it('voice turned off on purpose is not an alarm', () => {
    const s = statusOf(info({ voice: 'off', voice_note: 'hold-to-talk is turned off' }), null);
    expect(s.kind).toBe('off');
    expect(s.blocked).toBe(false);
  });
  it('a blocker beats the live display', () => {
    expect(statusOf(info({ voice: 'off', voice_note: 'finish Setup first' }), 'thinking').kind).toBe('blocked');
  });
  it('chat works with voice models missing: not blocked, and it says why voice is off', () => {
    const s = statusOf(info({ voice: 'chat_only', voice_note: 'the speech-to-text model is not downloaded' }), 'idle');
    expect(s.kind).toBe('ready');
    expect(s.label).toBe('Chat ready');
    expect(s.blocked).toBe(false);
    expect(s.detail).toContain('speech-to-text');
    expect(statusOf(info({ voice: 'chat_only' }), 'thinking').kind).toBe('thinking');
  });
});
