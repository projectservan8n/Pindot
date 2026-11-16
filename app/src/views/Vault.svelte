<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    errorText,
    type AuthChange,
    type ImportPreview,
    type ImportResult,
    type LoginSummary,
    type UseEntry,
    type VaultView,
  } from '../lib/api';
  import { shortTime } from '../lib/format';

  let view = $state<VaultView | null>(null);
  let message = $state('');
  let selected = $state<string | null>(null);
  let uses = $state<UseEntry[]>([]);

  // The add / edit form. The password is write-only: it is sent to be saved and never read back.
  let form = $state<null | { id: string | null; name: string; site: string; username: string; password: string; auth: string; clearAuth: boolean }>(null);
  let formError = $state('');
  let confirmDelete = $state<string | null>(null);

  // The import dialog.
  let preview = $state<ImportPreview | null>(null);
  let replace = $state<Set<string>>(new Set());
  let result = $state<ImportResult | null>(null);
  let busy = $state(false);

  async function load() {
    try {
      view = await api.vaultState();
    } catch (e) {
      message = errorText(e);
    }
  }

  async function loadUses() {
    if (!selected) {
      uses = [];
      return;
    }
    try {
      uses = await api.vaultUses(selected);
    } catch (e) {
      message = errorText(e);
    }
  }

  async function pick(id: string) {
    selected = selected === id ? null : id;
    confirmDelete = null;
    await loadUses();
  }

  function startAdd() {
    result = null;
    preview = null;
    formError = '';
    form = { id: null, name: '', site: '', username: '', password: '', auth: '', clearAuth: false };
  }

  function startEdit(l: LoginSummary) {
    result = null;
    preview = null;
    formError = '';
    form = { id: l.id, name: l.name, site: l.origin, username: l.username, password: '', auth: '', clearAuth: false };
  }

  async function save() {
    if (!form) return;
    formError = '';
    try {
      if (form.id === null) {
        const l = await api.vaultAdd({
          name: form.name,
          site: form.site,
          username: form.username,
          password: form.password,
          authenticator: form.auth.trim() ? form.auth.trim() : null,
        });
        selected = l.id;
      } else {
        const auth: AuthChange = form.clearAuth ? 'clear' : form.auth.trim() ? { set: form.auth.trim() } : 'keep';
        await api.vaultUpdate(form.id, {
          name: form.name,
          site: form.site,
          username: form.username,
          password: form.password ? form.password : null,
          authenticator: auth,
        });
      }
      form = null;
      message = 'Saved.';
      await load();
      await loadUses();
    } catch (e) {
      formError = errorText(e);
    }
  }

  async function remove(id: string) {
    confirmDelete = null;
    try {
      await api.vaultDelete(id);
      if (selected === id) selected = null;
      message = 'Deleted.';
      await load();
      await loadUses();
    } catch (e) {
      message = errorText(e);
    }
  }

  async function chooseImport() {
    form = null;
    result = null;
    busy = true;
    try {
      const p = await api.vaultImportChoose();
      if (p) {
        preview = p;
        replace = new Set();
      }
    } catch (e) {
      message = errorText(e);
    } finally {
      busy = false;
    }
  }

  function toggle(id: string) {
    const next = new Set(replace);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    replace = next;
  }

  async function applyImport() {
    busy = true;
    try {
      result = await api.vaultImportApply([...replace]);
      preview = null;
      await load();
    } catch (e) {
      message = errorText(e);
    } finally {
      busy = false;
    }
  }

  async function cancelImport() {
    preview = null;
    try {
      await api.vaultImportCancel();
    } catch {
      /* nothing was held */
    }
  }

  onMount(() => {
    load();
  });

  const logins = $derived(view?.logins ?? []);
</script>

