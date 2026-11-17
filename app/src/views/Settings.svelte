<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, type AppInfo, type HotkeyCheck, type InputDevices, type SettingsView, type TalkStyle } from '../lib/api';
  import {
    ELEVENLABS_STT,
    ELEVENLABS_TTS,
    chordFromCodes,
    cursorPresets,
    eyeInk,
    formatBytes,
    hotkeyPresets,
    hotkeyWords,
    isCursorColor,
    openRouterModel,
    openRouterSetting,
    speechEngine,
  } from '../lib/format';
  import Connect from './Connect.svelte';
  import BrainPicker from '../lib/BrainPicker.svelte';
  import { refreshAssistant } from '../lib/assistant.svelte';

  let { info }: { info: AppInfo | null } = $props();

  let view = $state<SettingsView | null>(null);
  let hotkeyText = $state('');
  let check = $state<HotkeyCheck | null>(null);
  let recording = $state(false);
  let saving = $state(false);
  let notes = $state<string[]>([]);
  let problems = $state<string[]>([]);
  let tokenConfirm = $state(false);
  let tokenNote = $state('');
  let previewing = $state(false);
  let voiceNote = $state('');
  let voiceBad = $state(false);

  const voiceGroups = $derived.by(() => {
    const all = view?.tts_voices ?? [];
    return [
      { label: 'American', voices: all.filter((v) => v.locale === 'en-us') },
      { label: 'British', voices: all.filter((v) => v.locale === 'en-gb') },
      { label: 'Characters', voices: all.filter((v) => v.locale === 'characters') },
    ].filter((g) => g.voices.length > 0);
  });
  const chosenVoice = $derived(
    view?.tts_voices.find((v) => v.name.toLowerCase() === (view?.settings.voice.tts_voice ?? '').toLowerCase()),
  );

  // Picking a voice downloads it at once (256 KB), so Save and Preview
  // never wait on the network.
  async function pickVoice(name: string) {
    if (!view) return;
    view.settings.voice.tts_voice = name || null;
    voiceNote = '';
    voiceBad = false;
    if (!name) return;
    voiceNote = `Downloading ${name}…`;
    try {
      await api.voiceDownload(name);
      voiceNote = `${name} is ready. Preview it, or Save to use it.`;
    } catch (e) {
      voiceNote = errorText(e);
      voiceBad = true;
    }
  }

  // Cloud speech: ElevenLabs and OpenRouter (keys in the system's
  // credential store; the OpenRouter key is the one the brain uses too).
  const sttEngine = $derived(view ? speechEngine(view.settings.voice.stt_model) : 'local');
  const ttsEngine = $derived(view ? speechEngine(view.settings.voice.tts_model) : 'local');
  let keys = $state<{ elevenlabs: boolean; openrouter: boolean }>({ elevenlabs: false, openrouter: false });
  let keyDraft = $state({ elevenlabs: '', openrouter: '' });
  let keyNote = $state('');
  let elVoices = $state<{ id: string; name: string }[] | null>(null);
  let elVoicesNote = $state('');
  let orModels = $state<{ id: string; name: string }[] | null>(null);
  let orNote = $state('');

  async function refreshKeys() {
    keys = await api.speechKeys();
    if (keys.elevenlabs && ttsEngine === 'elevenlabs' && elVoices === null) loadVoices();
  }

  async function saveKey(service: 'elevenlabs' | 'openrouter') {
    keyNote = '';
    try {
      await api.speechKeySet(service, keyDraft[service]);
      keyDraft[service] = '';
      keyNote = 'Key saved in your system keychain.';
      await refreshKeys();
      if (service === 'elevenlabs') loadVoices();
    } catch (e) {
      keyNote = errorText(e);
    }
  }

  async function deleteKey(service: 'elevenlabs' | 'openrouter') {
    keyNote = '';
    try {
      await api.speechKeyDelete(service);
      await refreshKeys();
      if (service === 'elevenlabs') elVoices = null;
    } catch (e) {
      keyNote = errorText(e);
    }
  }

  async function loadVoices() {
    elVoicesNote = '';
    try {
      elVoices = await api.elevenlabsVoices();
      if (view && !view.settings.voice.elevenlabs_voice && elVoices.length) view.settings.voice.elevenlabs_voice = elVoices[0].id;
    } catch (e) {
      elVoicesNote = errorText(e);
    }
  }

  async function loadOrModels() {
    orNote = '';
    try {
      orModels = await api.openrouterSttModels();
    } catch (e) {
      orNote = errorText(e);
    }
  }

  function pickStt(value: string) {
    if (!view) return;
    if (value === 'openrouter') {
      const current = openRouterModel(view.settings.voice.stt_model);
      view.settings.voice.stt_model = openRouterSetting(current || 'openai/whisper-large-v3-turbo');
      if (orModels === null) loadOrModels();
    } else {
      view.settings.voice.stt_model = value;
    }
  }

  function pickTts(value: string) {
    if (!view) return;
    view.settings.voice.tts_model = value;
    if (value === ELEVENLABS_TTS && keys.elevenlabs && elVoices === null) loadVoices();
  }

  async function preview() {
    if (!view) return;
    previewing = true;
    voiceBad = false;
    try {
      const voice = ttsEngine === 'elevenlabs' ? view.settings.voice.elevenlabs_voice : view.settings.voice.tts_voice;
      await api.voicePreview(voice, view.settings.voice.volume, assistantName, view.settings.voice.tts_model);
    } catch (e) {
      voiceNote = errorText(e);
      voiceBad = true;
    } finally {
      previewing = false;
    }
  }

  // The Auto warning is open; Auto is only set from its button.
  let autoConfirm = $state(false);

  // Limits and styles match voice/mod.rs (ASSISTANT_NAME_MAX,
  // TALK_NOTES_MAX, TalkStyle); Save validates them again.
  const NAME_MAX = 24;
  const NOTES_MAX = 300;
  // A half-typed or hand-edited colour never reaches the preview or the picker.
  const cursorColor = $derived(
    view && isCursorColor(view.settings.cursor_color) ? view.settings.cursor_color.toLowerCase() : cursorPresets[0].value,
  );

  const assistantName = $derived(view?.settings.voice.assistant_name.trim() || 'Rusty');
  const talkStyles: { value: TalkStyle; label: string; hint: string }[] = [
    { value: 'friendly', label: 'Friendly', hint: 'warm and relaxed, like a friend' },
    { value: 'brief', label: 'Brief', hint: 'as few words as possible' },
    { value: 'professional', label: 'Professional', hint: 'polite, calm and precise' },
    { value: 'playful', label: 'Playful', hint: 'upbeat, a light joke now and then' },
  ];

  // Values match brain/config.rs (CLAUDE_MODELS, EFFORT_LEVELS); Save
  // validates them again.
  const claudeModels: { value: string | null; label: string }[] = [
    { value: null, label: "Claude Code's default" },
    { value: 'haiku', label: 'Haiku: fastest, uses the least of your plan' },
    { value: 'sonnet', label: 'Sonnet' },
    { value: 'opus', label: 'Opus' },
    { value: 'fable', label: 'Fable' },
  ];
  const effortLevels: { value: string | null; label: string }[] = [
    { value: null, label: "Claude Code's default" },
    { value: 'low', label: 'Low' },
    { value: 'medium', label: 'Medium' },
    { value: 'high', label: 'High' },
    { value: 'xhigh', label: 'Extra high' },
    { value: 'max', label: 'Max' },
  ];

  const presets = $derived(hotkeyPresets(view?.os ?? 'macos'));
  // The shortcut as it is saved (macOS words), whatever words the box shows.
  const savedHotkey = $derived(check?.ok && check.normalized ? check.normalized : hotkeyText);

  // Microphones present now. Listing opens no stream.
  let mics = $state<InputDevices | null>(null);
  let micNote = $state('');
  const savedMic = $derived(view?.settings.voice.input_device ?? '');
  const micMissing = $derived(savedMic !== '' && mics !== null && !mics.devices.includes(savedMic));

  async function refreshMics() {
    micNote = '';
    try {
      mics = await api.listInputDevices();
    } catch (e) {
      micNote = errorText(e);
    }
  }

  async function chooseBrainFolder() {
    if (!view) return;
    try {
      const picked = await api.chooseBrainFolder();
      if (picked) view.settings.voice.brain_folder = picked;
    } catch (e) {
      problems = [errorText(e)];
    }
  }

  async function load() {
    try {
      view = await api.settingsGet();
      refreshMics();
      refreshKeys();
      if (speechEngine(view.settings.voice.stt_model) === 'openrouter') loadOrModels();
      hotkeyText = hotkeyWords(view.settings.hotkey, view.os);
      await runCheck();
    } catch (e) {
      problems = [errorText(e)];
    }
  }

  async function runCheck() {
    try {
      check = await api.hotkeyCheck(hotkeyText);
    } catch (e) {
      check = { ok: false, normalized: null, label: null, caution: null, error: errorText(e) };
    }
  }

  // Record a modifier-only shortcut inside this window: hold the keys, then
  // let go of all of them.
  function record() {
    recording = true;
    const pressed = new Set<string>();
    let seen = new Set<string>();
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      pressed.add(e.code);
      if (pressed.size >= seen.size) seen = new Set(pressed);
    };
    const up = (e: KeyboardEvent) => {
      e.preventDefault();
      pressed.delete(e.code);
      if (pressed.size === 0) {
        stop();
        const chord = chordFromCodes(seen);
        if (chord) {
          hotkeyText = hotkeyWords(chord, view?.os ?? 'macos');
          runCheck();
        } else {
          check = {
            ok: false,
            normalized: null,
            label: null,
            caution: null,
            error: 'Use modifier keys only (Ctrl, Option, Shift, Cmd). For Fn, pick it from the list.',
          };
        }
      }
    };
    const stop = () => {
      recording = false;
      window.removeEventListener('keydown', down, true);
      window.removeEventListener('keyup', up, true);
      window.removeEventListener('blur', stop);
    };
    window.addEventListener('keydown', down, true);
    window.addEventListener('keyup', up, true);
    window.addEventListener('blur', stop);
  }

  async function save() {
    if (!view) return;
    saving = true;
    notes = [];
    problems = [];
    try {
      const s = { ...view.settings, hotkey: check?.ok && check.normalized ? check.normalized : hotkeyText };
      const r = await api.settingsSave(s);
      notes = r.notes.length ? r.notes : ['Saved.'];
      refreshAssistant();
      await load();
    } catch (e) {
      problems = Array.isArray(e) ? (e as string[]) : [errorText(e)];
    } finally {
      saving = false;
    }
  }

  async function regenerate() {
    tokenConfirm = false;
    try {
      tokenNote = await api.tokenRegenerate();
    } catch (e) {
      tokenNote = errorText(e);
    }
  }

  onMount(load);
