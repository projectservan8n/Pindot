// Typed wrappers around the Rust commands in app/src-tauri/src/ui.rs.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SpeakMode = 'each_message' | 'at_stop';

/** 'ask': only Pindot's tools. 'auto': Claude Code's auto mode. */
export type PermissionMode = 'ask' | 'auto';

/** How the assistant talks (one line of its prompt). */
export type TalkStyle = 'friendly' | 'brief' | 'professional' | 'playful';

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
  /** What the assistant is called ("Rusty"). */
  assistant_name: string;
  talk_style: TalkStyle;
  /** The user's own words on how to be talked to. */
  talk_notes: string;
  /** A folder the assistant starts in so it knows you; null = none. */
  brain_folder: string | null;
  /** Name of the microphone; null = the system default. */
  input_device: string | null;
}

export interface Settings {
  version: number;
  hotkey: string;
  port: number;
  voice_enabled: boolean;
  onboarding_done: boolean;
  /** The AI cursor's colour, `#rrggbb`. */
  cursor_color: string;
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
  /** Why the saved brain folder cannot be used now; null = fine or unset. */
  brain_folder_notice: string | null;
}

export interface InputDevices {
  devices: string[];
  /** What the system default is called. */
  default: string | null;
}

export interface AppInfo {
  version: string;
  os: 'macos' | 'windows';
  data_dir: string | null;
  settings_path: string | null;
  load_note: string;
  server_url: string | null;
  server_error: string | null;
  voice: 'running' | 'chat_only' | 'starting' | 'failed' | 'off';
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
  /** What to do, when the state alone does not say it (Windows: which switch is off). */
  note: string | null;
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

export type VoiceDisplay = 'idle' | 'listening' | 'thinking' | 'acting' | 'speaking';

/** The `pindot:voice` event: what the voice loop is doing, and a notice to show. */
export interface VoiceEvent {
  state: VoiceDisplay;
  notice: string | null;
}

export type ChatItem =
  | { kind: 'user'; id: string; text: string; files: string[] }
  | { kind: 'assistant'; id: string; text: string }
  | { kind: 'thinking'; id: string; text: string }
  | { kind: 'step'; id: string; name: string; summary: string; result: string | null; is_error: boolean }
  | { kind: 'interrupted'; id: string };

export interface ChatView {
  session_id: string | null;
  items: ChatItem[];
  display: VoiceDisplay;
  /** False when the assistant is not running (no typed messages then). */
  can_send: boolean;
  /** False when speech is off or its models are missing: replies stay on screen. */
  can_speak: boolean;
}

/** What the dictation worker reports (event `pindot:dictation`). */
export type DictationEvent =
  | { kind: 'state'; id: number; source: 'button' | 'hotkey'; state: 'idle' | 'listening' | 'transcribing' }
  | { kind: 'text'; id: number; source: 'button' | 'hotkey'; text: string }
  | { kind: 'error'; id: number; source: 'button' | 'hotkey'; problem: string; message: string }
  | { kind: 'cancelled'; id: number; source: 'button' | 'hotkey' }
  | { kind: 'level'; level: number };

export interface Attached {
  name: string;
  path: string;
  size: number;
  kind: 'image' | 'pdf' | 'file';
}

export interface Rejected {
  name: string;
  reason: string;
}

export interface Imported {
  attached: Attached[];
  rejected: Rejected[];
}

export interface HistoryEntry {
  session_id: string;
  title: string;
  started: number;
  updated: number;
}

export interface OpenedChat {
  title: string;
  items: ChatItem[];
}

export interface AppRequest {
  name: string;
  bundle_id: string | null;
  running: boolean;
  warning: string | null;
}

export interface ApprovalRequest {
  id: string;
  session: string;
  apps: AppRequest[];
  /** Claude's own words: untrusted, shown as plain text only. */
  reason: string;
  /** A saved login Claude wants to sign in with. Every field comes from the vault. */
  login: LoginRequest | null;
}

export interface LoginRequest {
  name: string;
  site: string;
  username: string;
  /** username | password | code */
  field: string;
}

export interface LoginSummary {
  id: string;
  name: string;
  /** Scheme, host and port only. */
  origin: string;
  username: string;
  has_2fa: boolean;
  created: string;
  updated: string;
  last_used: string | null;
  /** The current key cannot open this login (it can still be deleted). */
  locked: boolean;
}

export interface VaultView {
  logins: LoginSummary[];
  /** Where the key is kept, e.g. "macOS Keychain". */
  place: string;
  problem: string | null;
}

export interface NewLogin {
  name: string;
  site: string;
  username: string;
  password: string;
  authenticator: string | null;
}

export type AuthChange = 'keep' | 'clear' | { set: string };

export interface EditLogin {
  name: string;
  site: string;
  username: string;
  /** Empty or null keeps the saved password. */
  password: string | null;
  authenticator: AuthChange;
}

export interface UseEntry {
  ts: string;
  login_id: string;
  login_name: string;
  site: string;
  /** username | password | code | - (a change to the vault) */
  field: string;
  result: string;
  detail: string;
  session: string;
  ok: boolean;
}

export interface ImportColumn {
  field: string;
  header: string;
}
export interface ImportSkip {
  line: number;
  reason: string;
}
export interface ImportConflict {
  id: string;
  name: string;
  site: string;
  username: string;
}
export interface ImportPreview {
  format: string;
  columns: ImportColumn[];
  total_rows: number;
  new_logins: number;
  unchanged: number;
  conflicts: ImportConflict[];
  gain_key: number;
  skipped: ImportSkip[];
  bad_authenticator: number;
}
export interface ImportResult {
  imported: number;
  replaced: number;
  keys_added: number;
  unchanged: number;
  kept_existing: number;
  skipped: number;
  skipped_reasons: ImportSkip[];
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  settingsGet: () => invoke<SettingsView>('settings_get'),
  chooseBrainFolder: () => invoke<string | null>('choose_brain_folder'),
  listInputDevices: () => invoke<InputDevices>('list_input_devices'),
  settingsSave: (settings: Settings) => invoke<{ notes: string[] }>('settings_save', { settings }),
  hotkeyCheck: (text: string) => invoke<HotkeyCheck>('hotkey_check', { text }),
  claudeStatus: () => invoke<ClaudeInfo>('claude_status'),
  permissions: () => invoke<PermissionStatus[]>('permissions'),
  permissionRequest: (permission: Permission) => invoke<PermissionState>('permission_request', { permission }),
  permissionOpenSettings: (permission: Permission) => invoke<void>('permission_open_settings', { permission }),
  modelsStatus: () => invoke<ModelInfo[]>('models_status'),
  voiceDownload: (name: string) => invoke<void>('voice_download', { name }),
  voicePreview: (name: string | null, volume: number, assistant: string) =>
    invoke<void>('voice_preview', { name, volume, assistant }),
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
  setupCheck: () => invoke<{ chat: string[]; voice: string[] }>('setup_check'),
  voiceCheck: () => invoke<string[]>('voice_check'),
  chatState: () => invoke<ChatView>('chat_state'),
  chatSend: (text: string, files: string[], speak: boolean) => invoke<void>('chat_send', { text, files, speak }),
  chatAttachPaths: (paths: string[]) => invoke<Imported>('chat_attach_paths', { paths }),
  chatAttachBytes: (name: string, data: string) => invoke<Imported>('chat_attach_bytes', { name, data }),
  chatChooseFiles: () => invoke<Imported | null>('chat_choose_files'),
  dictationStart: () => invoke<number>('dictation_start'),
  dictationStop: (id: number) => invoke<void>('dictation_stop', { id }),
  dictationCancel: () => invoke<void>('dictation_cancel'),
  dictationFocus: (focused: boolean) => invoke<void>('dictation_focus', { focused }),
  chatStop: () => invoke<void>('chat_stop'),
  chatImage: (path: string) => invoke<string>('chat_image', { path }),
  chatHistory: () => invoke<HistoryEntry[]>('chat_history'),
  chatOpen: (sessionId: string) => invoke<OpenedChat>('chat_open', { sessionId }),
  vaultState: () => invoke<VaultView>('vault_state'),
  vaultAdd: (login: NewLogin) => invoke<LoginSummary>('vault_add', { login }),
  vaultUpdate: (id: string, login: EditLogin) => invoke<LoginSummary>('vault_update', { id, login }),
  vaultDelete: (id: string) => invoke<void>('vault_delete', { id }),
  vaultUses: (id: string | null) => invoke<UseEntry[]>('vault_uses', { id }),
  vaultImportChoose: () => invoke<ImportPreview | null>('vault_import_choose'),
  vaultImportApply: (replace: string[]) => invoke<ImportResult>('vault_import_apply', { replace }),
  vaultImportCancel: () => invoke<void>('vault_import_cancel'),
  approvalsPending: () => invoke<ApprovalRequest[]>('approvals_pending'),
  approvalAnswerInline: (id: string, allow: boolean) => invoke<void>('approval_answer_inline', { id, allow }),
};

export const events = {
  state: (cb: () => void): Promise<UnlistenFn> => listen('pindot:state', () => cb()),
  navigate: (cb: (view: string) => void): Promise<UnlistenFn> =>
    listen<string>('pindot:navigate', (e) => cb(e.payload)),
  download: (cb: (e: DownloadEvent) => void): Promise<UnlistenFn> =>
    listen<DownloadEvent>('pindot:download', (e) => cb(e.payload)),
  voice: (cb: (e: VoiceEvent) => void): Promise<UnlistenFn> => listen<VoiceEvent>('pindot:voice', (e) => cb(e.payload)),
  dictation: (cb: (e: DictationEvent) => void): Promise<UnlistenFn> =>
    listen<DictationEvent>('pindot:dictation', (e) => cb(e.payload)),
  chat: (cb: () => void): Promise<UnlistenFn> => listen('pindot:chat', () => cb()),
  approval: (cb: (r: ApprovalRequest) => void): Promise<UnlistenFn> =>
    listen<ApprovalRequest>('pindot:approval', (e) => cb(e.payload)),
  approvalDone: (cb: (id: string) => void): Promise<UnlistenFn> =>
    listen<{ id: string }>('pindot:approval-done', (e) => cb(e.payload.id)),
  /** Files dropped on the window (Tauri gives paths; the page never sees the file). */
  dragDrop: (cb: (e: { type: 'enter' | 'over' | 'drop' | 'leave'; paths: string[] }) => void): Promise<UnlistenFn> =>
    listen<{ paths?: string[] }>('tauri://drag-drop', (e) => cb({ type: 'drop', paths: e.payload.paths ?? [] })).then(
      async (a) => {
        const b = await listen('tauri://drag-enter', () => cb({ type: 'enter', paths: [] }));
        const c = await listen('tauri://drag-leave', () => cb({ type: 'leave', paths: [] }));
        return () => {
          a();
          b();
          c();
        };
      },
    ),
  activity: (cb: (e: ActivityEntry) => void): Promise<UnlistenFn> =>
    listen<ActivityEntry>('pindot:activity', (e) => cb(e.payload)),
};

/** A command error as text (Rust returns a string or a list of strings). */
export function errorText(e: unknown): string {
  if (Array.isArray(e)) return e.join('\n');
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
