<script lang="ts">
  // Choosing the brain: Claude Code, a model on this computer, or an API key.
  // In Setup (`own`) it saves itself with "Use this brain"; in Settings the
  // page's Save saves `brain`. Keys and the model test act at once.
  import { onMount } from 'svelte';
  import { api, errorText, type BrainModel, type BrainSettings, type FoundServer, type ProviderView } from './api';

  let {
    brain = $bindable(),
    claudeReady = false,
    own = false,
  }: { brain: BrainSettings; claudeReady?: boolean; own?: boolean } = $props();

  let providers = $state<ProviderView[]>([]);
  let local = $state<FoundServer[] | null>(null);
  let models = $state<BrainModel[]>([]);
  let modelsError = $state('');
  let key = $state('');
  let keyNote = $state('');
  let testing = $state(false);
  let saved = $state('');
  let problems = $state<string[]>([]);

  const provider = $derived(providers.find((p) => p.id === brain.provider));
  const tested = $derived(brain.tested.find((t) => t.base_url === brain.base_url && t.model === brain.model));
  const choice = $derived(brain.kind === 'claude_code' ? 'claude' : provider?.local ? 'local' : 'api');

  onMount(async () => {
    providers = await api.brainProviders();
    if (brain.kind === 'loop') {
      if (provider?.local) local = await api.brainFindLocal();
      await loadModels();
    }
  });

  async function pick(c: 'claude' | 'local' | 'api') {
    saved = '';
    if (c === 'claude') {
      brain.kind = 'claude_code';
      return;
    }
    brain.kind = 'loop';
    if (c === 'local') {
      await findLocal();
    } else if (!provider || provider.local) {
      await setProvider('openai');
    }
  }

  async function findLocal() {
    local = null;
    local = await api.brainFindLocal();
    const here = local.find((s) => s.provider === brain.provider) ?? local[0];
    if (here) {
      brain.provider = here.provider;
      brain.base_url = here.base_url;
      models = here.models;
      if (!models.some((m) => m.id === brain.model)) brain.model = models[0]?.id ?? '';
    } else if (!provider?.local) {
      brain.provider = 'ollama';
      brain.base_url = providers.find((p) => p.id === 'ollama')?.base_url ?? brain.base_url;
      models = [];
    }
  }

  async function setProvider(id: string) {
    const p = providers.find((x) => x.id === id);
    if (!p) return;
    brain.provider = id;
    if (p.base_url) brain.base_url = p.base_url;
    brain.model = '';
    keyNote = '';
    await loadModels();
  }

  async function loadModels() {
    modelsError = '';
    models = [];
    if (!brain.base_url) return;
    try {
      models = await api.brainModels(brain.provider, brain.base_url);
      if (!models.some((m) => m.id === brain.model)) brain.model = models[0]?.id ?? brain.model;
    } catch (e) {
      modelsError = errorText(e);
    }
  }

  async function saveKey() {
    try {
      const warning = await api.brainKeySet(brain.provider, key);
      key = '';
      keyNote = warning ?? 'Key saved in your system keychain.';
      providers = await api.brainProviders();
      await loadModels();
    } catch (e) {
      keyNote = errorText(e);
    }
  }

  async function deleteKey() {
    try {
      await api.brainKeyDelete(brain.provider);
      keyNote = 'Key deleted.';
      providers = await api.brainProviders();
    } catch (e) {
      keyNote = errorText(e);
    }
  }

  async function test() {
    testing = true;
    try {
      const r = await api.brainTest($state.snapshot(brain));
      brain.tested = [r, ...brain.tested.filter((t) => !(t.base_url === r.base_url && t.model === r.model))];
    } catch (e) {
      const r = { base_url: brain.base_url, model: brain.model, ok: false, note: errorText(e), at: 0 };
      brain.tested = [r, ...brain.tested.filter((t) => !(t.base_url === r.base_url && t.model === r.model))];
    } finally {
      testing = false;
    }
  }

  async function use() {
    problems = [];
    saved = '';
    try {
      await api.brainChoose($state.snapshot(brain));
      saved = 'Saved. The assistant restarts with this brain.';
      if (brain.kind === 'loop' && !tested) await test();
    } catch (e) {
      problems = Array.isArray(e) ? e.map(String) : [errorText(e)];
    }
  }

  const modelLabel = (m: BrainModel) =>
    [m.id, m.vision ? 'sees pictures' : null, m.tools === false ? 'no tools' : null].filter(Boolean).join(' · ');
</script>

