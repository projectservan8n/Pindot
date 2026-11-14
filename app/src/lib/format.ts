// Pure helpers for the main window (unit tested in format.test.ts).
import type { ActivityEntry, ConnectStatus, PermissionState, Progress } from './api';

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '?';
  if (n < 1000) return `${n} B`;
  const units = ['kB', 'MB', 'GB'];
  let v = n;
  let i = -1;
  do {
    v /= 1000;
    i++;
  } while (v >= 1000 && i < units.length - 1);
  return `${v.toFixed(1)} ${units[i]}`;
}

export function percent(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((done / total) * 100)));
}

export function permissionText(state: PermissionState): string {
  switch (state) {
    case 'granted':
      return 'Allowed';
    case 'not_needed':
      return 'Not needed on this system';
    case 'not_asked':
      return 'Not asked yet';
    case 'denied':
      return 'Denied';
    case 'restricted':
      return 'Blocked by a profile';
    case 'not_granted':
      return 'Not allowed';
    default:
      return 'Unknown';
  }
}

export function progressText(p: Progress): string {
  switch (p.kind) {
    case 'checking':
      return `Checking ${p.file}`;
    case 'downloading':
      return `Downloading ${p.file}: ${formatBytes(p.model_done)} of ${formatBytes(p.model_total)}`;
    case 'resumed':
      return `Resuming ${p.file} from ${formatBytes(p.from)}`;
    case 'verified':
      return `${p.file} verified`;
    case 'checksum_failed':
      return `${p.file} was damaged (checksum mismatch); downloading it again`;
    case 'retrying':
      return `Retrying ${p.file} (attempt ${p.attempt}): ${p.error}`;
    case 'fallback':
      return `Using the fallback source: ${p.note}`;
  }
}

export function connectText(s: ConnectStatus): string {
  switch (s.state) {
    case 'connected':
      return 'Connected: Claude Code sessions in your terminal have the desktop tools.';
    case 'not_connected':
      return 'Not connected.';
    case 'outdated':
      return `Connected, but out of date (${s.reason}). Press Connect again.`;
    case 'name_taken':
      return `Another MCP server already uses the name "desktop" (${s.detail}). Remove it first.`;
    case 'unknown':
      return `Could not read the Claude Code config: ${s.error}`;
  }
}

/** "2026-11-14T09:12:03.123Z" to local "09:12:03" (plus the date if not today). */
export function shortTime(ts: string, now: Date = new Date()): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const pad = (n: number) => String(n).padStart(2, '0');
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  const sameDay =
    d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
  return sameDay ? time : `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${time}`;
}

/** Does a live activity entry pass the current filters? ('' = all). */
export function passes(e: ActivityEntry, session: string, tool: string): boolean {
  return (!session || e.session === session) && (!tool || e.tool === tool);
}

const MOD_CODES: Record<string, [string, 'Left' | 'Right']> = {
  ControlLeft: ['Ctrl', 'Left'],
  ControlRight: ['Ctrl', 'Right'],
  AltLeft: ['Option', 'Left'],
  AltRight: ['Option', 'Right'],
  ShiftLeft: ['Shift', 'Left'],
  ShiftRight: ['Shift', 'Right'],
  MetaLeft: ['Cmd', 'Left'],
  MetaRight: ['Cmd', 'Right'],
  OSLeft: ['Cmd', 'Left'],
  OSRight: ['Cmd', 'Right'],
};

const ORDER = ['Ctrl', 'Option', 'Shift', 'Cmd'];

/**
 * The chord text for the modifier keys pressed while recording
 * (`KeyboardEvent.code` values). One key keeps its side ("Right Option");
 * two or more become side-less ("Ctrl+Cmd"), which matches either side.
 * Returns null when a non-modifier key was in the set (only modifier-only
 * holds are allowed) or nothing was pressed. Fn never reaches a web page,
 * so it is picked from the list instead.
 */
export function chordFromCodes(codes: Iterable<string>): string | null {
  const keys: [string, 'Left' | 'Right'][] = [];
  for (const c of codes) {
    const k = MOD_CODES[c];
    if (!k) return null;
    if (!keys.some(([n, s]) => n === k[0] && s === k[1])) keys.push(k);
  }
  if (keys.length === 0) return null;
  if (keys.length === 1) return `${keys[0][1]} ${keys[0][0]}`;
  const names = [...new Set(keys.map(([n]) => n))];
  names.sort((a, b) => ORDER.indexOf(a) - ORDER.indexOf(b));
  return names.join('+');
}

/** Ready-made choices for the shortcut list, per OS (saved in macOS words). */
export function hotkeyPresets(os: 'macos' | 'windows'): { value: string; label: string }[] {
  if (os === 'windows') {
    return [
      { value: 'Ctrl+Cmd', label: 'Ctrl+Win (default)' },
      { value: 'Right Option', label: 'Right Alt' },
      { value: 'Right Ctrl', label: 'Right Ctrl' },
      { value: 'Ctrl+Option', label: 'Ctrl+Alt' },
    ];
  }
  return [
    { value: 'Fn', label: 'Fn / Globe (default)' },
    { value: 'Right Option', label: 'Right Option' },
    { value: 'Right Cmd', label: 'Right Cmd' },
    { value: 'Ctrl+Cmd', label: 'Ctrl+Cmd' },
    { value: 'Ctrl+Option', label: 'Ctrl+Option' },
  ];
}
