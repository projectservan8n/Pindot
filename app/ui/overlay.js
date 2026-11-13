// One overlay panel covers one monitor. Every panel gets the same cursor
// events in global points (top-left of the primary display = 0,0) and runs
// the same animation; each draws the cursor only while it is over its own
// monitor, so a move between monitors glides across the edge.
(function () {
  const M = window.__DESKHAND_MONITOR__ || { index: 0, x: 0, y: 0, width: innerWidth, height: innerHeight };
  const cursor = document.getElementById('cursor');
  const label = document.getElementById('label');
  let pos = null; // current global position
  let frame = null;

  function place(g) {
    const lx = g.x - M.x;
    const ly = g.y - M.y;
    const inside = lx >= -40 && ly >= -40 && lx <= M.width + 40 && ly <= M.height + 40;
    cursor.style.transform = 'translate(' + lx + 'px,' + ly + 'px)';
    cursor.style.opacity = inside ? '1' : '0';
  }

  const ease = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

  function moveTo(from, to, ms, text) {
    if (frame) cancelAnimationFrame(frame);
    label.textContent = text || '';
    label.style.display = text ? 'block' : 'none';
    const start = from || pos || to;
    const t0 = performance.now();
    const step = (now) => {
      const t = Math.min(1, (now - t0) / Math.max(ms, 1));
      const e = ease(t);
      pos = { x: start.x + (to.x - start.x) * e, y: start.y + (to.y - start.y) * e };
      place(pos);
      frame = t < 1 ? requestAnimationFrame(step) : null;
    };
    frame = requestAnimationFrame(step);
  }

  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (!ev) return;
  ev.listen('deskhand:cursor', (e) => moveTo(e.payload.from, e.payload.to, e.payload.ms, e.payload.label));
  ev.listen('deskhand:acting', (e) => document.body.classList.toggle('acting', !!e.payload));
  ev.listen('deskhand:hide', () => { cursor.style.opacity = '0'; pos = null; });
  ev.emit('deskhand:overlay-ready', { index: M.index });
})();
