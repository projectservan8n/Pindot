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
    type DictationEvent,
    type VoiceDisplay,
  } from '../lib/api';
  import { formatBytes, shortTime, stepLabel } from '../lib/format';
  import Markdown from '../components/Markdown.svelte';
  import Thumb from '../components/Thumb.svelte';
  import Rusty from '../components/Rusty.svelte';
  import { assistant, refreshAssistant } from '../lib/assistant.svelte';
  import { insertAtCaret, problemText } from '../lib/dictate';

  let { onGo }: { onGo: (v: string) => void } = $props();

  let view = $state<ChatView>({ session_id: null, items: [], display: 'idle', can_send: false, can_speak: false });
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
  const name = $derived(assistant.name);
  const placeholder = $derived(
    !view.can_send
      ? 'Chat starts once Setup is finished'
      : viewing
        ? 'You are reading an earlier conversation'
        : busy
          ? `${name} is working. You can type ahead, or press Stop.`
          : `Message ${name}. Enter sends, Shift+Enter adds a line.`,
  );

  // Dictation: speech to text in the box. It never sends anything.
  let dictPhase = $state<'idle' | 'listening' | 'transcribing'>('idle');
  let dictId = $state(0);
  let dictLevel = $state(0);
  let dictError = $state('');
  const dictating = $derived(dictPhase !== 'idle');
  const dictLabel = $derived(
    dictPhase === 'listening'
      ? 'Listening. Tap the microphone to finish, or Esc to cancel.'
      : dictPhase === 'transcribing'
        ? 'Writing out what you said...'
        : '',
  );

  async function toggleDictation() {
    dictError = '';
    if (dictPhase === 'listening') {
      try {
        await api.dictationStop(dictId);
      } catch (e) {
        dictError = errorText(e);
      }
      return;
    }
    if (dictPhase === 'transcribing') return;
    try {
      dictId = await api.dictationStart();
      dictPhase = 'listening';
    } catch (e) {
      dictError = errorText(e);
    }
  }

  async function cancelDictation() {
    dictPhase = 'idle';
    dictLevel = 0;
    try {
      await api.dictationCancel();
    } catch (e) {
      dictError = errorText(e);
    }
  }

  async function insertSpoken(text: string) {
    const el = inputEl;
    // The caret and selection are kept by the box even while it is not focused.
    const start = el ? el.selectionStart : draft.length;
    const end = el ? el.selectionEnd : draft.length;
    const r = insertAtCaret(draft, start, end, text);
    draft = r.text;
    await tick();
    if (inputEl) {
      inputEl.focus();
      inputEl.setSelectionRange(r.caret, r.caret);
    }
  }

  function onDictation(e: DictationEvent) {
    if (e.kind === 'level') {
      dictLevel = Math.max(0, Math.min(1, e.level * 4));
      return;
    }
    if (e.kind === 'state') {
      dictId = e.id;
      dictPhase = e.state;
      if (e.state === 'idle') dictLevel = 0;
      if (e.state === 'listening') dictError = '';
    } else if (e.kind === 'text') {
      insertSpoken(e.text);
    } else if (e.kind === 'error') {
      dictError = problemText(e.problem, e.message);
      dictPhase = 'idle';
      dictLevel = 0;
    } else if (e.kind === 'cancelled') {
      dictPhase = 'idle';
      dictLevel = 0;
    }
  }

  function onWindowKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && dictating) {
      e.preventDefault();
      e.stopPropagation();
      cancelDictation();
    }
  }

  // Only while the box (or the mic button beside it) has focus does the
  // hold-to-talk key dictate here instead of sending a spoken turn.
  function composerFocus(on: boolean, e?: FocusEvent) {
    if (!on && e?.relatedTarget instanceof Node && (e.currentTarget as HTMLElement).contains(e.relatedTarget)) return;
    api.dictationFocus(on).catch(() => {});
  }

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
      await api.chatSend(draft.trim(), attached.map((a) => a.path), speak && view.can_speak);
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
    refreshAssistant();
    load();
    api.approvalsPending().then((a) => (approvals = a), () => {});
    const unsubs = [
      events.chat(load),
      events.dictation(onDictation),
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
    return () => {
      unsubs.forEach((p) => p.then((f) => f()));
      // Leaving the screen ends a dictation without inserting anything.
      api.dictationCancel().catch(() => {});
      api.dictationFocus(false).catch(() => {});
    };
  });
</script>

