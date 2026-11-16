// Vault screens for the screenshot tool (fake data only):
//   ?v=empty|list|add|edit|import|result|history|approval
// The page drives itself with clicks in this throw-away mock page; nothing real is touched.
(function () {
  const v = new URLSearchParams(location.search).get('v');
  if (!v) return;
  const t = (m) => new Date(Date.now() - m * 60000).toISOString();
  const L = (id, name, origin, username, has_2fa, last) => ({ id, name, origin, username, has_2fa, created: t(9000), updated: t(300), last_used: last ? t(last) : null, locked: false });
  const logins = v === 'empty' ? [] : [
    L('l1', 'Work reports', 'https://reports.example.com', 'sam@example.com', true, 12),
    L('l2', 'Demo shop', 'https://shop.example.org', 'sam.demo', false, 1500),
    L('l3', 'Local test page', 'http://localhost:4310', 'tester', true, null),
    L('l4', 'Team wiki', 'https://wiki.example.net:8443', 'sam', false, null),
  ];
  const U = (m, id, name, site, field, result, detail, ok, session) => ({ ts: t(m), login_id: id, login_name: name, site, field, result, detail, session, ok });
  const uses = [
    U(5, 'l1', 'Work reports', 'https://reports.example.com', 'code', 'typed', '', true, 'f73c7cb0'),
    U(6, 'l1', 'Work reports', 'https://reports.example.com', 'password', 'typed', '', true, 'f73c7cb0'),
    U(7, 'l1', 'Work reports', 'https://reports.example.com', 'username', 'typed', '', true, 'f73c7cb0'),
    U(95, 'l1', 'Work reports', 'https://reports.example.com', 'password', 'refused', 'the page in front is not https://reports.example.com', false, '9d21aa04'),
    U(1500, 'l2', 'Demo shop', 'https://shop.example.org', 'password', 'refused', 'you did not approve this login', false, '4be0c1e7'),
  ];
  const preview = {
    format: 'Chrome', total_rows: 14,
    columns: [{ field: 'name', header: 'name' }, { field: 'url', header: 'url' }, { field: 'username', header: 'username' }, { field: 'password', header: 'password' }],
    new_logins: 9, unchanged: 2, gain_key: 0, bad_authenticator: 0,
    conflicts: [{ id: 'l2', name: 'Demo shop', site: 'https://shop.example.org', username: 'sam.demo' }],
    skipped: [{ line: 4, reason: 'no website address' }, { line: 9, reason: 'no password' }],
  };
  const result = { imported: 9, replaced: 0, keys_added: 0, unchanged: 2, kept_existing: 1, skipped: 2, skipped_reasons: [] };
  const routes = {
    vault_state: () => ({ logins, place: 'macOS Keychain', problem: null }),
    vault_uses: (a) => (a && a.id ? uses.filter((u) => u.login_id === a.id) : uses),
    vault_import_choose: () => preview,
    vault_import_apply: () => result,
    vault_import_cancel: () => null,
  };
  if (v === 'approval') {
    routes.approvals_pending = () => [{ id: 'r9', session: 'f73c7cb0', apps: [], reason: '', login: { name: 'Work reports', site: 'https://reports.example.com', username: 'sam@example.com', field: 'password' } }];
    routes.chat_state = () => ({ session_id: '2f1c9a52-aaaa-4bbb-8ccc-0123456789ab', display: 'acting', can_send: true, items: [
      { kind: 'user', id: 'u1', text: 'Sign in to the reports site and download this week\'s numbers.', files: [] },
      { kind: 'step', id: 's1', name: 'mcp__desktop__vault_logins', summary: 'Work reports', result: null, is_error: false },
    ] });
  }
  window.__MOCK_ROUTES = Object.assign(window.__MOCK_ROUTES || {}, routes);
  const click = (sel, text) => [...document.querySelectorAll(sel)].find((b) => (sel.includes('primary') ? b.textContent.trim() === text : b.textContent.trim().startsWith(text)))?.click();
  const fill = (id, val) => { const e = document.getElementById(id); if (e) { e.value = val; e.dispatchEvent(new Event('input', { bubbles: true })); } };
  window.addEventListener('load', () => setTimeout(() => {
    if (v === 'add') {
      click('button', 'Add login');
      setTimeout(() => { fill('v-name', 'Work reports'); fill('v-site', 'https://reports.example.com'); fill('v-user', 'sam@example.com'); fill('v-pass', 'not-a-real-one'); fill('v-auth', 'JBSWY3DPEHPK3PXP'); }, 100);
    }
    if (v === 'edit') click('button', 'Edit');
    if (v === 'import') click('button', 'Import passwords');
    if (v === 'result') { click('button', 'Import passwords'); setTimeout(() => click('button.primary', 'Import'), 900); }
    if (v === 'history') click('button.link', 'Work reports');
  }, 500));
})();
