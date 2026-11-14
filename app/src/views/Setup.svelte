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

  const missing = $derived(models.filter((m) => m.in_use && !m.present));
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

  async function download() {
    licensesOpen = false;
    downloading = true;
    progress = {};
    try {
      await api.modelsDownload(missing.map((m) => m.id));
    } catch (e) {
      downloading = false;
      message = errorText(e);
    }
  }

  async function finish() {
    finishing = true;
    try {
      await api.onboardingFinish();
      blockers = await api.voiceCheck();
      if (blockers.length === 0) onDone();
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
    Deskhand drives your own Claude Code (the <code>claude</code> command) on your own sign-in. It never uses an API
    key.
  </p>
  {#if !claude}
    <p>Checking for <code>claude</code>…</p>
  {:else if !claude.path}
    <div class="card bad">
      <p><strong>Claude Code is not installed.</strong></p>
      <p>Install it (see claude.com/claude-code), then press Check again.</p>
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
    <p class="muted">Windows needs no extra permission except the microphone privacy switch.</p>
  {:else}
    <p class="muted">
      macOS asks once for each. "Ask macOS" shows the system prompt; "Open System Settings" goes to the right list,
      where you switch Deskhand on. Some take effect after Deskhand restarts.
    </p>
  {/if}
  <table class="perms">
    <tbody>
      {#each perms as p (p.permission)}
        <tr>
          <td>
            <strong>{p.name}</strong>
            <div class="muted small">{p.why}</div>
          </td>
          <td class:ok={p.granted} class:bad={!p.granted} class="state">{permissionText(p.state)}</td>
          <td class="actions">
            {#if !p.granted}
              <button onclick={() => ask(p.permission)}>{info?.os === 'windows' ? 'Ask Windows' : 'Ask macOS'}</button>
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
    Speech to text and text to speech run on this Mac. The models are downloaded once from Hugging Face (with a
    fallback mirror) and every file is checked against its SHA-256.
  </p>
  <table class="models">
    <tbody>
      {#each models.filter((m) => m.in_use) as m (m.id)}
        <tr>
          <td>
            <strong>{m.display}</strong>
            <div class="muted small">{m.license} · {formatBytes(m.size)}</div>
            {#if progress[m.id]}
              <div class="bar"><div style={`width:${progress[m.id].pct}%`}></div></div>
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
            <p class="small">{m.license_text}</p>
            <p class="small muted">From <code>{m.source}</code>{m.fallback ? `; fallback: ${m.fallback}` : ''}</p>
          </div>
        {/each}
        <button class="primary" onclick={download}>Accept and download {formatBytes(missingSize)}</button>
        <button onclick={() => (licensesOpen = false)}>Not now</button>
      </div>
    {/if}
  {:else if models.length > 0}
    <p class="ok">Both models are downloaded and verified.</p>
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
      <p>Saved. Hold-to-talk starts once these are sorted:</p>
      <ul>
        {#each blockers as b (b)}<li>{b}</li>{/each}
      </ul>
    </div>
  {/if}
</section>

{#if message}<p class="bad">{message}</p>{/if}
