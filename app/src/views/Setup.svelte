<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    errorText,
    events,
    type AppInfo,
    type ClaudeInfo,
    type ModelInfo,
    type Permission,
    type PermissionStatus,
  } from '../lib/api';
  import { formatBytes, percent, permissionText, progressText } from '../lib/format';
  import Connect from './Connect.svelte';

  let { info, onDone }: { info: AppInfo | null; onDone: () => void } = $props();

  let claude = $state<ClaudeInfo | null>(null);
  let checkingClaude = $state(false);
  let perms = $state<PermissionStatus[]>([]);
  let models = $state<ModelInfo[]>([]);
  let licensesOpen = $state(false);
  let downloading = $state(false);
  let progress = $state<Record<string, { text: string; pct: number; error?: string; ok?: boolean }>>({});
  let blockers = $state<string[] | null>(null);
  let finishing = $state(false);
  let message = $state('');

  const inUse = $derived(models.filter((m) => m.in_use));
  const missing = $derived(inUse.filter((m) => !m.present));
  const missingSize = $derived(missing.reduce((n, m) => n + m.size, 0));

  async function checkClaude() {
    checkingClaude = true;
    try {
      claude = await api.claudeStatus();
    } catch (e) {
      message = errorText(e);
    } finally {
      checkingClaude = false;
    }
  }

  async function loadPerms() {
    try {
      perms = await api.permissions();
    } catch (e) {
      message = errorText(e);
    }
  }

  async function loadModels() {
    try {
      models = await api.modelsStatus();
    } catch (e) {
      message = errorText(e);
    }
  }

  async function ask(p: Permission) {
    try {
      await api.permissionRequest(p);
    } catch (e) {
      message = errorText(e);
    }
    loadPerms();
  }

  async function openPane(p: Permission) {
    try {
      await api.permissionOpenSettings(p);
    } catch (e) {
      message = errorText(e);
    }
  }

  // Downloads what is missing; for models already there it re-checks every
  // file's SHA-256 and downloads a damaged file again.
  async function download(list: ModelInfo[]) {
    licensesOpen = false;
    downloading = true;
    progress = {};
    try {
      await api.modelsDownload(list.map((m) => m.id));
    } catch (e) {
      downloading = false;
      message = errorText(e);
    }
  }

  // "huggingface.co/org/repo@rev path" -> "huggingface.co"
  const host = (source: string) => source.split('/')[0];

  async function finish() {
    finishing = true;
    try {
      await api.onboardingFinish();
      const c = await api.setupCheck();
      blockers = c.chat;
      // Chat does not wait for the voice: it opens as soon as the assistant can run.
      if (c.chat.length === 0) onDone();
    } catch (e) {
      message = errorText(e);
    } finally {
      finishing = false;
    }
  }

  onMount(() => {
    checkClaude();
    loadPerms();
    loadModels();
    // Read-only checks; they never show a prompt.
    const timer = setInterval(loadPerms, 2000);
    const unlisten = events.download((e) => {
      if (e.finished) {
        downloading = false;
        loadModels();
        return;
      }
      const cur = progress[e.id] ?? { text: '', pct: 0 };
      if (e.progress) {
        const p = e.progress;
        cur.text = progressText(p);
        if (p.kind === 'downloading') cur.pct = percent(p.model_done, p.model_total);
      }
      if (e.done) {
        if ('Ok' in e.done) {
          cur.ok = true;
          cur.pct = 100;
          cur.text = 'Ready (all files verified)';
          loadModels();
        } else {
          cur.error = e.done.Err;
        }
      }
      progress[e.id] = cur;
    });
    return () => {
      clearInterval(timer);
      unlisten.then((f) => f());
    };
  });
</script>

