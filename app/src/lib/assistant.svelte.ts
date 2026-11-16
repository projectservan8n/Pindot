// The assistant's name ("Rusty" unless the person changed it in Settings),
// shared by every screen. Claude Code is the CLI and keeps its own name.
import { api } from './api';

export const DEFAULT_NAME = 'Rusty';

export const assistant = $state({ name: DEFAULT_NAME });

export function nameOf(raw: string | null | undefined): string {
  return raw?.trim() || DEFAULT_NAME;
}

/** Read the saved name. Called at start and after Settings are saved. */
export async function refreshAssistant(): Promise<void> {
  try {
    assistant.name = nameOf((await api.settingsGet()).settings.voice.assistant_name);
  } catch {
    // Keep what we have: the default is fine.
  }
}