</script>

{#if view}
  {#if view.overrides.length > 0}
    <div class="banner warn">
      These environment variables override the saved settings right now: {view.overrides.join(', ')}.
    </div>
  {/if}

  <section>
    <h2>Assistant</h2>
    <div class="row">
      <label for="aname">Name</label>
      <input id="aname" bind:value={view.settings.voice.assistant_name} maxlength={NAME_MAX} placeholder="Rusty" />
    </div>
    <fieldset>
      <legend>How {assistantName} talks to you</legend>
      {#each talkStyles as t (t.value)}
        <label class="row">
          <input type="radio" bind:group={view.settings.voice.talk_style} value={t.value} />
          {t.label}<span class="small muted">{t.hint}</span>
        </label>
      {/each}
    </fieldset>
    <label for="anotes" class="notes-label">Anything else about how it talks to you? <span class="small muted">Optional</span></label>
    <textarea
      id="anotes"
      class="notes"
      rows="2"
      maxlength={NOTES_MAX}
      bind:value={view.settings.voice.talk_notes}
      placeholder="e.g. Call me Sam. Tell me what you did, not how."
    ></textarea>
    <p class="small muted">
      {view.settings.voice.talk_notes.length}/{NOTES_MAX} · Used from your next request. These change the tone, not the
      safety rules: {assistantName} still asks before anything risky.
    </p>
  </section>

  <section>
    <h2>Brain folder</h2>
    {#if view.settings.brain.kind === 'loop'}<p class="small warn-text">The brain folder works with Claude Code only for now.</p>{/if}
    <div class="row">
      <label for="brain">Brain folder</label>
      <input id="brain" readonly value={view.settings.voice.brain_folder ?? ''} placeholder="None" />
      <button onclick={chooseBrainFolder}>Choose…</button>
      <button onclick={() => view && (view.settings.voice.brain_folder = null)} disabled={!view.settings.voice.brain_folder}>Clear</button>
    </div>
    <p class="small muted">
      A folder of notes or a Claude Code project. {assistantName} starts there, reads its CLAUDE.md, opens only the notes
      a request needs, and can save things you ask it to remember in a pindot-notes folder inside it. Save to apply: it
      starts a fresh conversation.
    </p>
    {#if view.settings.voice.brain_folder}
      <p class="small warn-text">
        The folder's own Claude settings (hooks, permissions) apply to {assistantName}, and what it reads is sent to
        Anthropic through your Claude Code. Only pick a folder you trust.
      </p>
    {/if}
    {#if view.brain_folder_notice}<p class="small bad">{view.brain_folder_notice}</p>{/if}
  </section>

  <section>
    <h2>On-screen cursor</h2>
    <p class="small muted">The coloured cursor {assistantName} uses on your screen. This colour is for that cursor only.</p>
    <fieldset class="colour">
      <legend>Cursor colour</legend>
      <div class="swatches">
        {#each cursorPresets as p (p.value)}
          <button
            type="button"
            class="swatch"
            class:selected={cursorColor === p.value}
            style:background={p.value}
            aria-label={p.name}
            aria-pressed={cursorColor === p.value}
            title={p.name}
            onclick={() => view && (view.settings.cursor_color = p.value)}
          ></button>
        {/each}
        <label class="custom">
          <input type="color" aria-label="Any other colour" value={cursorColor} oninput={(e) => view && (view.settings.cursor_color = e.currentTarget.value)} />
          <span class="small muted">Any colour</span>
        </label>
        <svg class="preview" viewBox="40 40 176 176" width="44" height="44" role="img" aria-label="Preview of the cursor in this colour">
          <path fill={cursorColor} stroke="#fff" stroke-width="6" stroke-linejoin="round" paint-order="stroke" d="M46.12 71.59 A20 20 0 0 1 71.59 46.12 L197.59 90.12 A20 20 0 0 1 200.99 126.32 L153.64 153.64 L126.32 200.99 A20 20 0 0 1 90.12 197.59 Z" />
          <path fill={eyeInk(cursorColor)} d="M97.0 113.0 a8.0 8.0 0 0 1 16 0 v12 a8.0 8.0 0 0 1 -16 0 Z M127.0 113.0 a8.0 8.0 0 0 1 16 0 v12 a8.0 8.0 0 0 1 -16 0 Z" />
        </svg>
      </div>
    </fieldset>
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.voice.companion} />
      Keep the AI cursor next to mine while I talk: it shows my voice, then flies off to act and comes back
    </label>
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.voice.captions} />
      Show what {assistantName} says in a bubble next to the cursor
    </label>
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.streaming_mode} disabled={!view.streaming_supported} />
      Streaming mode
    </label>
    <p class="small muted">
      {#if view.streaming_supported}
        Show Pindot in OBS and screen shares: {assistantName}'s cursor, this window and approval popups. Your viewers then see the Vault's list too (names, usernames and sites; never a password). {assistantName}'s own screenshots still leave Pindot out. Applies at once.
      {:else}
        Needs macOS 14 or newer.
      {/if}
    </p>
  </section>

  <section>
    <h2>Hold-to-talk</h2>
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.voice_enabled} />
      Hold-to-talk is on
    </label>
    <div class="row">
      <label for="hk">Shortcut</label>
      <select
        id="hk-preset"
        aria-label="Shortcut presets"
        value={presets.some((p) => p.value === savedHotkey) ? savedHotkey : ''}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          if (v) {
            hotkeyText = hotkeyWords(v, view?.os ?? 'macos');
            runCheck();
          }
        }}
      >
        <option value="">Custom…</option>
        {#each presets as p (p.value)}<option value={p.value}>{p.label}</option>{/each}
      </select>
      <input
        id="hk"
        bind:value={hotkeyText}
        oninput={runCheck}
        placeholder={view.os === 'windows' ? 'e.g. Right Alt or Ctrl+Win' : 'e.g. Right Option or Ctrl+Cmd'}
      />
      <button onclick={record} disabled={recording}>{recording ? 'Hold the keys, then let go…' : 'Record'}</button>
    </div>
    {#if check}
      {#if check.ok}
        <p class="small ok">Hold <strong>{check.label}</strong> to talk. Other keys pressed with it cancel the hold.</p>
        <label class="row">
          <input type="checkbox" bind:checked={view.settings.voice.tap_to_talk} />
          Or tap {check.label} to talk hands-free, and tap again to send (stops by itself after a minute)
        </label>
        {#if check.caution}<p class="small warn-text">{check.caution}</p>{/if}
      {:else}
        <p class="small bad">{check.error}</p>
      {/if}
    {/if}
  </section>

  <section>
    <h2>Listening</h2>
    <div class="row">
      <label for="stt">Speech to text</label>
      <select
        id="stt"
        value={sttEngine === 'openrouter' ? 'openrouter' : view.settings.voice.stt_model}
        onchange={(e) => pickStt((e.currentTarget as HTMLSelectElement).value)}
      >
        <optgroup label="On this computer">
          {#each view.stt_models as m (m.id)}
            <option value={m.id}>{m.display} ({m.license}, {formatBytes(m.size)})</option>
          {/each}
        </optgroup>
        <optgroup label="Cloud (needs a key)">
          <option value={ELEVENLABS_STT}>ElevenLabs Scribe v3</option>
          <option value="openrouter">OpenRouter (pick a model)</option>
        </optgroup>
      </select>
    </div>
    {#if sttEngine === 'openrouter'}
      <div class="row">
        <label for="or-model">OpenRouter model</label>
        <select
          id="or-model"
          value={openRouterModel(view.settings.voice.stt_model)}
          onchange={(e) => view && (view.settings.voice.stt_model = openRouterSetting((e.currentTarget as HTMLSelectElement).value))}
        >
          {#if !orModels?.some((m) => m.id === openRouterModel(view?.settings.voice.stt_model ?? ''))}
            <option value={openRouterModel(view.settings.voice.stt_model)}>{openRouterModel(view.settings.voice.stt_model)}</option>
          {/if}
          {#each orModels ?? [] as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
        </select>
        <button onclick={loadOrModels}>Refresh</button>
      </div>
      {#if orNote}<p class="small bad">{orNote}</p>{/if}
    {/if}
    {#if sttEngine !== 'local'}
      {@const svc = sttEngine === 'elevenlabs' ? 'elevenlabs' : 'openrouter'}
      {@const svcName = svc === 'elevenlabs' ? 'ElevenLabs' : 'OpenRouter'}
      <div class="row">
        <label for="stt-key">{svcName} key</label>
        <input
          id="stt-key"
          type="password"
          autocomplete="off"
          bind:value={keyDraft[svc]}
          placeholder={keys[svc] ? 'A key is saved (paste one to replace it)' : `Paste your ${svcName} API key`}
        />
        <button onclick={() => saveKey(svc)} disabled={!keyDraft[svc].trim()}>Save key</button>
        {#if keys[svc]}<button onclick={() => deleteKey(svc)}>Delete key</button>{/if}
      </div>
      {#if keyNote}<p class="small">{keyNote}</p>{/if}
      <p class="small muted">
        What you say is sent to {svcName} to be written down{svc === 'openrouter' ? ' (the same key as an OpenRouter brain)' : ''}.
        The key is kept in your system keychain.
      </p>
    {/if}
    <div class="row">
      <label for="mic">Microphone</label>
      <select id="mic" value={savedMic} onchange={(e) => view && (view.settings.voice.input_device = (e.currentTarget as HTMLSelectElement).value || null)}>
        <option value="">System default{mics?.default ? ` (${mics.default})` : ''}</option>
        {#if micMissing}<option value={savedMic}>{savedMic} (not connected)</option>{/if}
        {#each mics?.devices ?? [] as d (d)}<option value={d}>{d}</option>{/each}
      </select>
      <button onclick={refreshMics}>Refresh</button>
    </div>
    {#if micMissing}
      <p class="small warn-text">"{savedMic}" isn't connected, so the system default is used until it is.</p>
    {/if}
    {#if micNote}<p class="small bad">{micNote}</p>{/if}
    <p class="small muted">Used from your next request after you Save.</p>
  </section>

  <section>
    <h2>Speaking</h2>
    <div class="row">
      <label for="tts">Text to speech</label>
      <select id="tts" value={view.settings.voice.tts_model} onchange={(e) => pickTts((e.currentTarget as HTMLSelectElement).value)}>
        <optgroup label="On this computer">
          {#each view.tts_models as m (m.id)}
            <option value={m.id}>{m.display} ({m.license}, {formatBytes(m.size)})</option>
          {/each}
        </optgroup>
        <optgroup label="Cloud (needs a key)">
          <option value={ELEVENLABS_TTS}>ElevenLabs Flash v2.5</option>
        </optgroup>
      </select>
    </div>
    {#if ttsEngine === 'elevenlabs'}
      {#if sttEngine !== 'elevenlabs'}
        <div class="row">
          <label for="tts-key">ElevenLabs key</label>
          <input
            id="tts-key"
            type="password"
            autocomplete="off"
            bind:value={keyDraft.elevenlabs}
            placeholder={keys.elevenlabs ? 'A key is saved (paste one to replace it)' : 'Paste your ElevenLabs API key'}
          />
          <button onclick={() => saveKey('elevenlabs')} disabled={!keyDraft.elevenlabs.trim()}>Save key</button>
          {#if keys.elevenlabs}<button onclick={() => deleteKey('elevenlabs')}>Delete key</button>{/if}
        </div>
        {#if keyNote}<p class="small">{keyNote}</p>{/if}
      {/if}
      <div class="row">
        <label for="el-voice">Voice</label>
        <select id="el-voice" bind:value={view.settings.voice.elevenlabs_voice} disabled={!elVoices?.length}>
          {#if !elVoices?.length}<option value={view.settings.voice.elevenlabs_voice}>{keys.elevenlabs ? 'Loading your voices…' : 'Add your key first'}</option>{/if}
          {#each elVoices ?? [] as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
        </select>
        <button onclick={loadVoices} disabled={!keys.elevenlabs}>Refresh</button>
        <button onclick={preview} disabled={previewing || !view.settings.voice.elevenlabs_voice}>{previewing ? 'Playing…' : 'Preview'}</button>
      </div>
      {#if elVoicesNote}<p class="small bad">{elVoicesNote}</p>{/if}
      {#if voiceNote}<p class="small {voiceBad ? 'bad' : 'ok'}">{voiceNote}</p>{/if}
      <p class="small muted">What {assistantName} says is sent to ElevenLabs to be spoken. The key is kept in your system keychain.</p>
    {:else}
    <div class="row">
      <label for="ttsv">Voice</label>
      <select
        id="ttsv"
        value={chosenVoice?.name ?? ''}
        onchange={(e) => pickVoice((e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">Default</option>
        {#each voiceGroups as g (g.label)}
          <optgroup label={g.label}>
            {#each g.voices as v (v.name)}
              <option value={v.name}>{v.name}{v.gender ? ` · ${v.gender}, ${v.age.toLowerCase()}` : ''}</option>
            {/each}
          </optgroup>
        {/each}
      </select>
      <button onclick={preview} disabled={previewing}>{previewing ? 'Playing…' : 'Preview'}</button>
    </div>
    {#if chosenVoice}<p class="small muted voice-desc">{chosenVoice.description}</p>{/if}
    {#if voiceNote}<p class="small {voiceBad ? 'bad' : 'ok'}">{voiceNote}</p>{/if}
    <p class="small muted">
      Extra voices are Apache-2.0, from huggingface.co/gradium/phonon-voices: 256 KB each, downloaded when you pick
      one.
    </p>
    {/if}
    <fieldset>
      <legend>Speak</legend>
      <label class="row">
        <input type="radio" bind:group={view.settings.voice.speak_mode} value="each_message" />
        Every message {assistantName} writes (default)
      </label>
      <label class="row">
        <input type="radio" bind:group={view.settings.voice.speak_mode} value="at_stop" />
        Only the final reply
      </label>
    </fieldset>
    <div class="row">
      <label for="vol">Volume</label>
      <input id="vol" type="range" min="0" max="1" step="0.05" bind:value={view.settings.voice.volume} />
      <span class="small">{Math.round(view.settings.voice.volume * 100)}%</span>
    </div>
  </section>

  <section>
    <h2>Brain</h2>
    <p class="small muted">The AI that understands you and uses the desktop tools. Changing it starts a fresh conversation.</p>
    <BrainPicker bind:brain={view.settings.brain} />
  </section>

  <section>
    <h2>Claude</h2>
    {#if view.settings.brain.kind === 'loop'}<p class="small warn-text">These apply to Claude Code only, not to the brain chosen above.</p>{/if}
    <p class="small muted">For what you ask by voice. A smaller model and lower effort reply sooner and use less of your Claude plan.</p>
    <div class="row">
      <label for="cmodel">Model</label>
      <select id="cmodel" bind:value={view.settings.voice.model}>
        {#each claudeModels as m (m.label)}<option value={m.value}>{m.label}</option>{/each}
      </select>
    </div>
    <div class="row">
      <label for="effort">Thinking effort</label>
      <select id="effort" bind:value={view.settings.voice.effort}>
        {#each effortLevels as e (e.label)}<option value={e.value}>{e.label}</option>{/each}
      </select>
    </div>
    <fieldset>
      <legend>Permissions</legend>
      <label class="row">
        <input
          type="radio"
          name="perm"
          checked={view.settings.voice.permission_mode === 'ask' && !autoConfirm}
          onchange={() => {
            if (view) view.settings.voice.permission_mode = 'ask';
            autoConfirm = false;
          }}
        />
        Ask (recommended): Claude uses only Pindot's desktop tools
      </label>
      <label class="row">
        <input
          type="radio"
          name="perm"
          checked={view.settings.voice.permission_mode === 'auto' || autoConfirm}
          onchange={() => (autoConfirm = view?.settings.voice.permission_mode !== 'auto')}
        />
        Auto: Claude can also run commands, change files and browse the web
      </label>
      {#if autoConfirm}
        <div class="disclaimer" role="alertdialog" aria-labelledby="auto-title" aria-describedby="auto-body">
          <strong id="auto-title">Before you turn on Auto</strong>
          <ul id="auto-body">
            <li>
              Claude can run terminal commands, change or delete files and fetch web pages
              <strong>without asking you</strong>. It runs as your user account, so it can reach anything you can.
            </li>
            <li>
              Claude Code checks each action and blocks the ones it judges risky. That check can be wrong, and text on
              a web page or on screen can try to trick Claude.
            </li>
            <li>Claude Code itself recommends auto mode only in isolated environments.</li>
            <li>Pindot still asks you before Claude controls each app, and Esc still stops everything.</li>
            <li>Each step costs about twice the tokens of Ask (about 10k instead of 5k).</li>
          </ul>
          <div class="row">
            <button
              class="danger"
              onclick={() => {
                if (view) view.settings.voice.permission_mode = 'auto';
                autoConfirm = false;
              }}>I understand, turn on Auto</button
            >
            <button onclick={() => (autoConfirm = false)}>Keep Ask</button>
          </div>
        </div>
      {:else if view.settings.voice.permission_mode === 'auto'}
        <p class="small warn-text">
          Auto is on: Claude Code's safety check, not you, approves commands and file changes.
        </p>
      {/if}
    </fieldset>
  </section>

  <section>
    <h2>MCP server</h2>
    <div class="row">
      <label for="port">Port</label>
      <input id="port" type="number" min="1024" max="65535" bind:value={view.settings.port} />
      <span class="small muted">
        Default {view.default_port}. Listens on 127.0.0.1 only{view.running_port
          ? `; running on ${view.running_port} now`
          : ''}.
      </span>
    </div>
    <div class="row">
      <span>Bearer token</span>
      {#if !tokenConfirm}
        <button onclick={() => (tokenConfirm = true)}>Regenerate token…</button>
      {:else}
        <span class="small">The old token stops working at once. Terminal Claude Code must Connect again.</span>
        <button class="danger" onclick={regenerate}>Regenerate</button>
        <button onclick={() => (tokenConfirm = false)}>Cancel</button>
      {/if}
    </div>
    {#if tokenNote}<p class="small">{tokenNote}</p>{/if}
    {#if info?.data_dir}
      <p class="small muted">
        {info.os === 'windows' ? 'Stored in your own user profile,' : 'Stored only for you (file mode 600)'} in
        <code>{info.data_dir}</code>.
      </p>
    {/if}
  </section>

  <div class="savebar">
    <button class="primary" onclick={save} disabled={saving || autoConfirm || (check !== null && !check.ok)}>Save</button>
    {#if autoConfirm}
      <span class="small warn-text">Turn on Auto in the warning above, or press Keep Ask, before saving.</span>
    {/if}
    {#each notes as n (n)}<span class="small ok">{n}</span>{/each}
    {#each problems as p (p)}<span class="small bad">{p}</span>{/each}
  </div>

  <section>
    <h2>Claude Code in your terminal</h2>
    <Connect />
  </section>
{:else if problems.length}
  {#each problems as p (p)}<p class="bad">{p}</p>{/each}
{:else}
  <p>Loading…</p>
{/if}

<style>
  .swatches {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  /* min-height: 0 undoes the app-wide button height, which stretched these
     26 px circles into 34 px tall ovals. */
  .swatch {
    flex: none;
    box-sizing: border-box;
    width: 26px;
    height: 26px;
    min-height: 0;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--line);
    cursor: pointer;
  }
  .swatch.selected {
    outline: 2px solid var(--fg);
    outline-offset: 2px;
  }
  .swatch:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .custom {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 6px;
  }
  .custom input {
    width: 34px;
    height: 28px;
    padding: 0;
  }
  .preview {
    margin-left: 10px;
    background: color-mix(in srgb, var(--fg) 6%, var(--card));
    border-radius: var(--r-sm);
  }
  .notes-label {
    display: block;
    margin: 10px 0 4px;
  }
  .notes {
    display: block;
    box-sizing: border-box;
    width: 100%;
    max-width: 62ch;
    resize: vertical;
  }
  .voice-desc {
    max-width: 62ch;
    margin-top: 0;
  }
  .disclaimer {
    margin: 8px 0 4px;
    padding: 12px 14px;
    border: 1px solid var(--warn);
    border-radius: var(--r-md);
    background: var(--warn-wash);
  }
  .disclaimer strong:first-child {
    color: var(--warn);
  }
  .disclaimer ul {
    margin: 6px 0 10px;
    padding-left: 18px;
  }
  .disclaimer li {
    margin: 3px 0;
  }
</style>
