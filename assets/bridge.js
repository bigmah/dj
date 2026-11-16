// Bridge between the webview and Rust.
//  - Captures the keyboard globally and forwards key down/up to Rust.
//  - Draws the scrolling + overview waveforms on canvases at display refresh rate.
// Rust -> JS messages arrive through dioxus.recv(), JS -> Rust through dioxus.send().

const DJ = {
  decks: [mkDeck(), mkDeck()],
  zoom: 1.0,
};
window.__dj = DJ;

function mkDeck() {
  return { id: 0, wave: null, rate: 150, st: null, recvAt: 0, ov: null, ovKey: "" };
}

const HOT_COLORS = ["#e5484d", "#f76b15", "#ffc53d", "#46a758", "#0090ff", "#8e4ec6", "#d6409f", "#12a594"];
const DECK_COLORS = ["#4ea1ff", "#ff9f43"];
const BASE_SPAN = 8; // seconds of real time visible in the scrolling waveform at zoom 1

// ---------- keyboard ----------
const isTyping = (el) =>
  el && ((el.tagName === "INPUT" && !["range", "checkbox", "button"].includes(el.type)) || el.tagName === "TEXTAREA" || el.isContentEditable);

window.addEventListener(
  "keydown",
  (e) => {
    if (e.metaKey) return; // leave Cmd shortcuts (quit, copy, ...) to the OS
    const t = e.target;
    if (isTyping(t)) {
      if (e.code === "Escape" || e.code === "Enter" || e.code === "Tab") {
        e.preventDefault();
        t.blur();
        dioxus.send({ k: "search_exit", code: e.code });
      } else if (e.code === "ArrowUp" || e.code === "ArrowDown" || e.code === "PageUp" || e.code === "PageDown") {
        e.preventDefault();
        dioxus.send({ k: "down", code: e.code, shift: e.shiftKey, alt: false, ctrl: false, rep: e.repeat });
      }
      return;
    }
    if (e.code === "Tab" || (e.ctrlKey && e.code === "KeyF")) {
      e.preventDefault();
      const s = document.getElementById("search");
      if (s) {
        s.focus();
        s.select();
      }
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    dioxus.send({ k: "down", code: e.code, shift: e.shiftKey, alt: e.altKey, ctrl: e.ctrlKey, rep: e.repeat });
  },
  true
);

window.addEventListener(
  "keyup",
  (e) => {
    if (isTyping(e.target) && !["ArrowUp", "ArrowDown"].includes(e.code)) return;
    e.preventDefault();
    dioxus.send({ k: "up", code: e.code });
  },
  true
);

window.addEventListener("blur", () => dioxus.send({ k: "blur" }));

// Mouse: click/drag on an overview to seek, wheel on the scrolling waveform to zoom.
let seeking = null;
function seekFrom(e, el) {
  const r = el.getBoundingClientRect();
  const frac = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  dioxus.send({ k: "seek", deck: +el.dataset.deck, frac });
}
document.addEventListener("mousedown", (e) => {
  const el = e.target.closest && e.target.closest("canvas.overview");
  if (!el) return;
  seeking = el;
  seekFrom(e, el);
});
document.addEventListener("mousemove", (e) => seeking && seekFrom(e, seeking));
document.addEventListener("mouseup", () => (seeking = null));
document.addEventListener(
  "wheel",
  (e) => {
    const el = e.target.closest && e.target.closest("canvas.scroller");
    if (!el) return;
    e.preventDefault();
    dioxus.send({ k: "zoom", factor: e.deltaY < 0 ? 1.1 : 1 / 1.1 });
  },
  { passive: false }
);

// ---------- drawing ----------
function b64ToBytes(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function fit(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  return canvas.getContext("2d");
}

function estPos(d) {
  const s = d.st;
  if (!s) return 0;
  let p = s.pos;
  if (s.play) {
    const dt = Math.min(0.1, (performance.now() - d.recvAt) / 1000);
    p += dt * s.sp;
    if (s.la && s.le != null && p >= s.le) p = s.ls + (p - s.le);
  }
  return Math.min(p, s.dur);
}

// Max of each band over [b0, b1)
function bandMax(wave, b0, b1, n) {
  let lo = 0, mi = 0, hi = 0;
  b0 = Math.max(0, b0);
  b1 = Math.min(n, b1);
  for (let b = b0; b < b1; b++) {
    const o = b * 3;
    if (wave[o] > lo) lo = wave[o];
    if (wave[o + 1] > mi) mi = wave[o + 1];
    if (wave[o + 2] > hi) hi = wave[o + 2];
  }
  return [lo, mi, hi];
}

const BAND_COLORS = ["#1f6feb", "#f0883e", "#f5f5f5"];
const BAND_SCALE = [1.0, 0.8, 0.55];

function drawColumns(ctx, cols, H, mid) {
  for (let band = 0; band < 3; band++) {
    ctx.fillStyle = BAND_COLORS[band];
    ctx.beginPath();
    for (const c of cols) {
      const h = (c.v[band] / 255) * mid * BAND_SCALE[band];
      if (h > 0.5) ctx.rect(c.x, mid - h, c.w, h * 2);
    }
    ctx.fill();
  }
}

function drawScroller(i) {
  const canvas = document.getElementById("wf-" + i);
  if (!canvas) return;
  const ctx = fit(canvas);
  const W = canvas.width, H = canvas.height, mid = H / 2;
  const dpr = window.devicePixelRatio || 1;
  ctx.fillStyle = "#0b0c0f";
  ctx.fillRect(0, 0, W, H);
  const d = DJ.decks[i];
  const s = d.st;
  if (!s || !s.l) {
    ctx.fillStyle = "#3a3f4b";
    ctx.font = `${12 * dpr}px -apple-system, sans-serif`;
    ctx.fillText(i === 0 ? "Deck A — empty" : "Deck B — empty", 12 * dpr, mid + 4 * dpr);
    return;
  }
  const pos = estPos(d);
  // Scale by speed so matched tempos line up visually across decks.
  const span = (BASE_SPAN / DJ.zoom) * Math.max(0.05, s.sp || 1);
  const pxPerSec = W / span;
  const t0 = pos - span / 2, t1 = pos + span / 2;
  const xOf = (t) => (t - pos) * pxPerSec + W / 2;

  // Loop region
  if (s.ls != null && s.le != null) {
    ctx.fillStyle = s.la ? "rgba(70,167,88,0.28)" : "rgba(255,255,255,0.06)";
    const a = xOf(s.ls), b = xOf(s.le);
    ctx.fillRect(a, 0, b - a, H);
  }

  // Waveform columns: locked to absolute time so they don't shimmer while scrolling.
  if (d.wave) {
    const n = d.wave.length / 3;
    const colW = Math.max(1, Math.round(dpr));
    const colSec = colW / pxPerSec;
    const firstCol = Math.floor(t0 / colSec);
    const lastCol = Math.ceil(t1 / colSec);
    const cols = [];
    for (let c = firstCol; c <= lastCol; c++) {
      const ta = c * colSec;
      if (ta < 0 || ta > s.dur) continue;
      const b0 = Math.floor(ta * d.rate);
      const b1 = Math.max(b0 + 1, Math.floor((ta + colSec) * d.rate));
      cols.push({ x: xOf(ta), w: colW, v: bandMax(d.wave, b0, b1, n) });
    }
    drawColumns(ctx, cols, H, mid);
  } else {
    ctx.fillStyle = "#555";
    ctx.font = `${12 * dpr}px -apple-system, sans-serif`;
    ctx.fillText("Analysing…", 12 * dpr, mid + 4 * dpr);
  }

  // Beat grid
  if (s.bpm > 0) {
    const beat = 60 / s.bpm;
    let k = Math.ceil((t0 - s.off) / beat);
    for (let t = s.off + k * beat; t < t1; t = s.off + ++k * beat) {
      if (t < 0) continue;
      const x = Math.round(xOf(t)) + 0.5;
      const bar = ((k % 4) + 4) % 4 === 0;
      ctx.fillStyle = bar ? "rgba(255,255,255,0.55)" : "rgba(255,255,255,0.18)";
      ctx.fillRect(x, 0, bar ? 2 * dpr : dpr, bar ? H : H * 0.12);
      if (!bar) ctx.fillRect(x, H * 0.88, dpr, H * 0.12);
    }
  }

  // Cue point + hot cues
  const marker = (t, color, label) => {
    if (t == null || t < t0 || t > t1) return;
    const x = xOf(t);
    ctx.fillStyle = color;
    ctx.fillRect(x - dpr, 0, 2 * dpr, H);
    if (label) {
      ctx.fillRect(x, 0, 14 * dpr, 14 * dpr);
      ctx.fillStyle = "#000";
      ctx.font = `bold ${10 * dpr}px -apple-system, sans-serif`;
      ctx.fillText(label, x + 4 * dpr, 11 * dpr);
    }
  };
  marker(s.cue, "#ff8c00", "");
  (s.hot || []).forEach((t, h) => marker(t, HOT_COLORS[h], String(h + 1)));

  // Playhead
  ctx.fillStyle = "#ff3b3b";
  ctx.fillRect(Math.round(W / 2) - dpr, 0, 2 * dpr, H);
}

function buildOverview(d, W, H) {
  const off = document.createElement("canvas");
  off.width = W;
  off.height = H;
  const ctx = off.getContext("2d");
  const n = d.wave.length / 3;
  const cols = [];
  for (let x = 0; x < W; x++) {
    const b0 = Math.floor((x / W) * n), b1 = Math.max(b0 + 1, Math.floor(((x + 1) / W) * n));
    cols.push({ x, w: 1, v: bandMax(d.wave, b0, b1, n) });
  }
  drawColumns(ctx, cols, H, H / 2);
  return off;
}

function drawOverview(i) {
  const canvas = document.getElementById("ov-" + i);
  if (!canvas) return;
  const ctx = fit(canvas);
  const W = canvas.width, H = canvas.height;
  const dpr = window.devicePixelRatio || 1;
  ctx.fillStyle = "#0b0c0f";
  ctx.fillRect(0, 0, W, H);
  const d = DJ.decks[i];
  const s = d.st;
  if (!s || !s.l || !d.wave || !s.dur) return;
  const key = `${d.id}:${W}x${H}`;
  if (d.ovKey !== key) {
    d.ov = buildOverview(d, W, H);
    d.ovKey = key;
  }
  ctx.drawImage(d.ov, 0, 0);
  const pos = estPos(d);
  const xOf = (t) => (t / s.dur) * W;
  ctx.fillStyle = "rgba(0,0,0,0.55)";
  ctx.fillRect(0, 0, xOf(pos), H);
  if (s.ls != null && s.le != null) {
    ctx.fillStyle = s.la ? "rgba(70,167,88,0.45)" : "rgba(255,255,255,0.12)";
    ctx.fillRect(xOf(s.ls), 0, Math.max(dpr, xOf(s.le) - xOf(s.ls)), H);
  }
  ctx.fillStyle = "#ff8c00";
  ctx.fillRect(xOf(s.cue), 0, dpr, H);
  (s.hot || []).forEach((t, h) => {
    if (t == null) return;
    ctx.fillStyle = HOT_COLORS[h];
    ctx.fillRect(xOf(t) - dpr, 0, 2 * dpr, H);
  });
  // Remaining-time warning: last 30 s flash red.
  if (s.play && s.dur - pos < 30 && Math.floor(performance.now() / 400) % 2 === 0) {
    ctx.strokeStyle = "#ff3b3b";
    ctx.lineWidth = 2 * dpr;
    ctx.strokeRect(dpr, dpr, W - 2 * dpr, H - 2 * dpr);
  }
  ctx.fillStyle = "#fff";
  ctx.fillRect(Math.round(xOf(pos)) - dpr, 0, 2 * dpr, H);
}

function frame() {
  try {
    for (let i = 0; i < DJ.decks.length; i++) {
      drawScroller(i);
      drawOverview(i);
    }
  } catch (err) {
    console.error(err);
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// ---------- messages from Rust ----------
while (true) {
  const m = await dioxus.recv();
  if (!m) continue;
  if (m.k === "state") {
    const now = performance.now();
    m.d.forEach((st, i) => {
      DJ.decks[i].st = st;
      DJ.decks[i].recvAt = now;
    });
  } else if (m.k === "wave") {
    const d = DJ.decks[m.deck];
    d.id = m.id;
    d.wave = m.data ? b64ToBytes(m.data) : null;
    d.rate = m.rate || 150;
    d.ovKey = "";
  } else if (m.k === "zoom") {
    DJ.zoom = m.z;
  } else if (m.k === "scroll") {
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const el = document.querySelector(".lib-row.sel");
        if (el) el.scrollIntoView({ block: "nearest" });
      })
    );
  } else if (m.k === "focus_search") {
    const s = document.getElementById("search");
    if (s) s.focus();
  }
}
