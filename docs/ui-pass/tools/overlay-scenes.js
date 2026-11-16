// Overlay harness scenes (wksnap after.js): ?scene=<name> plays the events the
// Rust side sends, on a timeline. Only for the screenshot tool.
(function () {
  const q = new URLSearchParams(location.search);
  const scene = q.get('scene') || 'glide';
  const color = q.get('color');
  const F = window.__fire;
  const at = (ms, fn) => setTimeout(fn, ms);
  const P = (x, y) => ({ x, y });
  const stay = (x, y) => F('pindot:cursor', { from: P(x, y), to: P(x, y), ms: 40, label: '' });
  if (color) F('pindot:cursor-color', color);
  const mate = (x, y, ms) => { const t = setInterval(() => F('pindot:companion', { x, y, level: 0.35 + 0.3 * Math.abs(Math.sin(Date.now() / 140)) }), 80); at(ms, () => clearInterval(t)); };
  const S = {
    glide: () => F('pindot:cursor', { from: P(40, 220), to: P(380, 90), ms: 900, label: 'Open Mail' }),
    click: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'click', to: P(200, 120), button: 'left', count: 1 })); },
    doubleclick: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'click', to: P(200, 120), button: 'left', count: 2 })); },
    press: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'press', to: P(200, 120) })); },
    scroll: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'scroll', to: P(200, 120), dy: 5, dx: 0 })); },
    drag: () => { stay(90, 80); at(300, () => F('pindot:action', { kind: 'drag', to: P(380, 200), ms: 700 })); },
    key: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'key', to: P(200, 120), keys: ['Cmd', 'S'], repeat: 1 })); },
    type: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'type', to: P(200, 120), chars: 24 })); },
    point: () => { stay(200, 120); at(300, () => F('pindot:action', { kind: 'point', to: P(200, 120) })); },
    companion: () => { F('pindot:voice', { state: 'listening' }); mate(120, 90, 3000); },
    thinking: () => { F('pindot:voice', { state: 'thinking' }); mate(120, 90, 3000); },
    caption: () => {
      stay(120, 80); F('pindot:voice', { state: 'speaking' });
      at(100, () => F('pindot:caption-timing', { seq: 1, secs: 4 }));
      at(150, () => F('pindot:caption', { seq: 1, text: 'Opening your inbox now. I found three unread messages from Sam and one from the bank.' }));
      at(4200, () => F('pindot:caption-end', { seq: 1, interrupted: false }));
    },
    // caption near the bottom of the screen, where the voice pill sits
    captionlow: () => {
      stay(250, 230); F('pindot:voice', { state: 'speaking' });
      at(150, () => F('pindot:caption', { seq: 1, text: 'Opening your inbox now, one second.' }));
    },
    captionnocursor: () => {
      F('pindot:voice', { state: 'speaking' });
      at(150, () => F('pindot:caption', { seq: 1, text: 'Opening your inbox now, one second.' }));
    },
    color: () => stay(200, 120),
    acting: () => { stay(200, 120); F('pindot:acting', true); },
    stopped: () => { stay(200, 120); F('pindot:acting', true); at(300, () => F('pindot:stopped')); },
    hide: () => { stay(200, 120); at(300, () => F('pindot:hide')); },
    resting: () => { stay(200, 120); F('pindot:acting', true); at(200, () => F('pindot:acting', false)); },
    eyes: () => {
      // travel, then sample the eyes every 25 ms into the log
      stay(40, 220); at(200, () => F('pindot:cursor', { from: P(40, 220), to: P(440, 80), ms: 1600, label: '' }));
      const e = document.getElementById('eyes'), r = document.querySelectorAll('.eye');
      const seen = { tx: [], blinks: 0 }; let was = false;
      const t = setInterval(() => {
        const m = new DOMMatrix(getComputedStyle(e).transform);
        seen.tx.push(+m.e.toFixed(2));
        const b = r[0].classList.contains('blink'); if (b && !was) seen.blinks++; was = b;
      }, 25);
      at(3800, () => { clearInterval(t); window.__logs.push('eyes ' + JSON.stringify({ min: Math.min(...seen.tx), max: Math.max(...seen.tx), blinks: seen.blinks })); });
    },
  };
  (S[scene] || S.glide)();
})();