{#if view}
  {#if view.problem}
    <div class="banner bad" role="alert">{view.problem}</div>
  {/if}

  <div class="row">
    <p class="muted small grow">
      Saved logins Claude can use. Pindot types them itself; Claude never sees a password. The key is kept in the {view.place}.
    </p>
    <span class="spacer"></span>
    <button onclick={chooseImport} disabled={busy}>Import passwords…</button>
    <button class="primary" onclick={startAdd}>Add login</button>
  </div>

  {#if message}<p class="small" role="status">{message}</p>{/if}

  {#if form}
    <form class="card" onsubmit={(e) => { e.preventDefault(); save(); }}>
      <strong>{form.id === null ? 'Add a login' : 'Edit login'}</strong>
      <div class="grid2">
        <label for="v-name">Name</label>
        <input id="v-name" bind:value={form.name} placeholder="Work reports" autocomplete="off" />
        <label for="v-site">Website</label>
        <input id="v-site" bind:value={form.site} placeholder="https://reports.example.com" autocomplete="off" />
        <label for="v-user">Username</label>
        <input id="v-user" bind:value={form.username} autocomplete="off" />
        <label for="v-pass">Password</label>
        <input
          id="v-pass"
          type="password"
          bind:value={form.password}
          autocomplete="new-password"
          placeholder={form.id === null ? '' : 'Leave blank to keep the saved one'}
        />
        <label for="v-auth">Authenticator key</label>
        <input
          id="v-auth"
          bind:value={form.auth}
          autocomplete="off"
          disabled={form.clearAuth}
          placeholder={form.id === null ? 'Optional: setup key or otpauth:// link' : 'Leave blank to keep the saved one'}
        />
      </div>
      {#if form.id !== null}
        <label class="check"><input type="checkbox" bind:checked={form.clearAuth} /> Remove the saved authenticator key</label>
      {/if}
      <p class="small muted">
        Pindot saves only the scheme, host and port of the website. A password can never be shown again once saved.
      </p>
      {#if formError}<p class="small err" role="alert">{formError}</p>{/if}
      <div class="row">
        <button class="primary" type="submit">Save</button>
        <button type="button" onclick={() => (form = null)}>Cancel</button>
      </div>
    </form>
  {/if}

  {#if preview}
    <div class="card">
      <strong>Import from {preview.format}</strong>
      <p class="small muted">
        {preview.total_rows} rows read. Columns used:
        {#each preview.columns as c, i (c.field)}{i ? ', ' : ' '}<code>{c.header}</code> as {c.field}{/each}.
      </p>
      <ul class="counts">
        <li><strong>{preview.new_logins}</strong> new</li>
        <li><strong>{preview.unchanged}</strong> already saved</li>
        <li><strong>{preview.gain_key}</strong> gain an authenticator key</li>
        <li><strong>{preview.conflicts.length}</strong> have a different password</li>
        <li><strong>{preview.skipped.length}</strong> skipped</li>
      </ul>
      {#if preview.bad_authenticator}
        <p class="small muted">{preview.bad_authenticator} authenticator keys could not be read and were left out.</p>
      {/if}
      {#if preview.conflicts.length}
        <p class="small">These are already saved with a different password. Tick one to replace it; otherwise it is kept as it is.</p>
        {#each preview.conflicts as c (c.id)}
          <label class="check">
            <input type="checkbox" checked={replace.has(c.id)} onchange={() => toggle(c.id)} />
            {c.name} <span class="muted">({c.site}, {c.username})</span>
          </label>
        {/each}
      {/if}
      {#if preview.skipped.length}
        <details>
          <summary class="small">Skipped rows</summary>
          <ul class="small">
            {#each preview.skipped.slice(0, 50) as s (s.line)}<li>Line {s.line}: {s.reason}</li>{/each}
          </ul>
        </details>
      {/if}
      <div class="row">
        <button class="primary" onclick={applyImport} disabled={busy}>Import</button>
        <button onclick={cancelImport}>Cancel</button>
      </div>
      <p class="small muted">Delete the exported file afterwards: it is not encrypted.</p>
    </div>
  {/if}

  {#if result}
    <div class="card" role="status">
      <strong>Import finished</strong>
      <ul class="counts">
        <li><strong>{result.imported}</strong> imported</li>
        <li><strong>{result.replaced}</strong> replaced</li>
        <li><strong>{result.keys_added}</strong> keys added</li>
        <li><strong>{result.kept_existing}</strong> kept as they were</li>
        <li><strong>{result.skipped}</strong> skipped</li>
      </ul>
      <button onclick={() => (result = null)}>Done</button>
    </div>
  {/if}

  {#if logins.length === 0 && !form && !preview}
    <div class="card">
      <strong>No saved logins yet</strong>
      <p class="muted">
        Import a CSV exported from Chrome, 1Password or Bitwarden, or add a login by hand. Once saved, Claude can ask Pindot to
        sign in; you approve each login for the session first.
      </p>
    </div>
  {:else if logins.length > 0}
    <table class="vault">
      <thead>
        <tr><th>Name</th><th>Website</th><th>Username</th><th class="c-2fa">2FA</th><th class="c-act"></th></tr>
      </thead>
      <tbody>
        {#each logins as l (l.id)}
          <tr class:sel={selected === l.id}>
            <td><button class="link" onclick={() => pick(l.id)} aria-expanded={selected === l.id}>{l.name}</button></td>
            <td><code>{l.origin}</code></td>
            <td>{#if l.locked}<span class="muted">Can't be opened</span>{:else}{l.username}{/if}</td>
            <td>{#if l.has_2fa}<span class="pill">2FA</span>{/if}</td>
            <td class="nowrap">
              {#if confirmDelete === l.id}
                <button class="danger small" onclick={() => remove(l.id)}>Delete {l.name}</button>
                <button class="small" onclick={() => (confirmDelete = null)}>Keep</button>
              {:else}
                <button class="small" onclick={() => startEdit(l)} disabled={l.locked}>Edit</button>
                <button class="small" onclick={() => (confirmDelete = l.id)}>Delete</button>
              {/if}
            </td>
          </tr>
          {#if selected === l.id}
            <tr class="hist">
              <td colspan="5">
                <strong class="small">Use history</strong>
                {#if uses.length === 0}
                  <p class="small muted">Nothing yet. Each time Claude asks to sign in with this login, it is listed here.</p>
                {:else}
                  <table class="actlog inner">
                    <thead><tr><th class="c-time">Time</th><th>Field</th><th>Result</th><th class="c-session">Session</th></tr></thead>
                    <tbody>
                      {#each uses as u, i (i)}
                        <tr class:fail={!u.ok}>
                          <td class="nowrap">{shortTime(u.ts)}</td>
                          <td>{u.field}</td>
                          <td>{u.result}{u.detail ? `: ${u.detail}` : ''}</td>
                          <td><code>{u.session}</code></td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {/if}
{:else}
  <p>{message || 'Loading…'}</p>
{/if}

<style>
  .grid2 {
    display: grid;
    grid-template-columns: 9rem 1fr;
    gap: var(--s2) var(--s3);
    align-items: center;
    margin: var(--s3) 0;
  }
  .grow {
    flex: 1 1 24rem;
    margin: 0;
  }
  .check {
    display: flex;
    gap: var(--s2);
    align-items: center;
    margin: var(--s1) 0;
  }
  .counts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s2) var(--s4);
    list-style: none;
    padding: 0;
    margin: var(--s2) 0;
  }
  .pill {
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 var(--s2);
    font-size: var(--fs-sm, 0.8em);
    background: var(--card);
  }
  table.vault {
    width: 100%;
    border-collapse: collapse;
  }
  table.vault tr.sel > td {
    background: var(--card);
  }
  table.vault tr.hist > td {
    padding: var(--s2) var(--s3) var(--s3);
  }
  table.inner {
    width: 100%;
    margin-top: var(--s2);
  }
  .err {
    color: var(--bad);
  }
  .c-2fa {
    width: 4rem;
  }
</style>
