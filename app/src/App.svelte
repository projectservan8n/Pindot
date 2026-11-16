<script lang="ts">
  import { onMount } from 'svelte';
  import { api, events, type AppInfo, type VoiceDisplay } from './lib/api';
  import { statusOf } from './lib/status';
  import Status from './components/Status.svelte';
  import Rusty from './components/Rusty.svelte';
  import Chat from './views/Chat.svelte';
  import Setup from './views/Setup.svelte';
  import Settings from './views/Settings.svelte';
  import Activity from './views/Activity.svelte';
  import Vault from './views/Vault.svelte';
  import { refreshAssistant } from './lib/assistant.svelte';

  type View = 'chat' | 'activity' | 'vault' | 'settings' | 'setup';
  const nav: { id: View; label: string }[] = [
    { id: 'chat', label: 'Chat' },
    { id: 'vault', label: 'Vault' },
    { id: 'activity', label: 'Activity' },
    { id: 'settings', label: 'Settings' },
  ];
  const titles: Record<View, string> = { chat: 'Chat', vault: 'Vault', activity: 'Activity', settings: 'Settings', setup: 'Setup' };

  function fromHash(): View | null {
    const h = location.hash.replace('#', '');
    return h === 'chat' || h === 'settings' || h === 'activity' || h === 'vault' || h === 'setup' ? h : null;
  }

  // Chat is the home once setup is done; the first run opens Setup.
  let view = $state<View>(fromHash() ?? 'chat');
  let ready = $state(fromHash() !== null);
  let info = $state<AppInfo | null>(null);
  let display = $state<VoiceDisplay | null>(null);
  let notice = $state<string | null>(null);

  function go(v: string) {
    const next: View = v === 'chat' || v === 'settings' || v === 'activity' || v === 'vault' ? v : 'setup';
    view = next;
    ready = true;
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
    refreshAssistant();
    if (!ready) {
      api.settingsGet().then(
        (s) => go(s.settings.onboarding_done ? 'chat' : 'setup'),
        () => go('setup'),
      );
    }
    const unsubs = [
      events.state(refresh),
      events.navigate(go),
      events.voice((v) => {
        display = v.state;
        notice = v.notice;
      }),
    ];
    const onHash = () => go(fromHash() ?? 'chat');
    window.addEventListener('hashchange', onHash);
    return () => {
      window.removeEventListener('hashchange', onHash);
      unsubs.forEach((p) => p.then((f) => f()));
    };
  });

  const status = $derived(statusOf(info, display, notice));
</script>

<a class="skip" href="#main-content" onclick={(e) => { e.preventDefault(); document.getElementById('main-content')?.focus(); }}>Skip to content</a>

<div class="shell">
  <aside class="side">
    <div class="brand">
      <Rusty size={30} />
      <div class="brand-text">
        <strong>Pindot</strong>
        <span>for Claude Code</span>
      </div>
    </div>
    <nav aria-label="Main">
      {#each nav as v (v.id)}
        <button type="button" class="navitem" class:active={view === v.id} aria-current={view === v.id ? 'page' : undefined} onclick={() => go(v.id)}>{v.label}</button>
      {/each}
    </nav>
    <span class="spacer"></span>
    <nav aria-label="Setup">
      <button type="button" class="navitem" class:active={view === 'setup'} aria-current={view === 'setup' ? 'page' : undefined} onclick={() => go('setup')}>
        Setup{#if status.blocked}<span class="badge" aria-label="needs attention">!</span>{/if}
      </button>
    </nav>
    <div class="side-foot">
      {#if info}<span>v{info.version}</span>{/if}
      {#if info?.server_url}<code title={`The address Claude Code connects to: ${info.server_url}`}>{info.server_url.replace(/^https?:\/\//, '')}</code>{/if}
    </div>
  </aside>

  <div class="stage">
    <div class="topbar">
      <h1 class="vtitle">{titles[view]}</h1>
      <span class="spacer"></span>
      <Status {status} onFix={view === 'setup' ? undefined : () => go('setup')} />
    </div>

    {#if info?.test_switches}
      <div class="banner warn" role="alert">Test build: test switches are compiled in. Not for normal use.</div>
    {/if}
    {#if info?.server_error}
      <div class="banner bad" role="alert">The connection Claude Code uses is not running: {info.server_error}. Restart Pindot; if it keeps happening, check that port {info.server_url ?? ''} is free.</div>
    {/if}
    {#if info?.load_note}
      <div class="banner">{info.load_note}</div>
    {/if}

    <main id="main-content" tabindex="-1" class:fill={view === 'chat'}>
      {#if !ready}
        <p class="muted pad">Loading...</p>
      {:else if view === 'chat'}
        <Chat onGo={go} />
      {:else if view === 'setup'}
        <Setup {info} onDone={() => go('chat')} />
      {:else if view === 'settings'}
        <Settings {info} />
      {:else if view === 'vault'}
        <Vault />
      {:else}
        <Activity />
      {/if}
    </main>
  </div>
</div>