<div class="picker">
  <div class="choices" role="radiogroup" aria-label="Brain">
    <label class="choice">
      <input type="radio" name="brain-kind" checked={choice === 'claude'} onchange={() => pick('claude')} />
      <span>
        <strong>Claude Code</strong>
        <span class="small muted">Your own Claude Code and Claude plan.{claudeReady ? ' Ready.' : ''}</span>
      </span>
    </label>
    <label class="choice">
      <input type="radio" name="brain-kind" checked={choice === 'local'} onchange={() => pick('local')} />
      <span>
        <strong>A model on this computer</strong>
        <span class="small muted">Ollama, LM Studio or llama.cpp. Nothing leaves your computer.</span>
      </span>
    </label>
    <label class="choice">
      <input type="radio" name="brain-kind" checked={choice === 'api'} onchange={() => pick('api')} />
      <span>
        <strong>An API key</strong>
        <span class="small muted">Anthropic, OpenAI, OpenRouter, DeepSeek, Groq, or another OpenAI-compatible service.</span>
      </span>
    </label>
  </div>

  {#if choice === 'local'}
    {#if local === null}
      <p class="small muted">Looking for Ollama, LM Studio and llama.cpp…</p>
    {:else if local.length === 0}
      <p class="small">
        No model server is running. Start Ollama (<code>ollama serve</code>) or LM Studio's server, then
        <button class="small" onclick={findLocal}>Look again</button>
      </p>
    {:else}
      <div class="row">
        <label for="brain-server">Server</label>
        <select id="brain-server" value={brain.provider} onchange={(e) => setProvider(e.currentTarget.value)}>
          {#each local as s (s.provider)}<option value={s.provider}>{s.name}</option>{/each}
        </select>
      </div>
    {/if}
  {:else if choice === 'api'}
    <div class="row">
      <label for="brain-provider">Provider</label>
      <select id="brain-provider" value={brain.provider} onchange={(e) => setProvider(e.currentTarget.value)}>
        {#each providers.filter((p) => !p.local) as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
      </select>
    </div>
    {#if brain.provider === 'custom'}
      <div class="row">
        <label for="brain-url">Address</label>
        <input id="brain-url" type="url" bind:value={brain.base_url} placeholder="https://example.com/v1" onchange={loadModels} />
      </div>
    {/if}
    <div class="row">
      <label for="brain-key">API key</label>
      <input
        id="brain-key"
        type="password"
        bind:value={key}
        placeholder={provider?.has_key ? 'A key is saved (paste one to replace it)' : 'Paste your API key'}
        autocomplete="off"
      />
      <button onclick={saveKey} disabled={!key.trim()}>Save key</button>
      {#if provider?.has_key}<button onclick={deleteKey}>Delete key</button>{/if}
    </div>
    {#if keyNote}<p class="small">{keyNote}</p>{/if}
    <p class="small muted">The key is kept in your system keychain. What you ask, and what Pindot reads on screen for it, goes to this provider.</p>
  {/if}

  {#if brain.kind === 'loop' && (choice === 'api' || (local && local.length))}
    {#if modelsError}<p class="inline-error">{modelsError}</p>{/if}
    <div class="row">
      <label for="brain-model">Model</label>
      {#if models.length}
        <select id="brain-model" bind:value={brain.model}>
          {#each models as m (m.id)}<option value={m.id}>{modelLabel(m)}</option>{/each}
        </select>
      {:else}
        <input id="brain-model" bind:value={brain.model} placeholder="Model name" />
      {/if}
      <button class="small" onclick={loadModels}>Refresh</button>
    </div>
    <div class="row">
      <label for="brain-vision">Pictures</label>
      <select id="brain-vision" bind:value={brain.vision}>
        <option value="auto">Automatic</option>
        <option value="on">This model can see pictures</option>
        <option value="off">Text only</option>
      </select>
    </div>
    <div class="row">
      <button onclick={test} disabled={testing || !brain.model}>{testing ? 'Testing…' : 'Test this model'}</button>
      {#if tested}
        {#if tested.ok}
          <span class="small ok">Passed the test.</span>
        {:else}
          <span class="small warn-text">Didn't pass: {tested.note} You can keep it anyway.</span>
        {/if}
      {/if}
    </div>
    {#if brain.provider === 'ollama'}
      <p class="small muted">
        Ollama gives a model a short memory by default. For longer requests, start it with
        <code>OLLAMA_CONTEXT_LENGTH=32768</code>.
      </p>
    {/if}
    <p class="small muted">Small models often can't use the desktop tools well. The test tells you.</p>
  {/if}

  {#if own}
    <div class="row">
      <button class="primary" onclick={use}>Use this brain</button>
      {#if saved}<span class="small ok">{saved}</span>{/if}
    </div>
    {#each problems as p (p)}<p class="inline-error">{p}</p>{/each}
  {/if}
</div>

<style>
  .picker {
    display: grid;
    gap: 10px;
  }
  .choices {
    display: grid;
    gap: 10px;
  }
  .choice {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .choice > span {
    display: grid;
    gap: 2px;
  }
</style>