<section>
  <h2>1. Claude Code</h2>
  <p class="muted">
    Pindot drives your own Claude Code (the <code>claude</code> command) on your own sign-in. It never uses an API
    key.
  </p>
  {#if !claude}
    <p>Checking for <code>claude</code>…</p>
  {:else if !claude.path}
    <div class="card bad">
      <p><strong>Claude Code is not installed.</strong></p>
      {#if claude.problem}
        <p>{claude.problem}</p>
      {:else}
        <p>Install it (see claude.com/claude-code), then press Check again.</p>
      {/if}
    </div>
  {:else if !claude.signed_in}
    <div class="card bad">
      <p><strong>Claude Code is not signed in.</strong></p>
      <p>Open a terminal and run <code>claude login</code>, sign in, then press Check again.</p>
      {#if claude.problem}<p class="muted">Details: {claude.problem}</p>{/if}
      <p class="muted">Found at <code>{claude.path}</code>{claude.version ? ` (${claude.version})` : ''}.</p>
    </div>
  {:else}
    <p class="ok">
      Signed in{claude.method ? ` (${claude.method})` : ''}. <code>{claude.path}</code>{claude.version
        ? `, ${claude.version}`
        : ''}
    </p>
  {/if}
  <button onclick={checkClaude} disabled={checkingClaude}>Check again</button>
</section>

<section>
  <h2>2. Permissions</h2>
  {#if info?.os === 'windows'}
    <p class="muted">
      Windows has nothing to grant per app and never asks. Pindot can hear you when the microphone switches in Windows
      Settings are on (Privacy &amp; security &gt; Microphone). Pindot shows up in the list there only after its first
      recording.
    </p>
  {:else}
    <p class="muted">
      macOS asks once for each. "Ask macOS" shows the system prompt; "Open System Settings" goes to the right list,
      where you switch Pindot on. Some take effect after Pindot restarts.
    </p>
  {/if}
  <table class="perms">
    <tbody>
      {#each perms as p (p.permission)}
        <tr>
          <td>
            <strong>{p.name}</strong>
            <div class="muted small">{p.why}</div>
            {#if p.note}<div class="bad small">{p.note}</div>{/if}
          </td>
          <td class:ok={p.granted} class:bad={!p.granted} class="state">{permissionText(p.state)}</td>
          <td class="actions">
            <!-- Windows has no prompt to show: the switches are in Settings. -->
            {#if !p.granted && info?.os !== 'windows'}
              <button onclick={() => ask(p.permission)}>Ask macOS</button>
            {/if}
            <button onclick={() => openPane(p.permission)}>{info?.os === 'windows' ? 'Open Settings' : 'Open System Settings'}</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</section>

<section>
  <h2>3. Voice models</h2>
  <p class="muted">
    Speech to text and text to speech run on this {info?.os === 'windows' ? 'PC' : 'Mac'}. The models are downloaded once, from the source named under
    each one, and every file is checked against its pinned SHA-256.
  </p>
  <table class="models">
    <tbody>
      {#each inUse as m (m.id)}
        <tr>
          <td>
            <strong>{m.display}</strong>
            <div class="muted small">
              {m.license} · {formatBytes(m.size)} · from {host(m.source)}{m.fallback ? ', with a fallback mirror' : ''}
            </div>
            {#if progress[m.id]}
              <div class="bar"><div style:width="{progress[m.id].pct}%"></div></div>
              <div class="small" class:bad={!!progress[m.id].error}>{progress[m.id].error ?? progress[m.id].text}</div>
            {/if}
          </td>
          <td class="state" class:ok={m.present} class:bad={!m.present}>{m.present ? 'Downloaded' : 'Missing'}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if downloading}
    <button onclick={() => api.modelsCancel()}>Cancel download</button>
    <span class="muted small">A cancelled download resumes where it stopped.</span>
  {:else if missing.length > 0}
    {#if !licensesOpen}
      <button class="primary" onclick={() => (licensesOpen = true)}>Download ({formatBytes(missingSize)})…</button>
    {:else}
      <div class="card">
        <p><strong>Licenses</strong>: please read before downloading.</p>
        {#each missing as m (m.id)}
          <div class="license">
            <p><strong>{m.display}</strong>: {m.license} (<code>{m.license_url}</code>)</p>
            <pre class="license-text">{m.license_text}</pre>
            <p class="small muted">From <code>{m.source}</code>{m.fallback ? `; fallback: ${m.fallback}` : ''}</p>
          </div>
        {/each}
        <button class="primary" onclick={() => download(missing)}>Accept and download {formatBytes(missingSize)}</button>
        <button onclick={() => (licensesOpen = false)}>Not now</button>
      </div>
    {/if}
  {:else if inUse.length > 0}
    <p class="ok">The voice models are downloaded.</p>
    <button onclick={() => download(inUse)}>Check files</button>
    <span class="muted small">Re-checks every file's SHA-256 and downloads a damaged file again.</span>
  {/if}
</section>

<section>
  <h2>4. Use it from your terminal too (optional)</h2>
  <Connect />
</section>

<section>
  <h2>5. Finish</h2>
  <p class="muted">
    After this, hold <strong>{info?.hotkey_label ?? 'the hold-to-talk key'}</strong>, speak, and let go. Press Esc to
    stop at any time.
  </p>
  <button class="primary" onclick={finish} disabled={finishing}>Finish setup</button>
  {#if blockers && blockers.length > 0}
    <div class="card bad">
      <p>Saved. Chat starts once these are sorted:</p>
      <ul>
        {#each blockers as b (b)}<li>{b}</li>{/each}
      </ul>
    </div>
  {/if}
</section>

{#if message}<p class="bad">{message}</p>{/if}
