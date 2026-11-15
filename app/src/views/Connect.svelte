<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, events, type ConnectView } from '../lib/api';
  import { connectText } from '../lib/format';

  let view = $state<ConnectView | null>(null);
  let busy = $state(false);
  let output = $state('');
  let copied = $state('');

  async function load() {
    try {
      view = await api.connectStatus();
    } catch (e) {
      output = errorText(e);
    }
  }

  async function connect() {
    busy = true;
    output = '';
    try {
      output = (await api.connectClaude()) || 'Done.';
    } catch (e) {
      output = errorText(e);
    } finally {
      busy = false;
      load();
    }
  }

  async function disconnect() {
    busy = true;
    output = '';
    try {
      output = (await api.disconnectClaude()) || 'Done.';
    } catch (e) {
      output = errorText(e);
    } finally {
      busy = false;
      load();
    }
  }

  async function copy() {
    try {
      const cmd = await api.connectCommand();
      await navigator.clipboard.writeText(cmd);
      copied = 'Copied (it contains your token: keep it private).';
    } catch (e) {
      copied = `Could not copy: ${errorText(e)}`;
    }
  }

  onMount(() => {
    load();
    const un = events.state(load);
    return () => un.then((f) => f());
  });
</script>

<p class="muted">
  "Connect" adds Pindot's tools to Claude Code in any terminal, by running this command for you (it changes your
  Claude Code user settings):
</p>
{#if view}
  {#if view.command}<pre class="cmd">{view.command}</pre>{/if}
  <p class:ok={view.status.state === 'connected'}>{connectText(view.status)}</p>
  {#if !view.claude_found}
    <p class="bad">Claude Code (<code>claude</code>) was not found, so the buttons cannot run it.</p>
  {/if}
  <button class="primary" onclick={connect} disabled={busy || !view.claude_found || !view.command}>
    Connect to Claude Code
  </button>
  <button onclick={disconnect} disabled={busy || !view.claude_found}>Disconnect</button>
  <button onclick={copy} disabled={!view.command}>Copy command</button>
  <p class="muted small">Disconnect runs <code>{view.remove_command}</code>.</p>
  {#if copied}<p class="small">{copied}</p>{/if}
  {#if output}<pre class="out">{output}</pre>{/if}
{/if}
