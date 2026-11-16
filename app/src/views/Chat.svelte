<script lang="ts">
  import { onMount, tick } from 'svelte';
  import {
    api,
    events,
    errorText,
    type ApprovalRequest,
    type Attached,
    type ChatItem,
    type ChatView,
    type HistoryEntry,
    type Imported,
    type VoiceDisplay,
  } from '../lib/api';
  import { formatBytes, shortTime, stepLabel } from '../lib/format';
  import Markdown from '../components/Markdown.svelte';
  import Thumb from '../components/Thumb.svelte';

  let { onGo }: { onGo: (v: string) => void } = $props();

  let view = $state<ChatView>({ session_id: null, items: [], display: 'idle', can_send: false });
  let loaded = $state(false);
  let draft = $state('');
  let attached = $state<Attached[]>([]);
  let problems = $state<string[]>([]);
  let sendError = $state('');
  let notice = $state('');
  let speak = $state(localStorage.getItem('pindot.chat.speak') === '1');
  let approvals = $state<ApprovalRequest[]>([]);
  let dragging = $state(false);
  let sending = $state(false);

  let historyOpen = $state(false);
  let history = $state<HistoryEntry[]>([]);
  let historyError = $state('');
  let viewing = $state<{ title: string; items: ChatItem[] } | null>(null);

  let logEl: HTMLDivElement | undefined = $state();
  let inputEl: HTMLTextAreaElement | undefined = $state();
  let stick = true;

  const items = $derived(viewing ? viewing.items : view.items);
  const busy = $derived(view.display === 'listening' || view.display === 'thinking' || view.display === 'acting');
  const canType = $derived(view.can_send && !viewing);
  const canSend = $derived(canType && !busy && !sending && (draft.trim().length > 0 || attached.length > 0));
  const placeholder = $derived(
    !view.can_send
      ? 'Chat starts once Setup is finished'
      : viewing
        ? 'You are reading an earlier conversation'
        : busy
          ? 'Rusty is working. You can type ahead, or press Stop.'
          : 'Message Rusty. Enter sends, Shift+Enter adds a line.',
  );

  type Row =
    | { kind: 'item'; item: ChatItem }
    | { kind: 'steps'; id: string; steps: Extract<ChatItem, { kind: 'step' }>[] };

  /** Consecutive tool calls fold into one collapsed group. */
  const rows = $derived.by(() => {
    const out: Row[] = [];
    for (const it of items) {
      if (it.kind === 'step') {
        const last = out.at(-1);
        if (last && last.kind === 'steps') last.steps.push(it);
        else out.push({ kind: 'steps', id: it.id, steps: [it] });
      } else out.push({ kind: 'item', item: it });
    }
    return out;
  });

  async function load() {
    try {
      view = await api.chatState();
    } catch (e) {
      notice = errorText(e);
    }
    loaded = true;
  }

  function nearBottom() {
    return !logEl || logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 80;
  }
  async function toBottom(force = false) {
    if (!force && !stick) return;
    await tick();
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  }
  $effect(() => {
    void items.length;
    void approvals.length;
    toBottom();
  });

  function note(r: Imported) {
    attached = [...attached, ...r.attached];
    problems = r.rejected.map((x) => `${x.name}: ${x.reason}`);
  }

  async function attachPaths(paths: string[]) {
    if (!canType || paths.length === 0) return;
    try {
      note(await api.chatAttachPaths(paths));
    } catch (e) {
      problems = [errorText(e)];
    }
  }

  async function chooseFiles() {
    try {
      const r = await api.chatChooseFiles();
      if (r) note(r);
    } catch (e) {
      problems = [errorText(e)];
    }
  }

  function toBase64(buf: ArrayBuffer): string {
    let s = '';
    const b = new Uint8Array(buf);
    for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
    return btoa(s);
  }

  async function onPaste(e: ClipboardEvent) {
    const files = [...(e.clipboardData?.files ?? [])];
    if (files.length === 0) return;
    e.preventDefault();
    if (!canType) return;
    for (const f of files) {
      if (f.size > 30 * 1024 * 1024) {
        problems = [`${f.name || 'Pasted file'}: too big to attach (the limit is 10 MB, 5 MB for pictures).`];
        continue;
      }
      try {
        const ext = f.type.startsWith('image/') ? f.type.slice(6).replace('jpeg', 'jpg') : 'bin';
        note(await api.chatAttachBytes(f.name || `pasted-image.${ext}`, toBase64(await f.arrayBuffer())));
      } catch (err) {
        problems = [errorText(err)];
      }
    }
  }

  async function send() {
    if (!canSend) return;
    sending = true;
    sendError = '';
    try {
      await api.chatSend(draft.trim(), attached.map((a) => a.path), speak);
      draft = '';
      attached = [];
      problems = [];
      notice = '';
      stick = true;
      await load();
      toBottom(true);
    } catch (e) {
      sendError = errorText(e);
    } finally {
      sending = false;
      inputEl?.focus();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  async function stop() {
    try {
      await api.chatStop();
    } catch (e) {
      sendError = errorText(e);
    }
  }

  function setSpeak(on: boolean) {
    speak = on;
    localStorage.setItem('pindot.chat.speak', on ? '1' : '0');
  }

  async function decide(r: ApprovalRequest, allow: boolean) {
    try {
      await api.approvalAnswerInline(r.id, allow);
    } catch (e) {
      sendError = errorText(e);
    }
    approvals = approvals.filter((a) => a.id !== r.id);
  }

  async function openHistory() {
    historyOpen = !historyOpen;
    if (!historyOpen) return;
    historyError = '';
    try {
      history = await api.chatHistory();
    } catch (e) {
      historyError = errorText(e);
    }
  }

  async function openPast(h: HistoryEntry) {
    historyError = '';
    try {
      viewing = await api.chatOpen(h.session_id);
      historyOpen = false;
      stick = false;
      await tick();
      if (logEl) logEl.scrollTop = 0;
    } catch (e) {
      historyError = errorText(e);
    }
  }

  function backToCurrent() {
    viewing = null;
    stick = true;
    toBottom(true);
    inputEl?.focus();
  }

  function fileName(p: string) {
    return p.split(/[\\/]/).pop() ?? p;
  }
  function isImage(p: string) {
    return /\.(png|jpe?g|gif|webp)$/i.test(p);
  }
  const stepCount = (n: number) => (n === 1 ? '1 step' : `${n} steps`);
  function groupLabel(steps: { name: string }[]) {
    const names = [...new Set(steps.map((s) => stepLabel(s.name)))];
    return `${stepCount(steps.length)}: ${names.slice(0, 4).join(', ')}${names.length > 4 ? ', ...' : ''}`;
  }
  const stamp = (secs: number) => shortTime(new Date(secs * 1000).toISOString());

  onMount(() => {
    load();
    api.approvalsPending().then((a) => (approvals = a), () => {});
    const unsubs = [
      events.chat(load),
      events.voice((v: { state: VoiceDisplay; notice: string | null }) => {
        view.display = v.state;
        if (v.notice) notice = v.notice;
      }),
      events.state(load),
      events.approval((r) => {
        if (!approvals.some((a) => a.id === r.id)) approvals = [...approvals, r];
      }),
      events.approvalDone((id) => (approvals = approvals.filter((a) => a.id !== id))),
      events.dragDrop((e) => {
        if (e.type === 'enter') dragging = canType;
        else if (e.type === 'leave') dragging = false;
        else {
          dragging = false;
          attachPaths(e.paths);
        }
      }),
    ];
    inputEl?.focus();
    return () => unsubs.forEach((p) => p.then((f) => f()));
  });
</script>

<div class="chat" class:dragging>
  <div class="chat-bar">
    <span class="spacer"></span>
    <button type="button" class="ghost" onclick={openHistory} aria-expanded={historyOpen} aria-controls="history-panel">History</button>
  </div>

  {#if historyOpen}
    <div id="history-panel" class="history" role="region" aria-label="Past conversations">
      {#if historyError}<p class="bad">{historyError}</p>{/if}
      {#if history.length === 0 && !historyError}
        <p class="muted">No earlier conversations yet. They appear here after your first message.</p>
      {:else}
        <ul>
          {#each history as h (h.session_id)}
            <li><button type="button" onclick={() => openPast(h)}><span class="h-title">{h.title || 'Untitled'}</span><span class="muted small">{stamp(h.updated)}</span></button></li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  {#if viewing}
    <div class="viewing" role="status">
      <span>Reading an earlier conversation{viewing.title ? `: ${viewing.title}` : ''}. It is read only.</span>
      <button type="button" onclick={backToCurrent}>Back to the current chat</button>
    </div>
  {/if}

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="log"  
    role="log"
    aria-label="Conversation"
    aria-live="polite"
    tabindex="0"
    bind:this={logEl}
    onscroll={() => (stick = nearBottom())}
  >
    {#if loaded && rows.length === 0}
      <div class="empty">
        <h2>{viewing ? 'Nothing in this conversation' : 'Ask Rusty anything'}</h2>
        {#if !view.can_send && !viewing}
          <p>Chat needs the assistant running, which needs Setup to be finished: the voice models, Claude Code and the permissions.</p>
          <button type="button" class="primary" onclick={() => onGo('setup')}>Open Setup</button>
        {:else if viewing}
          <p>This record has no messages we can show.</p>
        {:else}
          <p>Type below, or hold the key and talk. Both go to the same conversation, so you can do either and carry on.</p>
          <p class="muted small">Typed messages are answered on screen and not read aloud, unless you switch that on under the box. Drop files or pictures here to share them.</p>
        {/if}
      </div>
    {/if}

    {#each rows as r (r.kind === 'steps' ? `s-${r.id}` : r.item.id)}
      {#if r.kind === 'steps'}
        <details class="steps">
          <summary>
            <span class="chev" aria-hidden="true"></span>
            <span>{groupLabel(r.steps)}</span>
            {#if r.steps.some((s) => s.result === null) && busy && !viewing}<span class="running" role="status">working</span>{/if}
            {#if r.steps.some((s) => s.is_error)}<span class="bad small">a step failed</span>{/if}
          </summary>
          <ul>
            {#each r.steps as s (s.id)}
              <li>
                <details class="step" class:err={s.is_error}>
                  <summary>
                    <span class="sname">{stepLabel(s.name)}</span>
                    <span class="ssum">{s.summary}</span>
                    {#if s.result === null && busy && !viewing}<span class="running">running</span>{/if}
                  </summary>
                  {#if s.name !== stepLabel(s.name)}<p class="muted small rawname">Tool: <code>{s.name}</code></p>{/if}
                  {#if s.result !== null}<pre>{s.result || '(no output)'}</pre>{:else}<p class="muted small">No result yet.</p>{/if}
                </details>
              </li>
            {/each}
          </ul>
        </details>
      {:else if r.item.kind === 'user'}
        <div class="msg user">
          <div class="bubble">
            {#if r.item.text}<p class="plain">{r.item.text}</p>{/if}
            {#if r.item.files.length}
              <div class="files">
                {#each r.item.files as f (f)}
                  {#if isImage(f)}<Thumb path={f} name={fileName(f)} big />{:else}<span class="chip">{fileName(f)}</span>{/if}
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if r.item.kind === 'assistant'}
        <div class="msg assistant"><div class="bubble"><Markdown text={r.item.text} /></div></div>
      {:else if r.item.kind === 'thinking'}
        {#if r.item.text}
          <details class="think"><summary>Thinking</summary><p class="plain">{r.item.text}</p></details>
        {/if}
      {:else if r.item.kind === 'interrupted'}
        <p class="note" role="status">You stopped this.</p>
      {/if}
    {/each}

    {#if busy && !viewing}
      <p class="working" role="status"><span class="pulse" aria-hidden="true"></span>{view.display === 'listening' ? 'Listening' : view.display === 'acting' ? 'Using your computer' : 'Thinking'}</p>
    {/if}
  </div>

  {#each approvals as a (a.id)}
    <div class="approval" role="alertdialog" aria-labelledby={`ap-${a.id}`} aria-describedby={`apd-${a.id}`}>
      <strong id={`ap-${a.id}`}>Allow Rusty to control {a.apps.length === 1 ? 'this app' : 'these apps'}?</strong>
      <ul id={`apd-${a.id}`}>
        {#each a.apps as app, k (k)}
          <li>
            <span class="appname">{app.name}</span>{#if !app.running}<span class="muted small"> (not open yet)</span>{/if}
            {#if app.warning}<div class="warn-text small">{app.warning}</div>{/if}
          </li>
        {/each}
      </ul>
      {#if a.reason}<p class="reason small"><span class="muted">Its reason, in its own words:</span> <span class="plain">{a.reason}</span></p>{/if}
      <div class="row">
        <button type="button" class="primary" onclick={() => decide(a, true)}>Allow for this session</button>
        <button type="button" class="danger" onclick={() => decide(a, false)}>Deny</button>
      </div>
    </div>
  {/each}

  {#if notice}
    <p class="inline-error" role="alert">{notice} <button type="button" class="ghost small" onclick={() => (notice = '')}>Dismiss</button></p>
  {/if}

  <div class="composer">
    {#if attached.length}
      <ul class="chips" aria-label="Attached files">
        {#each attached as a (a.path)}
          <li class="chip">
            {#if a.kind === 'image'}<Thumb path={a.path} name={a.name} />{/if}
            <span>{a.name}</span><span class="muted small">{formatBytes(a.size)}</span>
            <button type="button" class="ghost small" aria-label={`Remove ${a.name}`} onclick={() => (attached = attached.filter((x) => x.path !== a.path))}>Remove</button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if problems.length}
      <ul class="problems" role="alert">{#each problems as p, k (k)}<li>{p}</li>{/each}</ul>
    {/if}
    {#if sendError}<p class="inline-error" role="alert">{sendError}</p>{/if}
    <textarea
      bind:this={inputEl}
      bind:value={draft}
      rows="2"
      {placeholder}
      aria-label="Message to Rusty"
      disabled={!canType}
      onkeydown={onKey}
      onpaste={onPaste}
    ></textarea>
    <div class="row tools">
      <button type="button" onclick={chooseFiles} disabled={!canType}>Attach files</button>
      <label class="check"><input type="checkbox" checked={speak} onchange={(e) => setSpeak(e.currentTarget.checked)} /> Read replies aloud</label>
      <span class="spacer"></span>
      {#if busy}<button type="button" class="danger" onclick={stop} aria-label="Stop (same as pressing Esc)">Stop</button>{/if}
      <button type="button" class="primary" onclick={send} disabled={!canSend}>Send</button>
    </div>
  </div>

  {#if dragging}<div class="dropzone" aria-hidden="true">Drop files to attach them</div>{/if}
</div>
