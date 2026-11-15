<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, type AppInfo, type HotkeyCheck, type SettingsView, type TalkStyle } from '../lib/api';
  import { chordFromCodes, formatBytes, hotkeyPresets } from '../lib/format';
  import Connect from './Connect.svelte';

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

  async function preview() {
    if (!view) return;
    previewing = true;
    voiceBad = false;
    try {
      await api.voicePreview(view.settings.voice.tts_voice, view.settings.voice.volume, assistantName);
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

  async function load() {
    try {
      view = await api.settingsGet();
      hotkeyText = view.settings.hotkey;
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
          hotkeyText = chord;
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
        value={presets.some((p) => p.value === hotkeyText) ? hotkeyText : ''}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          if (v) {
            hotkeyText = v;
            runCheck();
          }
        }}
      >
        <option value="">Custom…</option>
        {#each presets as p (p.value)}<option value={p.value}>{p.label}</option>{/each}
      </select>
      <input id="hk" bind:value={hotkeyText} oninput={runCheck} placeholder="e.g. Right Option or Ctrl+Cmd" />
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
    <h2>Voice</h2>
    <div class="row">
      <label for="stt">Speech to text</label>
      <select id="stt" bind:value={view.settings.voice.stt_model}>
        {#each view.stt_models as m (m.id)}
          <option value={m.id}>{m.display} ({m.license}, {formatBytes(m.size)})</option>
        {/each}
      </select>
    </div>
    <div class="row">
      <label for="tts">Text to speech</label>
      <select id="tts" bind:value={view.settings.voice.tts_model}>
        {#each view.tts_models as m (m.id)}
          <option value={m.id}>{m.display} ({m.license}, {formatBytes(m.size)})</option>
        {/each}
      </select>
    </div>
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
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.voice.companion} />
      Keep the AI cursor next to mine while I talk: it shows my voice, then flies off to act and comes back
    </label>
    <label class="row">
      <input type="checkbox" bind:checked={view.settings.voice.captions} />
      Show what {assistantName} says in a bubble next to the cursor
    </label>
    <div class="row">
      <label for="vol">Volume</label>
      <input id="vol" type="range" min="0" max="1" step="0.05" bind:value={view.settings.voice.volume} />
      <span class="small">{Math.round(view.settings.voice.volume * 100)}%</span>
    </div>
  </section>

  <section>
    <h2>Claude</h2>
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
      <p class="small muted">Stored only for you (file mode 600) in <code>{info.data_dir}</code>.</p>
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
    border-radius: 8px;
    background: color-mix(in srgb, var(--warn) 8%, var(--card));
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
