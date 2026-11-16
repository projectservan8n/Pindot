// One status line for every screen: what Rusty is doing right now, or what is
// stopping it. Pure, so it is unit tested (status.test.ts).
import type { AppInfo, VoiceDisplay } from './api';

export type StatusKind =
  | 'ready'
  | 'listening'
  | 'thinking'
  | 'speaking'
  | 'acting'
  | 'starting'
  | 'off'
  | 'blocked';

export interface Status {
  kind: StatusKind;
  /** Short, for the sidebar. */
  label: string;
  /** Longer, for a tooltip and screen readers: what it means and what to do. */
  detail: string;
  /** Needs the user to do something. */
  blocked: boolean;
}

function blocker(note: string | null): { label: string; detail: string } {
  const n = (note ?? '').toLowerCase();
  if (n.includes('model')) return { label: 'Needs a voice model', detail: `A voice model is missing. Open Setup and download it. (${note})` };
  if (n.includes('permission') || n.includes('accessib') || n.includes('microphone') || n.includes('screen recording') || n.includes('input monitoring')) {
    return { label: 'Needs a permission', detail: `A permission is missing. Open Setup to allow it. (${note})` };
  }
  if (n.includes('claude code') || n.includes('claude`') || n.includes('signed in')) {
    return { label: 'Needs Claude Code', detail: `Claude Code is not ready. Open Setup. (${note})` };
  }
  return { label: 'Blocked', detail: note ? `Voice cannot start: ${note}. Open Setup.` : 'Voice cannot start. Open Setup.' };
}

export function statusOf(info: AppInfo | null, display: VoiceDisplay | null, notice: string | null = null): Status {
  const s = (kind: StatusKind, label: string, detail: string, blocked = false): Status => ({ kind, label, detail, blocked });
  if (!info) return s('starting', 'Starting', 'Pindot is starting.');
  if (info.voice === 'failed') {
    const b = blocker(info.voice_note);
    return s('blocked', b.label, b.detail, true);
  }
  if (info.voice === 'off') {
    const note = info.voice_note ?? '';
    if (/turned off/i.test(note) && !/;/.test(note)) return s('off', 'Voice is off', 'Hold-to-talk is turned off in Settings. You can still chat.');
    if (!note) return s('off', 'Voice is off', 'Hold-to-talk is off.');
    const b = blocker(note);
    return s('blocked', b.label, b.detail, true);
  }
  const chatOnly = info.voice === 'chat_only';
  if (info.voice === 'starting') return s('starting', 'Starting', 'Hold-to-talk is starting.');
  switch (display) {
    case 'listening':
      return s('listening', 'Listening', 'Rusty is listening. Let go of the key to send.');
    case 'thinking':
      return s('thinking', 'Thinking', 'Rusty is working on your request.');
    case 'speaking':
      return s('speaking', 'Speaking', 'Rusty is speaking.');
    case 'acting':
      return s('acting', 'Controlling your computer', 'Rusty is using your mouse or keyboard. Press Esc to stop.');
    default:
      if (chatOnly) {
        const why = info.voice_note ? ` Voice is off: ${info.voice_note}.` : ' Voice is off.';
        return s('ready', 'Chat ready', `You can type and dictate in Chat.${why}${notice ? ` ${notice}` : ''}`);
      }
      return s('ready', 'Ready', `Hold ${info.hotkey_label} to talk, or type in Chat.${notice ? ` ${notice}` : ''}`);
  }
}
