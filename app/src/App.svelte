<script lang="ts">
  import { onMount } from 'svelte';
  import { api, events, type AppInfo } from './lib/api';
  import Setup from './views/Setup.svelte';
  import Settings from './views/Settings.svelte';
  import Activity from './views/Activity.svelte';

  type View = 'setup' | 'settings' | 'activity';
  const views: { id: View; label: string }[] = [
    { id: 'setup', label: 'Setup' },
    { id: 'settings', label: 'Settings' },
    { id: 'activity', label: 'Activity' },
  ];

  function fromHash(): View {
    const h = location.hash.replace('#', '');
    return h === 'settings' || h === 'activity' ? h : 'setup';
  }

  let view = $state<View>(fromHash());
  let info = $state<AppInfo | null>(null);

  function go(v: string) {
    const next = v === 'settings' || v === 'activity' ? v : 'setup';
    view = next;
    if (location.hash !== `#${next}`) history.replaceState(null, '', `#${next}`);
  }

  async function refresh() {
    try {
      info = await api.appInfo();
    } catch {
      info = null;
    }
  }

  onMount(() => {
    refresh();
    const unsubs = [events.state(refresh), events.navigate(go)];
    const onHash = () => go(fromHash());
    window.addEventListener('hashchange', onHash);
    return () => {
      window.removeEventListener('hashchange', onHash);
      unsubs.forEach((p) => p.then((f) => f()));
    };
  });

  const voiceText = $derived.by(() => {
    if (!info) return '';
    switch (info.voice) {
      case 'running':
        return `Hold ${info.hotkey_label} to talk`;
      case 'starting':
        return 'Hold-to-talk is starting';
      case 'failed':
        return `Hold-to-talk failed: ${info.voice_note ?? ''}`;
      default:
        return `Hold-to-talk is off${info.voice_note ? `: ${info.voice_note}` : ''}`;
    }
  });
</script>

<header>
  <div class="brand">
    <svg class="mark" viewBox="40 40 176 176" width="26" height="26" aria-hidden="true"><path fill="#ff2d95" fill-rule="evenodd" d="M46.12 71.59 A20 20 0 0 1 71.59 46.12 L197.59 90.12 A20 20 0 0 1 200.99 126.32 L153.64 153.64 L126.32 200.99 A20 20 0 0 1 90.12 197.59 Z M97.0 113.0 a8.0 8.0 0 0 1 16 0 v12 a8.0 8.0 0 0 1 -16 0 Z M127.0 113.0 a8.0 8.0 0 0 1 16 0 v12 a8.0 8.0 0 0 1 -16 0 Z"/></svg>
    <div class="brand-text">
      <strong>Pindot</strong>
      <span class="muted">works with Claude Code</span>
    </div>
  </div>
  <nav>
    {#each views as v (v.id)}
      <button class="tab" class:active={view === v.id} onclick={() => go(v.id)}>{v.label}</button>
    {/each}
  </nav>
</header>

{#if info?.test_switches}
  <div class="banner warn">Test build: test switches are compiled in. Not for normal use.</div>
{/if}
{#if info?.server_error}
  <div class="banner bad">The MCP server is not running: {info.server_error}</div>
{/if}
{#if info?.load_note}
  <div class="banner">{info.load_note}</div>
{/if}

<main>
  {#if view === 'setup'}
    <Setup {info} onDone={() => go('settings')} />
  {:else if view === 'settings'}
    <Settings {info} />
  {:else}
    <Activity />
  {/if}
</main>

<footer>
  <span class="dot" class:on={info?.voice === 'running'} class:fail={info?.voice === 'failed'}></span>
  <span>{voiceText}</span>
  <span class="spacer"></span>
  {#if info?.server_url}<code>{info.server_url}</code>{/if}
  {#if info}<span class="muted">v{info.version}</span>{/if}
</footer>
