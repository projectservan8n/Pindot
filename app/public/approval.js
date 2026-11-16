// Shows one approval request. Every string from the request is inserted as
// text (never HTML): app names and the reason come from Claude.
(function () {
  const req = window.__PINDOT_APPROVAL__ || { apps: [], reason: '' };
  const login = req.login || null;
  const names = req.apps.map((a) => a.name);
  document.getElementById('title').textContent = names.join(', ') || 'an app';
  const list = document.getElementById('apps');
  for (const a of req.apps) {
    const box = document.createElement('div');
    box.className = 'app';
    const n = document.createElement('div');
    n.className = 'name';
    n.textContent = a.name;
    const m = document.createElement('div');
    m.className = 'meta';
    m.textContent = (a.bundle_id || 'unknown bundle') + (a.running ? ' · running' : ' · not running');
    box.append(n, m);
    if (a.warning) {
      const w = document.createElement('div');
      w.className = 'warn';
      w.textContent = '⚠ ' + a.warning;
      box.append(w);
    }
    list.append(box);
  }
  document.getElementById('reason').textContent = req.reason || '(no reason given)';
  if (login) {
    // Every login detail comes from the vault, not from Claude.
    document.title = 'Pindot: let Claude sign in?';
    document.getElementById('h1').textContent = 'Let Claude sign in with a saved login?';
    document.getElementById('appsection').hidden = true;
    document.getElementById('loginsection').hidden = false;
    document.getElementById('l-name').textContent = login.name;
    document.getElementById('l-site').textContent = login.site;
    document.getElementById('l-user').textContent = login.username || '(none)';
    document.getElementById('l-field').textContent = login.field === 'code' ? 'the current authenticator code' : 'the ' + login.field;
    document.getElementById('allow').textContent = 'Allow for this session';
  }
  const answer = (allow) => {
    document.querySelectorAll('button').forEach((b) => (b.disabled = true));
    window.__TAURI__.core.invoke('approval_answer', { allow });
  };
  document.getElementById('allow').addEventListener('click', () => answer(true));
  document.getElementById('deny').addEventListener('click', () => answer(false));
  // Keyboard: Esc denies. Enter does nothing, so a stray keypress never allows.
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') answer(false); });
})();
