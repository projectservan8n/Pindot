// Chat screens for the screenshot tool: ?c=empty|nosession|convo|steps|approval|attach|error|history
//   dictation: dict-listening|dict-transcribing|dict-error|dict-done ; chatonly (voice off, Chat works)
(function () {
  const c = new URLSearchParams(location.search).get('c') || 'convo';
  const D = '/Users/sam/Library/Application Support/Pindot/attachments/2f1c9a52-aaaa-4bbb-8ccc-0123456789ab/';
  const svg = (a, b) => 'data:image/svg+xml;base64,' + btoa(`<svg xmlns="http://www.w3.org/2000/svg" width="320" height="240"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="320" height="240" fill="url(#g)"/><circle cx="230" cy="70" r="34" fill="#fff" opacity=".8"/><path d="M0 240 L110 110 L190 190 L240 140 L320 240Z" fill="#15151B" opacity=".55"/></svg>`);
  const u = (id, text, files = []) => ({ kind: 'user', id, text, files });
  const a = (id, text) => ({ kind: 'assistant', id, text });
  const st = (id, name, summary, result, is_error = false) => ({ kind: 'step', id, name, summary, result, is_error });
  const convo = [
    u('u1', 'Can you give me a quick script to rename all .jpeg files to .jpg, and a checklist for tidying my Downloads?'),
    a('a1', "Sure. Here is a small shell script:\n\n```bash\nfor f in *.jpeg; do\n  mv \"$f\" \"${f%.jpeg}.jpg\"\ndone\n```\n\nAnd a short checklist for **Downloads**:\n\n- Sort by *date added* and delete installers\n- Move receipts to `Documents/Receipts`\n- Empty the Bin when done\n\nSay the word and I can run it for you."),
  ];
  const steps = [
    u('u1', 'What is in my notes about the Lisbon trip?'),
    st('s0', 'mcp__desktop__request_access', 'Notes', 'The user approved for this session: Notes.'),
    st('s1', 'Glob', 'Lisbon*.md', '/Users/sam/Notes/Trips/Lisbon plan.md\n/Users/sam/Notes/Trips/Lisbon food.md'),
    st('s2', 'Read', '/Users/sam/Notes/Trips/Lisbon plan.md', '# Lisbon plan\n\nFlights: TP 1355, 12 March, 07:40\nHotel: Alfama, 4 nights\nTo book: Sintra day trip'),
    st('s3', 'Read', '/Users/sam/Notes/Trips/Lisbon food.md', null),
    a('a1', 'You fly on **12 March at 07:40** (TP 1355) and stay four nights in Alfama. The Sintra day trip is still to book.'),
  ];
  const approval = [
    u('u1', 'Open Notes and add a reminder to call the dentist.'),
    st('s1', 'mcp__desktop__request_access', 'Notes', null),
  ];
  const attach = [
    u('u1', 'What is wrong with this chart?', [D + 'chart.png']),
    a('a1', 'The y-axis starts at 40 rather than 0, which makes the change between the two bars look much bigger than it is.'),
  ];
  const sets = { empty: [], nosession: [], convo, steps, approval, attach, error: convo.slice(0, 1), history: convo, chatonly: [], 'dict-listening': convo.slice(0, 1), 'dict-transcribing': convo.slice(0, 1), 'dict-error': convo.slice(0, 1), 'dict-done': convo.slice(0, 1) };
  let can = c !== 'nosession';
  const req = { id: 'r1', session: 'f73c7cb0', apps: [{ name: 'Notes', bundle_id: 'com.apple.Notes', running: true, warning: null }], reason: 'To add a reminder about calling the dentist.' };
  window.__MOCK_ROUTES = {
    chat_state: () => ({ session_id: can ? '2f1c9a52-aaaa-4bbb-8ccc-0123456789ab' : null, items: sets[c] || convo, display: c === 'approval' || c === 'steps' ? 'acting' : 'idle', can_send: can, can_speak: c !== 'chatonly' }),
    dictation_start: () => 7, dictation_stop: () => null, dictation_cancel: () => null, dictation_focus: () => null,
    chat_image: () => svg('#ff9ac8', '#6b9cff'),
    approvals_pending: () => (c === 'approval' ? [req] : []),
    chat_history: () => [
      { session_id: 'a'.repeat(8) + '-aaaa-4bbb-8ccc-0123456789ab', title: 'What is in my notes about the Lisbon trip?', started: 1794800000, updated: 1794800900 },
      { session_id: 'b'.repeat(8) + '-aaaa-4bbb-8ccc-0123456789ab', title: 'Rename all .jpeg files to .jpg', started: 1794700000, updated: 1794700500 },
      { session_id: 'c'.repeat(8) + '-aaaa-4bbb-8ccc-0123456789ab', title: 'Open Notes and add a reminder', started: 1794600000, updated: 1794600100 },
    ],
    chat_choose_files: () => c === 'error'
      ? { attached: [], rejected: [{ name: 'holiday-video.mov', reason: 'That file is 84 MB and the limit is 10 MB. Trim it or send a smaller copy.' }] }
      : { attached: [{ name: 'chart.png', path: D + 'chart.png', size: 184320, kind: 'image' }, { name: 'budget.pdf', path: D + 'budget.pdf', size: 912000, kind: 'pdf' }], rejected: [] },
  };
  const dict = (kind, extra) => window.__MOCK_EMIT('pindot:dictation', Object.assign({ kind, id: 7, source: 'button' }, extra));
  if (c === 'info-blocked') { /* handled by the base mock's s=setup */ }
  addEventListener('load', () => {
    const click = (sel, t) => setTimeout(() => document.querySelector(sel)?.click(), t);
    if (c === 'attach' || c === 'error') {
      click('.tools button:not(.primary)', 500);
      setTimeout(() => {
        const ta = document.querySelector('textarea');
        if (ta && c === 'attach') { ta.value = 'And the other file too, please'; ta.dispatchEvent(new Event('input', { bubbles: true })); }
      }, 700);
    }
    if (c === 'error') setTimeout(() => window.__MOCK_EMIT('pindot:voice', { state: 'idle', notice: "I couldn't reach Claude Code. Open Setup and check that you are signed in, then try again." }), 600);
    if (c === 'steps' || c === 'approval') setTimeout(() => window.__MOCK_EMIT('pindot:voice', { state: 'acting', notice: null }), 300);
    if (c === 'steps') setTimeout(() => document.querySelectorAll('details.steps, details.step').forEach((d) => (d.open = true)), 600);
    if (c.startsWith('dict-')) {
      click('.mic', 400);
      if (c === 'dict-listening') setTimeout(() => { dict('state', { state: 'listening' }); window.__MOCK_EMIT('pindot:dictation', { kind: 'level', level: 0.18 }); }, 700);
      if (c === 'dict-transcribing') setTimeout(() => dict('state', { state: 'transcribing' }), 700);
      if (c === 'dict-error') setTimeout(() => dict('error', { problem: 'no_mic', message: 'The microphone would not start (no input device). Check the microphone in Settings.' }), 700);
      if (c === 'dict-done') setTimeout(() => {
        const ta = document.querySelector('textarea');
        ta.value = 'Remind me to call the dentist '; ta.dispatchEvent(new Event('input', { bubbles: true }));
        dict('state', { state: 'idle' }); dict('text', { text: 'on Thursday morning' });
      }, 700);
    }
    if (c === 'chatonly') setTimeout(() => window.__MOCK_EMIT('pindot:voice', { state: 'idle', notice: null }), 300);
    if (c === 'history') click('.chat-bar button', 500);
  });
})();
