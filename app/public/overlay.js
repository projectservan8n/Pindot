// Deskhand's AI cursor. One overlay panel covers one monitor. Every panel
// gets the same events in global points (top-left of the primary display =
// 0,0) and runs the same animation; each draws only what falls on its own
// monitor, so a move between monitors glides across the edge.
//
// Events (app/src-tauri/src/overlay.rs):
//   deskhand:cursor  {from, to, ms, label}  glide; the core acts after `ms` + 60 ms
//   deskhand:action  {kind: click|press|scroll|drag|key|type|point, ...}
//   deskhand:acting  bool         deskhand:stopped        deskhand:hide
//   deskhand:voice   {state, notice}
(function () {
  'use strict';
  const M = window.__DESKHAND_MONITOR__ || { index: 0, x: 0, y: 0, width: innerWidth, height: innerHeight };
  const $ = (id) => document.getElementById(id);
  const body = document.body;
  const cursor = $('cursor');
  const pose = $('pose');
  const lean = $('lean');
  const halo = $('halo');
  const tag = $('tag');
  const verbEl = $('verb');
  const nameEl = $('name');
  const hud = $('hud');
  const fx = $('fx');
  const trailEl = $('trail');
  const trailGrad = $('trail-grad');
  const dragPath = $('drag-path');
  const dragUnder = $('drag-under');

  const motionPref = matchMedia('(prefers-reduced-motion: reduce)');
  let reduced = motionPref.matches;
  motionPref.addEventListener('change', (e) => { reduced = e.matches; });

  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const local = (g) => ({ x: g.x - M.x, y: g.y - M.y });
  const onMonitor = (g, pad) => {
    const l = local(g);
    return l.x >= -pad && l.y >= -pad && l.x <= M.width + pad && l.y <= M.height + pad;
  };
  const SNAP = 'cubic-bezier(0.2, 0, 0, 1)';
  const SPRING = 'cubic-bezier(0.34, 1.56, 0.64, 1)';

  // ---------------------------------------------------------------- motion
  let pos = null;          // the tip, global points
  let vel = { x: 0, y: 0 }; // smoothed, px per ms
  let glide = null;        // {a, c, b, dir, len, t0, ms}
  let drag = null;         // {a, b, t0, ms}
  let waiting = [];        // actions that play once the cursor lands
  const trail = [];        // recent tips {x, y, t}
  const TRAIL_MS = 170;
  let tagOff = null;       // the tag's current offset from the tip
  let tagW = 0;
  let raf = 0;
  let last = 0;

  // A hand moves in an arc, not along a ruler: bow the path upwards by up to
  // 14% of its length (90 px at most).
  function arcControl(a, b) {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const d = Math.hypot(dx, dy);
    if (d < 1) return { x: a.x, y: a.y };
    let nx = -dy / d;
    let ny = dx / d;
    if (ny > 0) { nx = -nx; ny = -ny; }
    const bow = Math.min(d * 0.14, 90);
    return { x: (a.x + b.x) / 2 + nx * bow, y: (a.y + b.y) / 2 + ny * bow };
  }
  function quad(a, c, b, t) {
    const u = 1 - t;
    return { x: u * u * a.x + 2 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2 * u * t * c.y + t * t * b.y };
  }
  const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
  // In the last fifth the tip runs a few points past the target and settles
  // back, ending exactly on it before the core clicks.
  const settle = (t, len) => (t < 0.8 ? 0 : Math.sin((Math.PI * (t - 0.8)) / 0.2) * Math.min(4, len * 0.02));

  function kick() {
    if (!raf) { last = 0; raf = requestAnimationFrame(tick); }
  }

  function tick(now) {
    raf = 0;
    const dt = last ? Math.min(now - last, 50) : 16;
    last = now;
    const prev = pos;
    let moving = false;

    if (glide) {
      const t = clamp((now - glide.t0) / glide.ms, 0, 1);
      const p = quad(glide.a, glide.c, glide.b, easeInOut(t));
      const s = settle(t, glide.len);
      pos = { x: p.x + glide.dir.x * s, y: p.y + glide.dir.y * s };
      moving = true;
      if (t >= 1) { pos = glide.b; glide = null; land(); }
    } else if (drag) {
      // The real pointer moves in a straight line at a steady pace; follow it.
      const t = (now - drag.t0) / drag.ms;
      moving = true;
      if (t >= 0) {
        const k = clamp(t, 0, 1);
        pos = { x: drag.a.x + (drag.b.x - drag.a.x) * k, y: drag.a.y + (drag.b.y - drag.a.y) * k };
        drawDragPath(drag.a, pos);
        if (t >= 1) { drag = null; release(); }
      }
    }

    if (prev && pos && dt > 0) {
      const vx = (pos.x - prev.x) / dt;
      const vy = (pos.y - prev.y) / dt;
      vel = { x: vel.x * 0.72 + vx * 0.28, y: vel.y * 0.72 + vy * 0.28 };
    }
    if (!moving) { vel = { x: vel.x * 0.78, y: vel.y * 0.78 }; }
    if (moving && pos && !reduced) trail.push({ x: pos.x, y: pos.y, t: now });
    while (trail.length && now - trail[0].t > TRAIL_MS) trail.shift();

    let springing = false;
    if (pos) springing = render();
    drawTrail(now);

    const still = Math.abs(vel.x) + Math.abs(vel.y) < 0.004;
    if (glide || drag || trail.length || springing || !still) raf = requestAnimationFrame(tick);
  }

  // Draws the cursor at `pos`; returns true while the tag is still swinging.
  function render() {
    const l = local(pos);
    cursor.style.transform = 'translate3d(' + l.x + 'px,' + l.y + 'px,0)';
    cursor.classList.toggle('away', !onMonitor(pos, 40));
    const speed = Math.hypot(vel.x, vel.y);
    // Bank into horizontal motion like a paper plane; swell a little at speed.
    const bank = reduced ? 0 : clamp(vel.x * 11, -18, 18);
    const grow = reduced ? 1 : 1 + Math.min(speed * 0.05, 0.08);
    lean.style.transform = 'rotate(' + bank.toFixed(2) + 'deg) scale(' + grow.toFixed(3) + ')';

    // The tag hangs off the arrow and swings behind the motion.
    const base = tagBase(l);
    const goal = reduced ? base : {
      x: base.x - clamp(vel.x * 18, -20, 20),
      y: base.y - clamp(vel.y * 14, -16, 16),
    };
    if (!tagOff || reduced) tagOff = { x: goal.x, y: goal.y };
    tagOff = { x: tagOff.x + (goal.x - tagOff.x) * 0.24, y: tagOff.y + (goal.y - tagOff.y) * 0.24 };
    const tilt = reduced ? 0 : clamp(-vel.x * 6, -8, 8);
    tag.style.transform = 'translate3d(' + tagOff.x.toFixed(1) + 'px,' + tagOff.y.toFixed(1) + 'px,0) rotate(' + tilt.toFixed(2) + 'deg)';
    return Math.abs(goal.x - tagOff.x) + Math.abs(goal.y - tagOff.y) > 0.3;
  }

  // Below-right of the tip; flipped so the tag never leaves the monitor.
  function tagBase(l) {
    const flipX = l.x + 22 + tagW > M.width - 12;
    const flipY = l.y + 26 + 28 > M.height - 12;
    tag.classList.toggle('flip-x', flipX);
    tag.classList.toggle('flip-y', flipY);
    return { x: flipX ? -tagW - 8 : 20, y: flipY ? -32 : 24 };
  }

  // One tapered ribbon from the oldest point (a hair) to the tip (7 pt),
  // fading along its length and with age.
  function drawTrail(now) {
    const n = trail.length;
    if (n < 2) { trailEl.setAttribute('d', ''); return; }
    const pts = trail.map(local);
    const left = [];
    const right = [];
    for (let i = 0; i < n; i++) {
      const a = pts[Math.max(i - 1, 0)];
      const b = pts[Math.min(i + 1, n - 1)];
      const tl = Math.hypot(b.x - a.x, b.y - a.y) || 1;
      const tx = (b.x - a.x) / tl;
      const ty = (b.y - a.y) / tl;
      const fresh = clamp(1 - (now - trail[i].t) / TRAIL_MS, 0, 1);
      const w = 3.6 * (i / (n - 1)) * fresh;
      left.push((pts[i].x - ty * w).toFixed(1) + ' ' + (pts[i].y + tx * w).toFixed(1));
      right.push((pts[i].x + ty * w).toFixed(1) + ' ' + (pts[i].y - tx * w).toFixed(1));
    }
    trailEl.setAttribute('d', 'M' + left.join(' L') + ' L' + right.reverse().join(' L') + ' Z');
    trailGrad.setAttribute('x1', pts[0].x);
    trailGrad.setAttribute('y1', pts[0].y);
    trailGrad.setAttribute('x2', pts[n - 1].x);
    trailGrad.setAttribute('y2', pts[n - 1].y);
  }

  function drawDragPath(a, b) {
    const la = local(a);
    const lb = local(b);
    const d = 'M' + la.x + ' ' + la.y + ' L' + lb.x + ' ' + lb.y;
    dragPath.setAttribute('d', d);
    dragUnder.setAttribute('d', d);
  }

  function showCursor(at) {
    pos = at;
    tagOff = null;
    if (!body.classList.contains('cursor-on')) {
      body.classList.add('cursor-on');
      // Materialise: drop in from slightly larger, like a hand landing.
      pose.animate(
        reduced ? [{ opacity: 0 }, { opacity: 1 }]
                : [{ opacity: 0, transform: 'scale(1.7)' }, { opacity: 1, transform: 'scale(1)' }],
        { duration: 260, easing: SNAP });
    }
  }

  function hideCursor() {
    glide = null;
    drag = null;
    waiting = [];
    trail.length = 0;
    drawTrail(performance.now());
    body.classList.remove('dragging');
    hud.classList.remove('show');
    if (body.classList.contains('cursor-on')) {
      const out = pose.animate(
        reduced ? [{ opacity: 1 }, { opacity: 0 }]
                : [{ opacity: 1, transform: 'scale(1)' }, { opacity: 0, transform: 'scale(0.5)' }],
        { duration: 200, easing: SNAP });
      out.onfinish = () => { if (!glide) body.classList.remove('cursor-on'); };
    }
    pos = null;
  }

  // ------------------------------------------------------------- the glide
  function onGlide(p) {
    body.classList.remove('resting');
    setLabel(p.label || '');
    const to = p.to;
    const start = pos || p.from;
    if (!start || reduced) {
      if (pos && reduced) {
        // Reduced motion: no travel, a quick crossfade at the target.
        pose.animate([{ opacity: 0.2 }, { opacity: 1 }], { duration: 160, easing: 'ease-out' });
      }
      glide = null;
      showCursor(to);
      kick();
      setTimeout(land, Math.max(p.ms - 20, 0));
      return;
    }
    if (!pos) showCursor(start);
    const c = arcControl(start, to);
    const tx = to.x - c.x;
    const ty = to.y - c.y;
    const tl = Math.hypot(tx, ty) || 1;
    glide = {
      a: start, c, b: to,
      dir: { x: tx / tl, y: ty / tl },
      len: Math.hypot(to.x - start.x, to.y - start.y),
      t0: performance.now(),
      ms: Math.max(p.ms, 1),
    };
    kick();
  }

  function land() {
    const q = waiting;
    waiting = [];
    q.forEach(play);
  }

  // ------------------------------------------------------------- the tag
  function setLabel(text) {
    // The core labels scroll and drag glides with the bare verb.
    const verbOnly = text === 'scroll' || text === 'drag';
    // A click at x,y is labelled "(812, 344)": show the numbers plainly.
    const coords = /^\(\s*-?\d+(\.\d+)?,\s*-?\d+(\.\d+)?\s*\)$/.test(text);
    verbEl.textContent = verbOnly ? text[0].toUpperCase() + text.slice(1) : '';
    nameEl.textContent = verbOnly ? '' : coords ? text.slice(1, -1).trim() : text;
    nameEl.classList.toggle('coords', coords);
    refreshTag();
  }

  function setVerb(v) {
    if (verbEl.textContent === v) return;
    verbEl.textContent = v;
    refreshTag();
    if (!reduced) {
      verbEl.animate([{ opacity: 0, transform: 'translateY(4px)' }, { opacity: 1, transform: 'none' }],
        { duration: 160, easing: SNAP });
    }
  }

  function refreshTag() {
    const empty = !verbEl.textContent && !nameEl.textContent;
    tag.classList.toggle('empty', empty);
    tag.classList.toggle('no-verb', !verbEl.textContent);
    body.classList.remove('resting');
    tagW = tag.offsetWidth;
    if (pos) kick();
  }

  // ------------------------------------------------------------- actions
  function onAction(a) {
    body.classList.remove('resting');
    if (glide) { waiting.push(a); return; } // e.g. point: show it once landed
    play(a);
  }

  function play(a) {
    switch (a.kind) {
      case 'click': return click(a);
      case 'press': return press();
      case 'scroll': return scroll(a);
      case 'drag': return startDrag(a);
      case 'key': return keys(a);
      case 'type': return typing(a);
      case 'point': return spotlight();
    }
  }

  // A mark on screen at a global point; removed when its animation ends.
  function mark(cls, at, frames, opts) {
    if (!onMonitor(at, 120)) return null;
    const el = document.createElement('div');
    el.className = 'mark ' + cls;
    const l = local(at);
    el.style.left = l.x + 'px';
    el.style.top = l.y + 'px';
    fx.appendChild(el);
    const anim = el.animate(frames, Object.assign({ fill: 'both' }, opts));
    anim.onfinish = () => el.remove();
    return el;
  }
  const fade = (from) => [{ opacity: from }, { opacity: 0 }];

  const CLICK_VERBS = { left: ['Click', 'Double-click', 'Triple-click'], right: ['Right-click'], middle: ['Middle-click'] };

  function click(a) {
    const n = clamp(a.count || 1, 1, 3);
    const verbs = CLICK_VERBS[a.button] || CLICK_VERBS.left;
    setVerb(verbs[Math.min(n, verbs.length) - 1]);
    if (!pos) return;
    const at = pos;
    const right = a.button === 'right';
    for (let i = 0; i < n; i++) {
      setTimeout(() => {
        // The real pointer clicked: a ripple spreads out and the arrow taps.
        mark(right ? 'ring ring-right' : 'ring ring-click', at,
          reduced ? fade(0.9)
                  : [{ transform: 'scale(0.2)', opacity: 1 }, { transform: 'scale(1)', opacity: 0 }],
          { duration: 440, easing: SNAP });
        mark('tip-dot', at, reduced ? fade(1) : [{ transform: 'scale(1.2)', opacity: 1 }, { transform: 'scale(0)', opacity: 0 }],
          { duration: 220, easing: 'ease-in' });
        tap();
      }, i * 120);
    }
  }

  function tap() {
    if (reduced) return;
    pose.animate([{ transform: 'scale(1)' }, { transform: 'scale(0.8)', offset: 0.3 }, { transform: 'scale(1)' }],
      { duration: 240, easing: SNAP });
  }

  function press() {
    setVerb('Press');
    if (!pos) return;
    // Pressed through accessibility, the user's pointer untouched: the ring
    // gathers inwards instead of spreading out, and the arrow glows.
    mark('ring ring-press', pos,
      reduced ? fade(0.9)
              : [{ transform: 'scale(1.25)', opacity: 0 }, { transform: 'scale(0.42)', opacity: 1, offset: 0.62 },
                 { transform: 'scale(0.3)', opacity: 0 }],
      { duration: 420, easing: SNAP });
    halo.animate([{ opacity: 0.3, transform: 'scale(1)' }, { opacity: 0.95, transform: 'scale(1.55)', offset: 0.45 },
                  { opacity: 0.3, transform: 'scale(1)' }], { duration: 460, easing: 'ease-out' });
  }

  const ARROW = { up: '↑', down: '↓', left: '←', right: '→' };

  function scroll(a) {
    const dir = a.dy < 0 ? 'up' : a.dy > 0 ? 'down' : a.dx < 0 ? 'left' : 'right';
    setVerb('Scroll ' + ARROW[dir]);
    if (!pos) return;
    const v = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] }[dir];
    const angle = { down: 0, up: 180, left: 90, right: -90 }[dir];
    for (let i = 0; i < 3; i++) {
      const at = { x: pos.x + v[0] * (16 + i * 10) + (v[0] ? 0 : -10), y: pos.y + v[1] * (16 + i * 10) + (v[1] ? 0 : 14) };
      const el = mark('chev', at,
        reduced ? [{ opacity: 0 }, { opacity: 1, offset: 0.4 }, { opacity: 0 }]
                : [{ opacity: 0, transform: 'rotate(' + angle + 'deg) translateY(-6px)' },
                   { opacity: 1, transform: 'rotate(' + angle + 'deg) translateY(0)', offset: 0.4 },
                   { opacity: 0, transform: 'rotate(' + angle + 'deg) translateY(8px)' }],
        { duration: 520, delay: i * 70, easing: 'ease-out' });
      if (el) el.innerHTML = '<svg width="16" height="10" viewBox="0 0 16 10"><polyline points="2,2 8,8 14,2" fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/><polyline points="2,2 8,8 14,2" fill="none" stroke="#ff2d95" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    }
  }

  let grab = null;

  function startDrag(a) {
    setVerb('Drag');
    if (!pos) showCursor(a.to);
    // The core holds the button for about 140 ms before the pointer travels.
    drag = { a: pos, b: a.to, t0: performance.now() + 140, ms: Math.max(a.ms, 1) };
    drawDragPath(pos, pos);
    body.classList.add('dragging');
    if (!reduced) grab = pose.animate([{ transform: 'scale(1)' }, { transform: 'scale(0.86)' }],
      { duration: 140, easing: SNAP, fill: 'forwards' });
    kick();
  }

  function release() {
    body.classList.remove('dragging');
    if (grab) { grab.cancel(); grab = null; }
    if (!reduced) {
      pose.animate([{ transform: 'scale(0.86)' }, { transform: 'scale(1.08)', offset: 0.55 }, { transform: 'scale(1)' }],
        { duration: 300, easing: SNAP });
    }
    if (pos) mark('ring ring-drop', pos, reduced ? fade(0.9) : [{ transform: 'scale(0.4)', opacity: 1 }, { transform: 'scale(1.6)', opacity: 0 }],
      { duration: 380, easing: SNAP });
  }

  function spotlight() {
    if (!pos) return;
    for (let i = 0; i < 2; i++) {
      mark('ring ring-spot', pos,
        reduced ? fade(0.8)
                : [{ transform: 'scale(0.55)', opacity: 0.95 }, { transform: 'scale(1.35)', opacity: 0 }],
        { duration: 760, delay: i * 220, easing: SNAP });
    }
    if (!reduced) {
      pose.animate([{ transform: 'rotate(0deg)' }, { transform: 'rotate(-12deg)', offset: 0.35 }, { transform: 'rotate(0deg)' }],
        { duration: 420, easing: SPRING });
    }
  }

  // ------------------------------------------------------------- keys, typing
  let hudTimer = null;

  function placeHud() {
    const w = hud.offsetWidth;
    if (pos && onMonitor(pos, 0)) {
      // Keys and typing take the tag's place beside the arrow.
      const l = local(pos);
      const left = clamp(l.x + 20, 12, M.width - w - 12);
      const top = l.y + 26 + hud.offsetHeight > M.height - 12 ? l.y - 16 - hud.offsetHeight : l.y + 26;
      hud.style.transform = 'translate(' + left + 'px,' + top + 'px)';
      return true;
    }
    if (M.index === 0) { // no cursor here: under the banner on the main display
      hud.style.transform = 'translate(' + (M.width / 2 - w / 2) + 'px, 92px)';
      return true;
    }
    return false;
  }

  function showHud(content, ms) {
    hud.replaceChildren(content);
    hud.classList.add('show');
    if (!placeHud()) { hud.classList.remove('show'); return; }
    if (hudTimer) clearTimeout(hudTimer);
    hudTimer = setTimeout(() => {
      const out = hud.animate(reduced ? fade(1) : [{ opacity: 1, transform: hud.style.transform },
        { opacity: 0, transform: hud.style.transform + ' translateY(-6px)' }], { duration: 220, easing: 'ease-in' });
      out.onfinish = () => hud.classList.remove('show');
    }, ms);
  }

  function keys(a) {
    setLabel('');
    const row = document.createElement('div');
    row.className = 'keys';
    const caps = (a.keys || []).map((k, i) => {
      const cap = document.createElement('span');
      cap.className = 'cap' + (k.length > 1 && !/^F\d+$/.test(k) && !/^[^\w\s]$/.test(k) ? ' word' : '');
      cap.textContent = k;
      row.appendChild(cap);
      return cap;
    });
    if (a.repeat > 1) {
      const t = document.createElement('span');
      t.className = 'times';
      t.textContent = '×' + a.repeat;
      row.appendChild(t);
    }
    showHud(row, 1500);
    if (reduced) return;
    // Caps pop in one by one, then go down together like a real chord.
    caps.forEach((cap, i) => cap.animate(
      [{ opacity: 0, transform: 'translateY(8px) scale(0.7)' }, { opacity: 1, transform: 'none' }],
      { duration: 200, delay: i * 45, easing: SPRING, fill: 'backwards' }));
    const down = caps.length * 45 + 200;
    caps.forEach((cap) => cap.animate(
      [{ transform: 'none', borderBottomWidth: '3px' }, { transform: 'translateY(2px)', borderBottomWidth: '1px', offset: 0.4 },
       { transform: 'none', borderBottomWidth: '3px' }],
      { duration: 220, delay: down, easing: 'ease-out' }));
  }

  function typing(a) {
    setLabel('');
    const chip = document.createElement('div');
    chip.className = 'chip';
    chip.innerHTML = '<svg width="18" height="12" viewBox="0 0 18 12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="0.8" y="0.8" width="16.4" height="10.4" rx="2.2"/><path d="M4 4.2h.01M7 4.2h.01M10 4.2h.01M13.4 4.2h.01M5 8h8"/></svg>';
    const word = document.createElement('span');
    word.textContent = 'Typing';
    const caret = document.createElement('span');
    caret.className = 'caret';
    const count = document.createElement('span');
    count.className = 'count';
    const n = a.chars || 0;
    count.textContent = n + (n === 1 ? ' character' : ' characters');
    chip.append(word, caret, count);
    showHud(chip, clamp(n * 14, 1100, 3200));
    if (!reduced) chip.animate([{ opacity: 0, transform: 'translateY(6px)' }, { opacity: 1, transform: 'none' }],
      { duration: 200, easing: SNAP });
  }

  // ------------------------------------------------------------- events
  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (!ev) return;
  ev.listen('deskhand:cursor', (e) => onGlide(e.payload));
  ev.listen('deskhand:action', (e) => onAction(e.payload || {}));

  const bannerText = $('banner-text');
  const ACTING = 'Claude is controlling input';
  let stoppedTimer = null;
  let restTimer = null;
  ev.listen('deskhand:acting', (e) => {
    if (e.payload) {
      body.classList.remove('stopped', 'resting');
      bannerText.textContent = ACTING;
      if (restTimer) clearTimeout(restTimer);
    } else {
      // Between actions the cursor stays but rests: dimmer, tag tucked away.
      if (restTimer) clearTimeout(restTimer);
      restTimer = setTimeout(() => { if (!glide && !drag) body.classList.add('resting'); }, 1600);
    }
    body.classList.toggle('acting', !!e.payload);
  });
  ev.listen('deskhand:stopped', () => {
    if (stoppedTimer) clearTimeout(stoppedTimer);
    body.classList.remove('acting');
    body.classList.add('stopped');
    bannerText.textContent = 'Stopped · Claude no longer controls input';
    hideCursor();
    stoppedTimer = setTimeout(() => body.classList.remove('stopped'), 2500);
  });
  ev.listen('deskhand:hide', hideCursor);

  // Voice pill. A notice (e.g. "run `claude login`") stays for a few
  // seconds; pressing the hotkey again replaces it.
  const pill = $('voice');
  const pillText = $('voice-text');
  const LABELS = { listening: 'Listening', thinking: 'Thinking', acting: 'Working', speaking: 'Speaking' };
  let voiceState = 'idle';
  let notice = null;
  let noticeTimer = null;
  function renderVoice() {
    pill.dataset.state = voiceState;
    pillText.textContent = notice || LABELS[voiceState] || '';
    pill.classList.toggle('notice', !!notice);
    pill.classList.toggle('show', !!notice || voiceState !== 'idle');
  }
  ev.listen('deskhand:voice', (e) => {
    const s = e.payload || {};
    const state = s.state || 'idle';
    if (s.notice) {
      notice = s.notice;
      if (noticeTimer) clearTimeout(noticeTimer);
      noticeTimer = setTimeout(() => { notice = null; renderVoice(); }, 7000);
    } else if (state === 'listening') {
      notice = null;
    }
    voiceState = state;
    renderVoice();
  });
  ev.emit('deskhand:overlay-ready', { index: M.index });
})();
