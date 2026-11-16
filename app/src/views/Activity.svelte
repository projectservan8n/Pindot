<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, events, type ActivityEntry, type ActivityView } from '../lib/api';
  import { passes, shortTime } from '../lib/format';

  let view = $state<ActivityView | null>(null);
  let session = $state('');
  let tool = $state('');
  let confirming = $state(false);
  let message = $state('');

  async function load() {
    try {
      view = await api.activityAll(session || null, tool || null);
    } catch (e) {
      message = errorText(e);
    }
  }

  async function clear() {
    confirming = false;
    try {
      await api.activityClear();
      session = '';
      tool = '';
      message = 'Activity log cleared.';
      await load();
    } catch (e) {
      message = errorText(e);
    }
  }

  function live(e: ActivityEntry) {
    if (!view) return;
    view.total += 1;
    if (!view.sessions.includes(e.session)) view.sessions = [...view.sessions, e.session].sort();
    if (!view.tools.includes(e.tool)) view.tools = [...view.tools, e.tool].sort();
    if (passes(e, session, tool)) view.entries = [...view.entries, e];
  }

  onMount(() => {
    load();
    const un = events.activity(live);
    return () => un.then((f) => f());
  });

  const rows = $derived(view ? [...view.entries].reverse() : []);
</script>

{#if view}
  <div class="row filters">
    <label for="f-session">Session</label>
    <select id="f-session" bind:value={session} onchange={load}>
      <option value="">All</option>
      {#each view.sessions as s (s)}<option value={s}>{s}</option>{/each}
    </select>
    <label for="f-tool">Tool</label>
    <select id="f-tool" bind:value={tool} onchange={load}>
      <option value="">All</option>
      {#each view.tools as t (t)}<option value={t}>{t}</option>{/each}
    </select>
    <span class="small muted">{view.entries.length} of {view.total}</span>
    <span class="spacer"></span>
    <button onclick={load}>Refresh</button>
    {#if !confirming}
      <button class="danger" onclick={() => (confirming = true)} disabled={view.total === 0}>Clear…</button>
    {/if}
  </div>
  {#if confirming}
    <div class="card bad">
      <p>Delete all {view.total} entries? This empties the activity log file and cannot be undone.</p>
      <button class="danger" onclick={clear}>Clear the log</button>
      <button onclick={() => (confirming = false)}>Cancel</button>
    </div>
  {/if}
  {#if message}<p class="small">{message}</p>{/if}
  {#if rows.length === 0}
    <div class="card">
      <strong>Nothing here yet{session || tool ? ' for this filter' : ''}</strong>
      <p class="muted">
        {session || tool ? 'Clear the filters to see everything.' : 'Each time Rusty clicks, types or looks at your screen, it is listed here so you can see what happened. Try asking it to do something in Chat.'}
      </p>
    </div>
  {:else}
    <table class="log">
      <thead>
        <tr><th>Time</th><th>Session</th><th>Tool</th><th>Details</th><th>Result</th></tr>
      </thead>
      <tbody>
        {#each rows as e, i (i)}
          <tr class:fail={!e.ok}>
            <td class="nowrap">{shortTime(e.ts)}</td>
            <td><code>{e.session}</code></td>
            <td>{e.tool}</td>
            <td class="args">{e.args}</td>
            <td>{e.outcome}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if view.path}<p class="small muted">Log file: <code>{view.path}</code></p>{/if}
{:else}
  <p>{message || 'Loading…'}</p>
{/if}
