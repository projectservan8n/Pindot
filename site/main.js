/* Pindot site. No libraries, no trackers. */
(() => {
  "use strict";
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const root = document.documentElement;

  /* ---------- Rusty marks ---------- */
  const BODY = "M46.12 71.59 A20 20 0 0 1 71.59 46.12 L197.59 90.12 A20 20 0 0 1 200.99 126.32 L153.64 153.64 L126.32 200.99 A20 20 0 0 1 90.12 197.59 Z";
  const NS = "http://www.w3.org/2000/svg";
  $$("svg[data-rusty]").forEach((svg) => {
    if (svg.childElementCount) return;
    const body = document.createElementNS(NS, "path");
    body.setAttribute("class", "body");
    body.setAttribute("d", BODY);
    const eyes = document.createElementNS(NS, "g");
    eyes.setAttribute("class", "eyes");
    [97, 127].forEach((x) => {
      const r = document.createElementNS(NS, "rect");
      r.setAttribute("x", x); r.setAttribute("y", 105);
      r.setAttribute("width", 16); r.setAttribute("height", 28); r.setAttribute("rx", 8);
      eyes.appendChild(r);
    });
    svg.append(body, eyes);
  });
  const marks = $$("svg[data-rusty]");

  // eyes follow the pointer
  let px = -1, py = -1, eyeFrame = 0;
  const lookAll = () => {
    eyeFrame = 0;
    for (const m of marks) {
      const r = m.getBoundingClientRect();
      if (r.bottom < 0 || r.top > innerHeight || r.width === 0) continue;
      const cx = r.left + r.width * 0.47, cy = r.top + r.height * 0.47;
      const dx = px - cx, dy = py - cy, d = Math.hypot(dx, dy) || 1;
      const k = Math.min(1, d / 260) * 9;
      m.querySelector(".eyes").setAttribute("transform", `translate(${((dx / d) * k).toFixed(1)} ${((dy / d) * k).toFixed(1)})`);
    }
  };
  addEventListener("pointermove", (e) => {
    if (e.pointerType === "touch") return;
    px = e.clientX; py = e.clientY;
    if (!eyeFrame) eyeFrame = requestAnimationFrame(lookAll);
  }, { passive: true });

  // blink now and then
  if (!reduce) {
    const blink = () => {
      const m = marks[Math.floor(Math.random() * marks.length)];
      const group = [m, $("#actor .mark"), $(".hero .perch")];
      group.forEach((g) => { g.classList.remove("blink"); void g.getBBox(); g.classList.add("blink"); });
      setTimeout(() => group.forEach((g) => g.classList.remove("blink")), 220);
      setTimeout(blink, 2200 + Math.random() * 3800);
    };
    setTimeout(blink, 2400);
  }

  /* ---------- header and reveal ---------- */
  const top = $("#top");
  const onScroll = () => top.classList.toggle("scrolled", scrollY > 12);
  addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  const rvs = $$(".rv");
  if (reduce || !("IntersectionObserver" in window)) {
    rvs.forEach((el) => el.classList.add("in"));
  } else {
    const io = new IntersectionObserver((ents) => {
      ents.forEach((e) => { if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); } });
    }, { rootMargin: "0px 0px -8% 0px" });
    rvs.forEach((el) => io.observe(el));
    requestAnimationFrame(() => $$(".hero .rv").forEach((el) => el.classList.add("in")));
  }

  /* ---------- dot field (pindot: a pressed dot) ---------- */
  const cvs = $("#dots");
  const ctx = cvs.getContext("2d");
  const hero = $(".hero");
  let W = 0, H = 0, dpr = 1, dots = [], heroOn = true, t0 = performance.now();
  const presses = []; // {x, y, t}
  const rustyRGB = () => {
    const c = getComputedStyle(root).getPropertyValue("--rusty").trim();
    const n = parseInt(c.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  };
  let RGB = rustyRGB();
  const sizeDots = () => {
    dpr = Math.min(2, devicePixelRatio || 1);
    W = cvs.clientWidth; H = cvs.clientHeight;
    cvs.width = W * dpr; cvs.height = H * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const gap = W < 640 ? 24 : 30;
    dots = [];
    for (let y = gap / 2; y < H; y += gap) for (let x = gap / 2; x < W; x += gap) dots.push(x, y);
  };
  let hx = -999, hy = -999;
  hero.addEventListener("pointermove", (e) => {
    const r = cvs.getBoundingClientRect();
    hx = e.clientX - r.left; hy = e.clientY - r.top;
  }, { passive: true });
  hero.addEventListener("pointerleave", () => { hx = hy = -999; });
  hero.addEventListener("pointerdown", (e) => {
    if (e.target.closest(".stage, button, a")) return;
    const r = cvs.getBoundingClientRect();
    presses.push({ x: e.clientX - r.left, y: e.clientY - r.top, t: performance.now() });
  });
  const drawDots = (now) => {
    ctx.clearRect(0, 0, W, H);
    const t = (now - t0) / 1000;
    const [R, G, B] = RGB;
    while (presses.length && now - presses[0].t > 1800) presses.shift();
    for (let i = 0; i < dots.length; i += 2) {
      const x = dots[i], y = dots[i + 1];
      // slow ambient swell, fading toward the bottom
      const fade = Math.max(0, 1 - y / (H * 0.95));
      let a = 0.07 + 0.05 * Math.sin(x * 0.012 + y * 0.009 + t * 0.9) * fade;
      let s = 1.15, heat = 0;
      const d = Math.hypot(x - hx, y - hy);
      if (d < 170) heat = 1 - d / 170;
      for (const p of presses) {
        const ring = ((now - p.t) / 1000) * 520;
        const dd = Math.abs(Math.hypot(x - p.x, y - p.y) - ring);
        if (dd < 40) heat = Math.max(heat, (1 - dd / 40) * (1 - (now - p.t) / 1800));
      }
      if (heat > 0) {
        s += heat * 2.3;
        ctx.fillStyle = `rgba(${R},${G},${B},${(0.15 + heat * 0.75) * Math.max(fade, 0.3)})`;
      } else {
        ctx.fillStyle = `rgba(245,245,247,${a * fade})`;
      }
      ctx.beginPath(); ctx.arc(x, y, s, 0, 6.2832); ctx.fill();
    }
  };
  const loopDots = (now) => {
    if (heroOn) drawDots(now);
    if (!reduce) requestAnimationFrame(loopDots);
  };
  sizeDots();
  if (reduce) drawDots(performance.now()); else requestAnimationFrame(loopDots);
  new ResizeObserver(() => { sizeDots(); if (reduce) drawDots(performance.now()); }).observe(cvs);
  if ("IntersectionObserver" in window) new IntersectionObserver(([e]) => { heroOn = e.isIntersecting; }).observe(hero);

  /* ---------- the stage ---------- */
  const stage = $("#stage");
  const actor = $("#actor"), bubble = $("#bubble"), you = $("#you");
  const pill = $("#pill"), pillState = $("#pill-state"), pillHeard = $("#pill-heard");
  const banner = $("#banner"), approve = $("#approve");
  const winNotes = $("#win-notes"), winSet = $("#win-settings"), note = $("#note");
  const icoNotes = $("#ico-notes"), icoSet = $("#ico-settings");
  const talk = $("#talk"), talkHelp = $("#talk-help");
  const tasks = $$(".task");

  const PHRASES = {
    notes: "Write a shopping list in Notes",
    dark: "Turn on dark mode",
    teach: "Show me how to change my wallpaper",
  };
  const ORDER = ["notes", "dark", "teach"];
  let task = "notes";
  let manual = false;        // someone has used it: approvals and steps wait for real clicks
  const allowed = new Set(); // "Allow for this session"

  class Cancel extends Error {}
  let run = null;
  const newRun = () => {
    if (run) run.cancel();
    run = {
      dead: false, hooks: new Set(),
      cancel() { this.dead = true; this.hooks.forEach((f) => f()); this.hooks.clear(); },
    };
    return run;
  };
  const guard = (r) => { if (r.dead) throw new Cancel(); };
  const wait = (r, ms) => new Promise((res, rej) => {
    if (r.dead) return rej(new Cancel());
    const h = () => { clearTimeout(id); rej(new Cancel()); };
    const id = setTimeout(() => { r.hooks.delete(h); res(); }, reduce ? Math.min(ms, 300) : ms);
    r.hooks.add(h);
  });

  // positions are kept as fractions of the stage, so a resize keeps Rusty in place
  const rel = (el, fx = 0.5, fy = 0.5) => {
    const s = stage.getBoundingClientRect(), r = el.getBoundingClientRect();
    return [(r.left - s.left + r.width * fx) / s.width, (r.top - s.top + r.height * fy) / s.height];
  };
  const toPx = ([fx, fy]) => [fx * stage.clientWidth, fy * stage.clientHeight];

  const sprite = (el, tipX, tipY) => {
    let at = [0.7, 0.66], cur = null;
    const place = (p) => {
      const [x, y] = toPx(p);
      return `translate(${(x - el.offsetWidth * tipX).toFixed(1)}px, ${(y - el.offsetHeight * tipY).toFixed(1)}px)`;
    };
    return {
      get at() { return at; },
      set(p) { at = p; el.style.transform = place(p); },
      async go(r, p, speed = 1) {
        guard(r);
        const [x0, y0] = toPx(at), [x1, y1] = toPx(p);
        const dist = Math.hypot(x1 - x0, y1 - y0);
        // if he's mid-move, start from where he is right now
        const moving = cur && cur.playState === "running";
        const from = moving ? getComputedStyle(el).transform : place(at), to = place(p);
        if (moving) cur.cancel();
        at = p;
        if (dist < 2 || reduce) { el.style.transform = to; return; }
        // a slight arc, like a hand moving a mouse
        const mx = (x0 + x1) / 2 - (y1 - y0) * 0.12, my = (y0 + y1) / 2 + (x1 - x0) * 0.12;
        const mid = `translate(${(mx - el.offsetWidth * tipX).toFixed(1)}px, ${(my - el.offsetHeight * tipY).toFixed(1)}px)`;
        const dur = Math.min(1250, 380 + dist * 0.85) / speed;
        const anim = el.animate([{ transform: from }, { transform: mid, offset: 0.5 }, { transform: to }],
          { duration: dur, easing: "cubic-bezier(.45,.05,.2,1)" });
        cur = anim;
        const h = () => anim.cancel();
        r.hooks.add(h);
        el.style.transform = to;
        try { await anim.finished; } catch { /* cancelled */ }
        r.hooks.delete(h);
        guard(r);
      },
    };
  };
  const rusty = sprite(actor, 52 / 256, 52 / 256);
  const ghost = sprite(you, 0.06, 0.04);

  const ripple = (p, color) => {
    const [x, y] = toPx(p);
    const d = document.createElement("span");
    d.className = "ripple";
    d.style.left = x + "px"; d.style.top = y + "px";
    if (color) d.style.borderColor = color;
    stage.appendChild(d);
    setTimeout(() => d.remove(), 700);
  };
  const press = async (r) => {
    actor.classList.add("press");
    ripple(rusty.at);
    await wait(r, 140);
    actor.classList.remove("press");
  };
  const say = (text) => {
    actor.classList.toggle("flip", rusty.at[0] > 0.58);
    if (!text) { bubble.classList.remove("show"); return; }
    bubble.textContent = text;
    bubble.classList.add("show");
  };
  const setPill = (state, heard = "") => {
    if (!state) { pill.classList.remove("show"); return; }
    pill.dataset.state = state;
    pillState.textContent = { listening: "Listening", thinking: "Thinking", working: "Working", speaking: "Speaking" }[state];
    pillHeard.textContent = heard;
    pill.classList.add("show");
  };
  const setBanner = (on) => banner.classList.toggle("show", on);

  const openWin = (win, ico) => {
    for (const [w, i] of [[winNotes, icoNotes], [winSet, icoSet]]) {
      if (w !== win) { w.classList.remove("show"); i.classList.remove("open"); }
    }
    ico.classList.remove("bounce"); void ico.offsetWidth; ico.classList.add("bounce", "open");
    win.classList.add("show");
  };
  const closeWins = () => {
    [winNotes, winSet].forEach((w) => w.classList.remove("show"));
    [icoNotes, icoSet].forEach((i) => i.classList.remove("open", "bounce"));
  };
  const setPane = (name) => {
    $$(".side-item[data-pane]", winSet).forEach((b) => b.classList.toggle("on", b.dataset.pane === name));
    $$(".set-pane", winSet).forEach((p) => p.classList.toggle("on", p.dataset.pane === name));
  };
  const setWall = (w) => {
    stage.dataset.wall = w;
    $$(".thumb", winSet).forEach((t) => t.classList.toggle("on", t.dataset.wall === w));
  };

  // the ghost pointer stands in for a person when nobody is using the demo
  const ghostClick = async (r, el) => {
    if (!you.classList.contains("show")) {
      ghost.set([0.92, 0.95]);
      you.classList.add("show");
      await wait(r, 200);
    }
    await ghost.go(r, rel(el, 0.5, 0.55), 0.8);
    await wait(r, 260);
    ripple(ghost.at, "#ffffff");
    await wait(r, 160);
  };
  const ghostAway = () => you.classList.remove("show");

  // wait for a real click on one of the targets (or the ghost's, in autoplay)
  const awaitClick = (r, targets, auto) => new Promise((res, rej) => {
    if (r.dead) return rej(new Cancel());
    const offs = [];
    const done = (v) => { offs.forEach((f) => f()); r.hooks.delete(h); res(v); };
    const h = () => { offs.forEach((f) => f()); rej(new Cancel()); };
    r.hooks.add(h);
    targets.forEach((t) => {
      const f = (e) => { e.preventDefault(); done(t); };
      t.addEventListener("click", f);
      offs.push(() => t.removeEventListener("click", f));
    });
    if (auto) ghostClick(r, auto).then(() => done(auto), () => {});
  });

  const askApproval = async (r, app, why, risky) => {
    if (allowed.has(app)) return true;
    $("#ap-app").textContent = app;
    $("#ap-why").textContent = why;
    approve.classList.toggle("risky", !!risky);
    approve.classList.remove("nudge");
    approve.classList.add("show");
    setPill("working", `Waiting for you to allow ${app}`);
    const allow = $("#ap-allow"), deny = $("#ap-deny");
    let nudge;
    if (manual) {
      allow.focus({ preventScroll: true });
      nudge = setTimeout(() => approve.classList.add("nudge"), 2600);
    }
    try {
      const hit = await awaitClick(r, [allow, deny], manual ? null : allow);
      if (hit === allow && manual) allowed.add(app); // the stand-in's yes doesn't count for you
      return hit === allow;
    } finally {
      clearTimeout(nudge);
      approve.classList.remove("show", "nudge");
      ghostAway();
    }
  };

  const typeInto = async (r, el, text) => {
    el.classList.remove("blank");
    const caret = document.createElement("span");
    caret.className = "caret";
    el.textContent = "";
    el.appendChild(caret);
    for (const ch of text) {
      caret.before(ch);
      await wait(r, ch === "\n" ? 260 : 32 + Math.random() * 46);
    }
    return caret;
  };

  const resetDesk = () => {
    closeWins();
    approve.classList.remove("show", "nudge", "risky");
    stage.classList.remove("teach-click");
    actor.classList.remove("point", "press");
    say("");
    ghostAway();
    setBanner(false);
    $$(".app-ico, .side-item, .thumb", stage).forEach((b) => b.setAttribute("tabindex", "-1"));
  };

  /* the three jobs */
  const JOBS = {
    async notes(r) {
      say("I'll put that in Notes.");
      if (!(await askApproval(r, "Notes", "to write your shopping list.", false))) {
        setPill("speaking"); say("OK, I won't touch Notes."); return;
      }
      setPill("working"); setBanner(true); say("Opening Notes");
      await rusty.go(r, rel(icoNotes));
      await press(r);
      note.textContent = ""; note.classList.add("blank");
      openWin(winNotes, icoNotes);
      await wait(r, 420);
      say("Writing your list");
      await rusty.go(r, rel(note, 0.22, 0.12));
      await press(r);
      say("");
      const caret = await typeInto(r, note, "Shopping list\n\n- eggs\n- calamansi\n- coffee beans\n- dish soap");
      await wait(r, 300);
      caret.remove();
      setBanner(false);
      await rusty.go(r, rel(note, 0.78, 0.9));
      setPill("speaking"); say("Done. Four things on your list.");
    },
    async dark(r) {
      const goingDark = !stage.classList.contains("dark");
      say("That's in Settings.");
      if (!(await askApproval(r, "Settings", goingDark ? "to switch on dark mode." : "to switch dark mode off.", true))) {
        setPill("speaking"); say("OK, I'll leave Settings alone."); return;
      }
      setPill("working"); setBanner(true); say("Opening Settings");
      await rusty.go(r, rel(icoSet));
      await press(r);
      setPane("look");
      openWin(winSet, icoSet);
      await wait(r, 480);
      const target = goingDark ? $("#seg-dark") : $("#seg-light");
      say(goingDark ? "Clicking Dark" : "Clicking Light");
      await rusty.go(r, rel(target));
      await press(r);
      stage.classList.toggle("dark", goingDark);
      $$(".seg span", winSet).forEach((s) => s.classList.toggle("on", s === target));
      setBanner(false);
      await wait(r, 500);
      setPill("speaking"); say(goingDark ? "Dark mode is on. Easier on the eyes." : "Back to light mode.");
    },
    async teach(r) {
      // he points, you click. No control of your computer, so no banner.
      setPill("speaking");
      stage.classList.add("teach-click");
      actor.classList.add("point");
      const step = async (el, text, fx = 0.5, fy = 0.5, others = []) => {
        const targets = [el, ...others];
        targets.forEach((t) => t.setAttribute("tabindex", "0"));
        // listen from the start, so a quick click while he's still moving counts
        const hitP = awaitClick(r, targets, null);
        let settled = false;
        hitP.then(() => { settled = true; }, () => { settled = true; });
        rusty.go(r, rel(el, fx, fy)).then(() => {
          if (settled || r.dead) return;
          say(text);
          if (!manual) ghostClick(r, el).then(() => { if (!settled && !r.dead) el.click(); }, () => {});
        }, () => {});
        const hit = await hitP;
        targets.forEach((t) => t.setAttribute("tabindex", "-1"));
        return hit;
      };
      await step(icoSet, "Step 1 of 3: open Settings, down here.");
      setPane("look");
      openWin(winSet, icoSet);
      await wait(r, 520);
      await step($("#side-wall"), "Step 2 of 3: click Wallpaper.", 0.5, 0.5);
      setPane("wall");
      await wait(r, 380);
      const thumbs = $$(".thumb", winSet);
      const next = thumbs.find((t) => !t.classList.contains("on")) || thumbs[0];
      const picked = await step(next, "Step 3 of 3: pick one you like.", 0.5, 0.5, thumbs.filter((t) => t !== next));
      setWall(picked.dataset.wall);
      ghostAway();
      actor.classList.remove("point");
      stage.classList.remove("teach-click");
      await wait(r, 500);
      say("That's it. You did it yourself.");
    },
  };

  // hearing: the words appear while the button is held
  let heardTimer = 0;
  const listen = () => {
    clearInterval(heardTimer);
    const words = PHRASES[task].split(" ");
    let n = 0;
    setPill("listening", "");
    heardTimer = setInterval(() => {
      if (n < words.length) pillHeard.textContent = words.slice(0, ++n).join(" ");
      else clearInterval(heardTimer);
    }, reduce ? 40 : 190);
  };

  const perform = async (r, name) => {
    clearInterval(heardTimer);
    resetDesk();
    pillHeard.textContent = PHRASES[name];
    setPill("thinking", PHRASES[name]);
    await wait(r, 900);
    await JOBS[name](r);
    await wait(r, 2600);
    say("");
    setPill(null);
    r.cancel(); // finished: nothing left to stop
  };

  const selectTask = (name) => {
    task = name;
    tasks.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.task === name)));
  };

  const stop = (msg = true) => {
    if (!run || run.dead) return;
    run.cancel();
    clearInterval(heardTimer);
    resetDesk();
    talk.classList.remove("down");
    if (msg) {
      setPill("speaking");
      pillHeard.textContent = "";
      pillState.textContent = "Stopped";
      say("Stopped.");
      setTimeout(() => { if (!run || run.dead) { say(""); setPill(null); } }, 1600);
    } else {
      setPill(null);
    }
  };

  /* autoplay, until someone takes over */
  let inView = false, autoTimer = 0, autoIdx = 0;
  const autoplay = async () => {
    if (manual || !inView) return;
    const r = newRun();
    const name = ORDER[autoIdx % ORDER.length];
    selectTask(name);
    try {
      await wait(r, 700);
      talk.classList.add("down");
      listen();
      await wait(r, PHRASES[name].split(" ").length * 190 + 650);
      talk.classList.remove("down");
      await perform(r, name);
      autoIdx++;
      clearTimeout(autoTimer);
      autoTimer = setTimeout(autoplay, 1400);
    } catch (e) {
      if (!(e instanceof Cancel)) throw e;
    }
  };

  const takeOver = () => {
    manual = true;
    clearTimeout(autoTimer);
    if (run && !run.dead) { run.cancel(); resetDesk(); setPill(null); }
    talk.classList.remove("down");
  };

  /* hold to talk */
  let downAt = 0, holding = false;
  const down = () => {
    if (holding) return;
    takeOver();
    holding = true; downAt = performance.now();
    talk.classList.add("down");
    talkHelp.textContent = "Listening... let go when you're done";
    newRun();
    listen();
  };
  const up = async () => {
    if (!holding) return;
    holding = false;
    talk.classList.remove("down");
    const held = performance.now() - downAt;
    const r = run;
    if (held < 350) {
      clearInterval(heardTimer);
      setPill(null);
      talkHelp.textContent = "Hold it down while you talk";
      talk.animate([{ transform: "translateX(0)" }, { transform: "translateX(-5px)" }, { transform: "translateX(5px)" }, { transform: "translateX(0)" }], { duration: 260 });
      return;
    }
    talkHelp.textContent = "Press and hold, then let go";
    try { await perform(r, task); } catch (e) { if (!(e instanceof Cancel)) throw e; }
  };
  talk.addEventListener("pointerdown", (e) => { if (e.button === 0) { e.preventDefault(); talk.setPointerCapture?.(e.pointerId); down(); } });
  talk.addEventListener("pointerup", up);
  talk.addEventListener("pointercancel", up);
  talk.addEventListener("contextmenu", (e) => e.preventDefault());
  talk.addEventListener("keydown", (e) => {
    if ((e.key === " " || e.key === "Enter") && !e.repeat) { e.preventDefault(); down(); }
  });
  talk.addEventListener("keyup", (e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); up(); } });
  talk.addEventListener("blur", () => { if (holding) up(); });

  // picking a job runs it, as if you'd said it
  tasks.forEach((b) => b.addEventListener("click", async () => {
    takeOver();
    selectTask(b.dataset.task);
    const r = newRun();
    try {
      talk.classList.add("down");
      listen();
      await wait(r, PHRASES[task].split(" ").length * 190 + 450);
      talk.classList.remove("down");
      await perform(r, task);
    } catch (e) {
      talk.classList.remove("down");
      if (!(e instanceof Cancel)) throw e;
    }
  }));

  addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !run || run.dead) return;
    manual = true;
    clearTimeout(autoTimer);
    stop(true);
  });

  // the real Pindot hides from screenshots; this one just starts when you can see it
  const parkActor = () => rusty.set(rusty.at);
  new ResizeObserver(() => { parkActor(); if (you.classList.contains("show")) ghost.set(ghost.at); }).observe(stage);
  rusty.set([0.7, 0.66]);
  if ("IntersectionObserver" in window) {
    new IntersectionObserver(([e]) => {
      inView = e.isIntersecting;
      if (inView && !manual && (!run || run.dead)) { clearTimeout(autoTimer); autoTimer = setTimeout(autoplay, 900); }
      if (!inView && run && !run.dead && !holding) { clearTimeout(autoTimer); run.cancel(); resetDesk(); setPill(null); talk.classList.remove("down"); }
    }, { threshold: 0.45 }).observe(stage);
  }

  /* ---------- chapters ---------- */
  const shots = $$(".frame img");
  const chs = $$(".ch");
  if ("IntersectionObserver" in window) {
    const cio = new IntersectionObserver((ents) => {
      ents.forEach((e) => {
        if (!e.isIntersecting) return;
        const i = +e.target.dataset.shot;
        chs.forEach((c) => c.classList.toggle("on", c === e.target));
        shots.forEach((s, k) => s.classList.toggle("on", k === i));
      });
    }, { rootMargin: "-48% 0px -48% 0px" });
    chs.forEach((c) => cio.observe(c));
  }
  chs[0]?.classList.add("on");

  /* ---------- tabs (brains, install) ---------- */
  const tablist = (list) => {
    const tabs = $$('[role="tab"]', list);
    const pick = (t, focus) => {
      tabs.forEach((x) => {
        const on = x === t;
        x.setAttribute("aria-selected", String(on));
        x.tabIndex = on ? 0 : -1;
        document.getElementById(x.getAttribute("aria-controls")).hidden = !on;
      });
      if (focus) t.focus();
    };
    tabs.forEach((t, i) => {
      t.addEventListener("click", () => pick(t));
      t.addEventListener("keydown", (e) => {
        const k = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
        if (e.key === "Home") { e.preventDefault(); pick(tabs[0], true); }
        else if (e.key === "End") { e.preventDefault(); pick(tabs[tabs.length - 1], true); }
        else if (k) { e.preventDefault(); pick(tabs[(i + k + tabs.length) % tabs.length], true); }
      });
    });
  };
  $$('[role="tablist"]').forEach(tablist);
  // show Windows steps first to Windows visitors
  if (/Windows/i.test(navigator.userAgent)) $("#os-win")?.click();

  /* ---------- copy buttons ---------- */
  $$(".copy").forEach((b) => b.addEventListener("click", async () => {
    const text = b.dataset.copy;
    try {
      await navigator.clipboard.writeText(text);
      b.textContent = "Copied";
    } catch {
      const code = b.parentElement.querySelector("code");
      const sel = getSelection(), range = document.createRange();
      range.selectNodeContents(code); sel.removeAllRanges(); sel.addRange(range);
      b.textContent = "Selected";
    }
    setTimeout(() => { b.textContent = "Copy"; }, 1600);
  }));

  /* code that scrolls sideways gets a tab stop so keyboard users can scroll it */
  const tabScrollers = () => $$(".term code").forEach((c) => {
    if (c.scrollWidth > c.clientWidth + 1) c.tabIndex = 0;
    else c.removeAttribute("tabindex");
  });
  tabScrollers();
  addEventListener("resize", tabScrollers, { passive: true });
  $$("[role=tab]").forEach((t) => t.addEventListener("click", () => setTimeout(tabScrollers, 0)));

  /* ---------- Rusty's colour ---------- */
  const lum = (hex) => {
    const n = parseInt(hex.slice(1), 16);
    const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => {
      v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
  };
  const contrast = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  const paint = (c) => {
    const L = lum(c), dark = lum("#15151b"), light = lum("#f5f5f7");
    const ink = contrast(L, dark) >= contrast(L, light) ? "#15151b" : "#f5f5f7";
    root.style.setProperty("--rusty", c);
    root.style.setProperty("--rusty-ink", ink);
    // eyes follow the app's own rule (format.ts eyeInk): light eyes on dark colours
    root.style.setProperty("--rusty-eye", L < 0.12 ? "#f5f5f7" : "#15151b");
    RGB = rustyRGB();
    $$(".sw").forEach((s) => s.setAttribute("aria-pressed", String(s.dataset.c === c)));
  };
  $$(".sw").forEach((s) => s.addEventListener("click", () => paint(s.dataset.c)));

  /* ---------- demo video ---------- */
  const player = $("#player"), video = $("#video"), play = $("#play");
  play.addEventListener("click", () => {
    video.controls = true;
    player.classList.add("playing");
    video.play().catch(() => {});
    video.focus({ preventScroll: true });
  });
  video.addEventListener("play", () => { video.controls = true; player.classList.add("playing"); });
  video.addEventListener("ended", () => player.classList.remove("playing"));
})();
