// The activity view: recent entries from the log, then live ones.
(function () {
  const rows = document.getElementById('rows');
  const T = window.__TAURI__;
  function add(e, fresh) {
    const tr = document.createElement('tr');
    if (!e.ok) tr.classList.add('fail');
    if (fresh) tr.classList.add('new');
    for (const [cls, text] of [['ts', (e.ts || '').replace('T', ' ').replace('Z', '')], ['tool', e.tool],
      ['args', e.args], ['outcome', e.outcome], ['session', e.session]]) {
      const td = document.createElement('td');
      td.className = cls;
      td.textContent = text || '';
      tr.append(td);
    }
    rows.prepend(tr);
    while (rows.children.length > 1000) rows.lastChild.remove();
  }
  T.core.invoke('activity_recent').then((r) => {
    document.getElementById('path').textContent = r.path ? 'Log file: ' + r.path : 'Memory only';
    for (const e of r.entries) add(e, false);
  });
  T.event.listen('deskhand:activity', (ev) => add(ev.payload, true));
})();