<svelte:window onkeydown={onWindowKey} />

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
        <Rusty size={84} pose={!view.can_send && !viewing ? 'blocked' : 'idle'} />
        <h2>{viewing ? 'Nothing in this conversation' : `Ask ${name} anything`}</h2>
        {#if !view.can_send && !viewing}
          <p>Chat needs Claude Code, installed and signed in. Setup walks you through it.</p>
          <button type="button" class="primary" onclick={() => onGo('setup')}>Open Setup</button>
        {:else if viewing}
          <p>This record has no messages we can show.</p>
        {:else}
          <p>Type below, or tap Dictate and talk. {#if view.can_speak}Hold the key to get a spoken answer.{/if}</p>
          <p class="muted small">Typed messages are answered on screen{#if view.can_speak}, and read aloud only if you switch that on{/if}. Drop files or pictures in to share them.</p>
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
        <!-- Not "You stopped this": Pindot presses Esc itself when Claude
             Code asks for a permission it may not have. -->
        <p class="note" role="status">This request was stopped.</p>
      {/if}
    {/each}

    {#if busy && !viewing}
      <p class="working" role="status"><span class="pulse" aria-hidden="true"></span>{view.display === 'listening' ? 'Listening' : view.display === 'acting' ? 'Using your computer' : 'Thinking'}</p>
    {/if}
  </div>

  {#each approvals as a (a.id)}
    <div class="approval" role="alertdialog" aria-labelledby={`ap-${a.id}`} aria-describedby={`apd-${a.id}`}>
      {#if a.login}
      <strong id={`ap-${a.id}`}>Let {name} sign in with a saved login?</strong>
      <div id={`apd-${a.id}`}>
        <p><span class="appname">{a.login.name}</span> <span class="muted small">{a.login.site}</span></p>
        <p class="small">Username: {a.login.username || '(none)'}. Pindot types {a.login.field === 'code' ? 'the current authenticator code' : `the ${a.login.field}`} itself, only on exactly this website. {name} never sees it.</p>
      </div>
      {:else}
      <strong id={`ap-${a.id}`}>Allow {name} to control {a.apps.length === 1 ? 'this app' : 'these apps'}?</strong>
      <ul id={`apd-${a.id}`}>
        {#each a.apps as app, k (k)}
          <li>
            <span class="appname">{app.name}</span>{#if !app.running}<span class="muted small"> (not open yet)</span>{/if}
            {#if app.warning}<div class="warn-text small">{app.warning}</div>{/if}
          </li>
        {/each}
      </ul>
      {#if a.reason}<p class="reason small"><span class="muted">Its reason, in its own words:</span> <span class="plain">{a.reason}</span></p>{/if}
      {/if}
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
    <div class="compose-box" onfocusin={() => composerFocus(true)} onfocusout={(e) => composerFocus(false, e)} role="group" aria-label="Write a message">
    {#if dictating || dictError}
      <p class="dictation" class:listening={dictPhase === 'listening'} class:err={!!dictError && !dictating} role={dictError && !dictating ? 'alert' : 'status'} style:--level={dictLevel}>
        {#if dictating}<span class="dot" aria-hidden="true"></span>{dictLabel}{:else}{dictError}{/if}
      </p>
    {/if}
    <textarea
      bind:this={inputEl}
      bind:value={draft}
      rows="2"
      {placeholder}
      aria-label={`Message to ${name}`}
      disabled={!canType}
      onkeydown={onKey}
      onpaste={onPaste}
    ></textarea>
    <div class="row tools">
      <button type="button" onclick={chooseFiles} disabled={!canType}>Attach files</button>
      <button
        type="button"
        class="mic"
        class:on={dictPhase === 'listening'}
        aria-pressed={dictPhase === 'listening'}
        aria-label={dictPhase === 'listening' ? 'Stop dictating' : 'Dictate into the message'}
        title={dictPhase === 'listening' ? 'Stop (Esc cancels)' : 'Dictate: speak, and the words are typed here. Nothing is sent.'}
        disabled={!canType || dictPhase === 'transcribing'}
        style:--level={dictLevel}
        onclick={toggleDictation}
      >
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path fill="currentColor" d="M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V22h2v-3.08A7 7 0 0 0 19 12h-2Z"/></svg>
        <span>{dictPhase === 'listening' ? 'Done' : dictPhase === 'transcribing' ? 'Working' : 'Dictate'}</span>
      </button>
      {#if view.can_speak}<label class="check"><input type="checkbox" checked={speak} onchange={(e) => setSpeak(e.currentTarget.checked)} /> Read replies aloud</label>{/if}
      <span class="spacer"></span>
      {#if busy}<button type="button" class="danger" onclick={stop} aria-label="Stop (same as pressing Esc)">Stop</button>{/if}
      <button type="button" class="primary" onclick={send} disabled={!canSend}>Send</button>
    </div>
    </div>
  </div>

  {#if dragging}<div class="dropzone" aria-hidden="true">Drop files to attach them</div>{/if}
</div>
