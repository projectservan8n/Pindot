// Pindot's AI cursor. One overlay panel covers one monitor. Every panel
// gets the same events in global points (top-left of the primary display =
// 0,0) and runs the same animation; each draws only what falls on its own
// monitor, so a move between monitors glides across the edge.
//
// Events (app/src-tauri/src/overlay.rs):
//   pindot:cursor  {from, to, ms, label}  glide; the core acts after `ms` + 60 ms
//   pindot:action  {kind: click|press|scroll|drag|key|type|point, ...}
//   pindot:acting  bool         pindot:stopped        pindot:hide
//   pindot:voice   {state, notice}
//   pindot:companion {x, y, level}   the user's pointer and mic level, ~60 Hz
//                    while a voice request is in progress: the cursor sits beside the user's pointer
//   pindot:caption {seq, text}       pindot:caption-end {seq, interrupted}
//   pindot:caption-timing {seq, secs}  the reply's real audio length
(function () {
  'use strict';
  const M = window.__PINDOT_MONITOR__ || { index: 0, x: 0, y: 0, width: innerWidth, height: innerHeight };
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
  const bars = $('buddy').children;
  const caption = $('caption');
  const captionText = $('caption-text');

  const arrow = $('arrow');
  const eyesEl = $('eyes');
  const eyeRects = arrow.querySelectorAll('.eye');

  const motionPref = matchMedia('(prefers-reduced-motion: reduce)');
  let reduced = motionPref.matches;
  motionPref.addEventListener('change', (e) => { reduced = e.matches; if (!reduced) return; look(0, 0); });

  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  // Points arrive as global points. Inside this page everything is in CSS
  // pixels: UNIT points make one (1 on macOS; on Windows a point is a
  // physical pixel, so UNIT is the monitor's scale).
  const UNIT = M.unit > 0 ? M.unit : 1;
  const px = (p) => (p && typeof p.x === 'number' ? Object.assign({}, p, { x: p.x / UNIT, y: p.y / UNIT }) : p);
  const ORIGIN = { x: M.x / UNIT, y: M.y / UNIT };
  const local = (g) => ({ x: g.x - ORIGIN.x, y: g.y - ORIGIN.y });
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

  // Companion: during a voice request the cursor sits beside the user's pointer, flies
  // off to act, holds on the target a moment, then comes back.
  const OFF = { x: 16, y: 18 };
  let mate = null;          // the user's pointer, global points
  let mateOn = false;
  let mateTimer = null;
  let holdUntil = 0;
  // After a `point` the cursor waits on the spot until the user's pointer
  // gets there (the app watches it and sends pindot:unpark), instead of
  // flying back while the user is still looking. The next glide, Esc or the
  // user talking again end the wait too; the timer only backs up the app's
  // two-minute limit.
  let parked = false;
  let parkTimer = null;
  const PARK_FALLBACK_MS = 125000;
  let following = false;
  let level = 0;            // smoothed mic level, 0..1
  let voiceState = 'idle';

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
      if (t >= 1) {
        // An action follows a glide to a target: stay there for it.
        if (!glide.home) holdUntil = now + 1500;
        pos = glide.b;
        glide = null;
        land();
      }
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

    let chasing = false;
    const nowFollowing = !!(mateOn && mate && pos && !glide && !drag && now >= holdUntil
      && !parked && !body.classList.contains('acting'));
    if (nowFollowing !== following) {
      following = nowFollowing;
      if (following) setLabel('');
      updateBuddy();
    }
    if (following) {
      const tgt = { x: mate.x + OFF.x, y: mate.y + OFF.y };
      const d = Math.hypot(tgt.x - pos.x, tgt.y - pos.y);
      if (d > 140 && !reduced) {
        // A long way back from a target: fly home on an arc.
        startGlide(pos, tgt, clamp(d * 0.55, 260, 520));
        glide.home = true;
        moving = true;
      } else {
        const k = reduced ? 1 : 1 - Math.exp(-dt / 60);
        pos = { x: pos.x + (tgt.x - pos.x) * k, y: pos.y + (tgt.y - pos.y) * k };
        chasing = d > 0.4;
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
    if (glide || drag || trail.length || springing || chasing || !still) raf = requestAnimationFrame(tick);
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
    // While he travels the eyes sweep left and right (one full sweep every
    // SWEEP_MS, always starting from centre), leaning a little into the
    // direction of travel on top (same smoothed velocity).
    if (speed > 0.02 && !reduced) {
      const t = performance.now();
      if (!sweepFrom) sweepFrom = t;
      const k = Math.min(speed * 8, 1);
      const sweep = Math.sin((t - sweepFrom) / SWEEP_MS * 2 * Math.PI) * SWEEP_REACH;
      look(sweep + vel.x / speed * k * 0.4, vel.y / speed * k);
    } else {
      sweepFrom = 0;
      look(0, 0);
    }

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
    placeCaption();
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
    startGlide(start, to, p.ms);
    kick();
  }

  function startGlide(start, to, ms) {
    const c = arcControl(start, to);
    const tx = to.x - c.x;
    const ty = to.y - c.y;
    const tl = Math.hypot(tx, ty) || 1;
    glide = {
      a: start, c, b: to,
      dir: { x: tx / tl, y: ty / tl },
      len: Math.hypot(to.x - start.x, to.y - start.y),
      t0: performance.now(),
      ms: Math.max(ms, 1),
    };
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
    const empty = !verbEl.textContent && !nameEl.textContent && !tag.dataset.buddy;
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
    // Stay on the target after the action; a point stays long enough to find.
    holdUntil = Math.max(holdUntil, performance.now() + (a.kind === 'point' ? 4000 : 900));
    setTimeout(kick, a.kind === 'point' ? 4020 : 920);
    // Beside the user's pointer (the companion), a point waits for them.
    if (a.kind === 'point' && mateOn) park();
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
    squint(220);
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
      if (el) el.innerHTML = '<svg width="16" height="10" viewBox="0 0 16 10"><polyline points="2,2 8,8 14,2" fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/><polyline points="2,2 8,8 14,2" fill="none" class="chev-stroke" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
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

  // ------------------------------------------------------------- companion
  // The tag shows the user's voice while they talk and three dots while
  // Claude thinks; action labels take it back when the cursor flies off.
  function updateBuddy() {
    const mode = following ? (voiceState === 'listening' ? 'listening' : voiceState === 'thinking' ? 'thinking' : '') : '';
    if ((tag.dataset.buddy || '') === mode) return;
    if (mode) tag.dataset.buddy = mode;
    else delete tag.dataset.buddy;
    for (const b of bars) b.style.height = '';
    refreshTag();
  }

  const SHAPE = [0.55, 0.85, 1, 0.8, 0.5];
  function drawLevel(rms) {
    // Speech sits around -40..-18 dBFS on a laptop mic.
    const db = 20 * Math.log10(rms + 1e-5);
    const goal = clamp((db + 54) / 36, 0, 1);
    level += (goal - level) * (goal > level ? 0.6 : 0.15);
    if (tag.dataset.buddy !== 'listening') return;
    const t = performance.now();
    for (let i = 0; i < bars.length; i++) {
      const wob = reduced ? 1 : 0.75 + 0.25 * Math.sin(t * 0.011 + i * 1.7);
      bars[i].style.height = (3 + level * 13 * SHAPE[i] * wob).toFixed(1) + 'px';
    }
  }

  function endMate() {
    mateOn = false;
    following = false;
    updateBuddy();
    // Waiting on a spot it pointed at: stay (unpark hides it if need be).
    if (!parked && !body.classList.contains('acting') && !glide && !drag) hideCursor();
  }

  function park() {
    parked = true;
    clearTimeout(parkTimer);
    parkTimer = setTimeout(unpark, PARK_FALLBACK_MS);
  }
  // The wait is over: rejoin the user's pointer, or go if nothing follows it.
  function unpark() {
    if (!parked) return;
    dropPark();
    if (!mateOn && !body.classList.contains('acting') && !glide && !drag) hideCursor();
    else kick();
  }
  // A new target, Esc or a hide: just forget the wait.
  function dropPark() {
    parked = false;
    clearTimeout(parkTimer);
    parkTimer = null;
  }

  // ------------------------------------------------------------- captions
  // What Claude says, revealed in step with the speech: from its first sound,
  // in proportion to the audio's real length once the speaker knows it (it
  // generates ahead of playback), a little ahead so reading never trails
  // the voice; until then at a brisk estimated pace. Beside the cursor, or
  // above the voice pill on the main display when there is no cursor.
  const CPS = 14; // Phonon measured about 13 chars/s
  const LEAD = 0.12;
  const timings = new Map(); // seq -> audio seconds (may come before the caption)
  let capSecs = null;
  let capShown = 0;
  let capSeq = null;
  let capText = '';
  let capT0 = 0;
  let capRaf = 0;
  let capTimer = null;

  function setCaption(n) {
    captionText.textContent = capText.slice(0, n);
    caption.classList.toggle('short', caption.scrollHeight <= caption.clientHeight + 1);
    caption.scrollTop = caption.scrollHeight;
    placeCaption();
  }

  function showCaption(seq, text) {
    capSeq = seq;
    capText = text;
    capT0 = performance.now();
    capSecs = timings.get(seq) || null;
    capShown = 0;
    for (const k of timings.keys()) if (k < seq) timings.delete(k);
    if (capTimer) { clearTimeout(capTimer); capTimer = null; }
    caption.getAnimations().forEach((a) => a.cancel());
    caption.classList.add('show');
    cancelAnimationFrame(capRaf);
    if (reduced) { setCaption(capText.length); return; }
    const step = () => {
      const t = (performance.now() - capT0) / 1000 + LEAD;
      let goal = capSecs ? Math.ceil(capText.length * Math.min(1, t / capSecs)) : Math.floor(t * CPS) + 1;
      // Whole words: a word appears as it starts being said, not letter by
      // letter behind the voice.
      const sp = capText.indexOf(' ', Math.max(goal - 1, 0));
      goal = sp === -1 ? capText.length : sp;
      const n = Math.min(capText.length, Math.max(capShown, goal));
      if (n !== capShown) { capShown = n; setCaption(n); }
      if (n < capText.length) capRaf = requestAnimationFrame(step);
    };
    setCaption(0);
    capRaf = requestAnimationFrame(step);
  }

  function hideCaption(ms) {
    cancelAnimationFrame(capRaf);
    if (!caption.classList.contains('show')) return;
    const out = caption.animate([{ opacity: 1 }, { opacity: 0 }], { duration: ms, easing: 'ease-in', fill: 'forwards' });
    out.onfinish = () => { caption.classList.remove('show'); out.cancel(); };
  }

  function placeCaption() {
    if (!caption.classList.contains('show')) return;
    const w = caption.offsetWidth;
    const h = caption.offsetHeight;
    let x;
    let y;
    if (pos && onMonitor(pos, 0) && body.classList.contains('cursor-on')) {
      const l = local(pos);
      const tagH = tag.classList.contains('empty') ? 0 : tag.offsetHeight + 6;
      const flipX = l.x + 20 + w > M.width - 12;
      const below = l.y + 24 + tagH + h <= M.height - 12;
      caption.classList.toggle('flip-x', flipX);
      caption.classList.toggle('flip-y', !below);
      x = flipX ? l.x - w - 8 : l.x + 20;
      y = below ? l.y + 24 + tagH : l.y - 14 - h;
    } else if (M.index === 0) {
      caption.classList.remove('flip-x', 'flip-y');
      x = M.width / 2 - w / 2;
      y = M.height - 112 - h;
    } else {
      caption.style.visibility = 'hidden';
      return;
    }
    x = clamp(x, 8, M.width - w - 8);
    y = clamp(y, 8, M.height - h - 8);
    // Never on top of the voice pill ("Speaking"): if the bubble would cover
    // it, move it just above, just below or beside it, whichever is free.
    if (pill.offsetWidth > 0) {
      const pr = pill.getBoundingClientRect();
      const hits = (cx, cy) => cx < pr.right + 8 && cx + w > pr.left - 8 && cy < pr.bottom + 8 && cy + h > pr.top - 8;
      if (hits(x, y)) {
        const fits = (cx, cy) => cx >= 8 && cy >= 8 && cx + w <= M.width - 8 && cy + h <= M.height - 8 && !hits(cx, cy);
        const tries = [[x, pr.top - 8 - h], [x, pr.bottom + 8], [pr.left - 8 - w, y], [pr.right + 8, y]];
        const ok = tries.find((t) => fits(t[0], t[1]));
        if (ok) { x = ok[0]; y = ok[1]; }
      }
    }
    caption.style.visibility = '';
    caption.style.transform = 'translate(' + x.toFixed(1) + 'px,' + y.toFixed(1) + 'px)';
  }

  // ------------------------------------------------------------- events
  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (!ev) return;
  ev.listen('pindot:cursor', (e) => {
    const p = e.payload || {};
    dropPark();
    onGlide(Object.assign({}, p, { from: px(p.from), to: px(p.to) }));
  });
  ev.listen('pindot:action', (e) => {
    const a = e.payload || {};
    onAction(a.to ? Object.assign({}, a, { to: px(a.to) }) : a);
  });

  // ------------------------------------------------------- cursor colour
  // One colour (Settings, #rrggbb) drives every accent: the lighter and
  // darker body stops are made in HSL; the glow, halo and ring tints use
  // its RGB. The default pink gives #ff7cb9 / #ff2d95 / #cf1673 within a
  // few units. Dark colours get light eyes.
  function hexToRgb(h) { return [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16)); }
  function rgbToHsl(r, g, b) {
    r /= 255; g /= 255; b /= 255;
    const mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2, d = mx - mn;
    if (!d) return [0, 0, l];
    const s = d / (1 - Math.abs(2 * l - 1));
    const h = mx === r ? ((g - b) / d + (g < b ? 6 : 0)) : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return [h * 60, s, l];
  }
  function hslToHex(h, s, l) {
    const a = s * Math.min(l, 1 - l);
    const f = (n) => {
      const k = (n + h / 30) % 12;
      return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
    };
    return '#' + [f(0), f(8), f(4)].map((v) => v.toString(16).padStart(2, '0')).join('');
  }
  function luminance(rgb) {
    const [r, g, b] = rgb.map((v) => { const c = v / 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  }
  function setCursorColor(hex) {
    if (typeof hex !== 'string' || !/^#[0-9a-fA-F]{6}$/.test(hex)) return;
    const rgb = hexToRgb(hex);
    const [h, s, l] = rgbToHsl(rgb[0], rgb[1], rgb[2]);
    const st = document.documentElement.style;
    st.setProperty('--accent', hex.toLowerCase());
    st.setProperty('--accent-lit', hslToHex(h, s, Math.min(l + 0.15, 0.9)));
    st.setProperty('--accent-deep', hslToHex(h, s * 0.81, Math.max(l - 0.15, 0.06)));
    st.setProperty('--accent-rgb', rgb.join(', '));
    st.setProperty('--accent-glow', 'rgba(' + rgb.join(', ') + ', 0.5)');
    st.setProperty('--ink', luminance(rgb) < 0.12 ? '#f5f5f7' : '#15151b');
  }

  // ------------------------------------------------------------- the eyes
  // Blinks come from a timer; the look follows the cursor's own velocity
  // (see render); the mood is one data attribute set from the voice state,
  // resting, taps and Esc. All of it is CSS transitions: no loop of its own.
  const LOOK_MAX = 7; // drawing units; the group is scaled 0.16, so ~1.1 px
  const SWEEP_MS = 520; // one left-right-and-back sweep while travelling
  const SWEEP_REACH = 2; // in LOOK_MAX units: 14 drawing units, ~2.2 px each way
  let sweepFrom = 0; // when the current stretch of travel began
  let lookKey = '';
  function look(x, y) {
    const k = (Math.round(x * LOOK_MAX * 2) / 2) + ',' + (Math.round(y * LOOK_MAX * 2) / 2);
    if (k === lookKey) return;
    lookKey = k;
    const [px_, py_] = k.split(',');
    eyesEl.style.transform = k === '0,0' ? '' : 'translate(' + px_ + 'px,' + py_ + 'px)';
  }

  let blinkTimer = null;
  function blinkOnce(then) {
    if (!body.classList.contains('cursor-on') || document.hidden) { if (then) then(); return; }
    eyeRects.forEach((r) => r.classList.add('blink'));
    setTimeout(() => {
      eyeRects.forEach((r) => r.classList.remove('blink'));
      if (then) setTimeout(then, 90);
    }, 70);
  }
  function scheduleBlink() {
    blinkTimer = setTimeout(() => {
      blinkOnce(Math.random() < 0.18 ? () => blinkOnce() : null);
      scheduleBlink();
    }, 3000 + Math.random() * 4000);
  }
  scheduleBlink();

  let eyeHold = null; // 'squint' | 'closed' for a moment
  let eyeHoldTimer = null;
  let sleepy = false;
  let sleepTimer = null;
  function renderEyes() {
    let m = eyeHold;
    if (!m && sleepy) m = 'sleepy';
    if (!m && voiceState !== 'idle' && voiceState !== 'acting') m = voiceState;
    if (m) arrow.dataset.eyes = m; else delete arrow.dataset.eyes;
  }
  function holdEyes(mode, ms) {
    eyeHold = mode;
    renderEyes();
    if (eyeHoldTimer) clearTimeout(eyeHoldTimer);
    eyeHoldTimer = setTimeout(() => { eyeHold = null; renderEyes(); }, ms);
  }
  function squint(ms) { if (eyeHold !== 'closed') holdEyes('squint', ms); }
  function eyesClosed(ms) { holdEyes('closed', ms); }
  // Resting for a good while: half-closed. Watch the class instead of
  // touching every place that sets it.
  new MutationObserver(() => {
    const r = body.classList.contains('resting');
    if (r && !sleepTimer) {
      sleepTimer = setTimeout(() => { sleepy = true; renderEyes(); }, 20000);
    } else if (!r && sleepTimer) {
      clearTimeout(sleepTimer);
      sleepTimer = null;
      if (sleepy) { sleepy = false; renderEyes(); }
    }
  }).observe(body, { attributes: true, attributeFilter: ['class'] });

  const bannerText = $('banner-text');
  // The assistant's name (Settings). The page asks for it once it listens;
  // the app sends it again when it changes.
  let assistant = 'Rusty';
  const acting = () => assistant + ' is controlling input';
  ev.listen('pindot:assistant', (e) => {
    if (typeof e.payload !== 'string' || !e.payload) return;
    assistant = e.payload;
    if (body.classList.contains('acting')) bannerText.textContent = acting();
  }).then(() => ev.emit('pindot:overlay-ready'));
  ev.listen('pindot:cursor-color', (e) => setCursorColor(e.payload));
  let stoppedTimer = null;
  let restTimer = null;
  ev.listen('pindot:acting', (e) => {
    if (e.payload) {
      body.classList.remove('stopped', 'resting');
      bannerText.textContent = acting();
      if (restTimer) clearTimeout(restTimer);
    } else {
      // Between actions the cursor stays but rests: dimmer, tag tucked away.
      if (restTimer) clearTimeout(restTimer);
      restTimer = setTimeout(() => { if (!glide && !drag && !mateOn) body.classList.add('resting'); }, 1600);
      // With the companion on: hold on the target a moment, then come back
      // (never cutting short a longer hold, such as a point's).
      holdUntil = Math.max(holdUntil, performance.now() + 900);
      setTimeout(kick, 920);
    }
    body.classList.toggle('acting', !!e.payload);
  });
  ev.listen('pindot:stopped', () => {
    dropPark();
    if (stoppedTimer) clearTimeout(stoppedTimer);
    body.classList.remove('acting');
    body.classList.add('stopped');
    bannerText.textContent = 'Stopped · ' + assistant + ' no longer controls input';
    eyesClosed(900);
    hideCursor();
    hideCaption(150);
    stoppedTimer = setTimeout(() => body.classList.remove('stopped'), 2500);
  });
  ev.listen('pindot:hide', () => { dropPark(); hideCursor(); });
  ev.listen('pindot:unpark', unpark);

  // Voice pill. A notice (e.g. "run `claude login`") stays for a few
  // seconds; pressing the hotkey again replaces it.
  const pill = $('voice');
  const pillText = $('voice-text');
  const LABELS = { listening: 'Listening', thinking: 'Thinking', acting: 'Working', speaking: 'Speaking' };
  let notice = null;
  let noticeTimer = null;
  function renderVoice() {
    pill.dataset.state = voiceState;
    pillText.textContent = notice || LABELS[voiceState] || '';
    pill.classList.toggle('notice', !!notice);
    pill.classList.toggle('show', !!notice || voiceState !== 'idle');
  }
  ev.listen('pindot:voice', (e) => {
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
    renderEyes();
    renderVoice();
    updateBuddy();
  });

  ev.listen('pindot:companion', (e) => {
    const p = e.payload || {};
    mate = px({ x: p.x, y: p.y });
    if (!mateOn) {
      mateOn = true;
      body.classList.remove('resting');
      if (!pos) showCursor({ x: mate.x + OFF.x, y: mate.y + OFF.y });
    }
    drawLevel(p.level || 0);
    if (mateTimer) clearTimeout(mateTimer);
    mateTimer = setTimeout(endMate, 1200);
    kick();
  });

  ev.listen('pindot:caption', (e) => {
    const p = e.payload || {};
    if (p.text) showCaption(p.seq, String(p.text));
  });
  ev.listen('pindot:caption-timing', (e) => {
    const p = e.payload || {};
    if (!(p.secs > 0)) return;
    timings.set(p.seq, p.secs);
    if (p.seq === capSeq) capSecs = p.secs;
  });
  ev.listen('pindot:caption-end', (e) => {
    const p = e.payload || {};
    if (p.seq !== capSeq) return;
    cancelAnimationFrame(capRaf);
    if (!p.interrupted) setCaption(capText.length);
    capTimer = setTimeout(() => hideCaption(260), p.interrupted ? 200 : 2400);
  });
  ev.emit('pindot:overlay-ready', { index: M.index });
})();
