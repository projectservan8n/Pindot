// Overlay harness, injected at document start (wksnap inject.js).
// Stands in for Tauri's event bridge so overlay.js can be driven by hand:
// window.__fire(name, payload). Only for the screenshot tool.
(function () {
  const H = {};
  window.__logs = window.__logs || [];
  window.__TAURI__ = { event: {
    listen: (n, cb) => { (H[n] = H[n] || []).push(cb); return Promise.resolve(() => {}); },
    emit: (n) => { window.__logs.push('emit ' + n); return Promise.resolve(); } } };
  window.__fire = (n, p) => (H[n] || []).forEach((f) => f({ payload: p }));
  // ?fast=1: the cursor's slow timers (20 s until sleepy, 3 to 7 s between
  // blinks) run in a fraction of the time so a short strip can show them.
  if (new URLSearchParams(location.search).get('fast')) {
    const st = window.setTimeout;
    window.setTimeout = (f, ms, ...r) => st(f, ms === 20000 ? 500 : (ms >= 3000 && ms < 7000 ? 250 : ms), ...r);
  }
  document.documentElement.style.background = '#8a8f98';
})();
