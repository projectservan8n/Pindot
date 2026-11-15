// Typed wrappers around the Rust commands in app/src-tauri/src/ui.rs.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SpeakMode = 'each_message' | 'at_stop';

/** 'ask': only Deskhand's tools. 'auto': Claude Code's auto mode. */
export type PermissionMode = 'ask' | 'auto';

export interface VoiceSettings {
  stt_model: string;
  tts_model: string;
  tts_voice: string | null;
  idle_reset_secs: number;
  speak_mode: SpeakMode;
  volume: number;
  /** `claude --model`; null = Claude Code's default. */
  model: string | null;
  /** `claude --effort`; null = Claude Code's default. */
  effort: string | null;
  permission_mode: PermissionMode;
  /** A quick tap listens hands-free until the next tap. */
  tap_to_talk: boolean;
  /** The AI cursor sits by the user's pointer during a voice request. */
  companion: boolean;
  /** What Claude says appears beside the cursor. */
  captions: boolean;
}

export interface Settings {
  version: number;
  hotkey: string;
  port: number;
  voice_enabled: boolean;
  onboarding_done: boolean;
  voice: VoiceSettings;
  [extra: string]: unknown;
}

export interface PhononVoice {
  name: string;
  /** 'en-us', 'en-gb' or 'characters'. */
  locale: string;
  gender: string;
  age: string;
  description: string;
}

export interface ModelChoice {
  id: string;
  display: string;
  license: string;
  size: number;
}

export interface SettingsView {
  settings: Settings;
  overrides: string[];
  stt_models: ModelChoice[];
  tts_models: ModelChoice[];
  tts_voices: PhononVoice[];
  default_port: number;
  running_port: number | null;
  os: 'macos' | 'windows';
}

export interface AppInfo {
  version: string;
  os: 'macos' | 'windows';
  data_dir: string | null;
  settings_path: string | null;
  load_note: string;
  server_url: string | null;
  server_error: string | null;
  voice: 'running' | 'starting' | 'failed' | 'off';
  voice_note: string | null;
  hotkey_label: string;
  capturable: boolean;
  test_switches: boolean;
}

export interface HotkeyCheck {
  ok: boolean;
  normalized: string | null;
  label: string | null;
  caution: string | null;
  error: string | null;
}

export interface ClaudeInfo {
  path: string | null;
  version: string | null;
  signed_in: boolean;
  method: string | null;
  problem: string | null;
  hint: string;
}

export type Permission = 'Accessibility' | 'InputMonitoring' | 'ScreenRecording' | 'Microphone';
export type PermissionState =
  | 'granted'
  | 'not_granted'
  | 'not_asked'
  | 'denied'
  | 'restricted'
  | 'not_needed'
  | 'unknown';

export interface PermissionStatus {
  permission: Permission;
  granted: boolean;
  state: PermissionState;
  name: string;
  why: string;
  settings_url: string;
}

export type ModelKind = 'SpeechToText' | 'TextToSpeech';

export interface ModelInfo {
  id: string;
  display: string;
  kind: ModelKind;
  license: string;
  license_url: string;
  license_text: string;
  source: string;
  size: number;
  present: boolean;
  in_use: boolean;
  fallback: string | null;
  dir: string | null;
}

export type Progress =
  | { kind: 'checking'; file: string }
  | { kind: 'downloading'; file: string; done: number; total: number; model_done: number; model_total: number }
  | { kind: 'resumed'; file: string; from: number }
  | { kind: 'verified'; file: string }
  | { kind: 'checksum_failed'; file: string }
  | { kind: 'retrying'; file: string; attempt: number; error: string }
  | { kind: 'fallback'; note: string };

export interface DownloadEvent {
  id: string;
  progress: Progress | null;
  done: { Ok: string } | { Err: string } | null;
  finished: boolean;
}

export type ConnectStatus =
  | { state: 'not_connected' }
  | { state: 'connected' }
  | { state: 'outdated'; reason: string }
  | { state: 'name_taken'; detail: string }
  | { state: 'unknown'; error: string };

export interface ConnectView {
  status: ConnectStatus;
  command: string | null;
  remove_command: string;
  claude_found: boolean;
  port: number | null;
}

export interface ActivityEntry {
  ts: string;
  session: string;
  tool: string;
  args: string;
  outcome: string;
  ok: boolean;
}

export interface ActivityView {
  path: string | null;
  entries: ActivityEntry[];
  sessions: string[];
  tools: string[];
  total: number;
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  settingsGet: () => invoke<SettingsView>('settings_get'),
  settingsSave: (settings: Settings) => invoke<{ notes: string[] }>('settings_save', { settings }),
  hotkeyCheck: (text: string) => invoke<HotkeyCheck>('hotkey_check', { text }),
  claudeStatus: () => invoke<ClaudeInfo>('claude_status'),
  permissions: () => invoke<PermissionStatus[]>('permissions'),
  permissionRequest: (permission: Permission) => invoke<PermissionState>('permission_request', { permission }),
  permissionOpenSettings: (permission: Permission) => invoke<void>('permission_open_settings', { permission }),
  modelsStatus: () => invoke<ModelInfo[]>('models_status'),
  voiceDownload: (name: string) => invoke<void>('voice_download', { name }),
  voicePreview: (name: string | null, volume: number) => invoke<void>('voice_preview', { name, volume }),
  modelsDownload: (ids: string[]) => invoke<void>('models_download', { ids }),
  modelsCancel: () => invoke<void>('models_cancel'),
  connectStatus: () => invoke<ConnectView>('connect_status'),
  connectCommand: () => invoke<string>('connect_command'),
  connectClaude: () => invoke<string>('connect_claude'),
  disconnectClaude: () => invoke<string>('disconnect_claude'),
  tokenRegenerate: () => invoke<string>('token_regenerate'),
  activityAll: (session: string | null, tool: string | null) =>
    invoke<ActivityView>('activity_all', { session, tool }),
  activityClear: () => invoke<void>('activity_clear'),
  onboardingFinish: () => invoke<void>('onboarding_finish'),
  voiceSetEnabled: (on: boolean) => invoke<void>('voice_set_enabled', { on }),
  voiceCheck: () => invoke<string[]>('voice_check'),
};

export const events = {
  state: (cb: () => void): Promise<UnlistenFn> => listen('deskhand:state', () => cb()),
  navigate: (cb: (view: string) => void): Promise<UnlistenFn> =>
    listen<string>('deskhand:navigate', (e) => cb(e.payload)),
  download: (cb: (e: DownloadEvent) => void): Promise<UnlistenFn> =>
    listen<DownloadEvent>('deskhand:download', (e) => cb(e.payload)),
  activity: (cb: (e: ActivityEntry) => void): Promise<UnlistenFn> =>
    listen<ActivityEntry>('deskhand:activity', (e) => cb(e.payload)),
};

/** A command error as text (Rust returns a string or a list of strings). */
export function errorText(e: unknown): string {
  if (Array.isArray(e)) return e.join('\n');
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
