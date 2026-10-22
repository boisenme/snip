// Recreation of snip's overlay for the README demo. Everything drawn here mirrors
// draw_screen / draw_ann / draw_hint in src/main.rs (same colours, sizes, strokes and text),
// so if those change, update the constants below and re-render (see render.mjs).

const SNIP = {
  RED: 'rgb(235,52,52)',
  WHITE: 'rgb(255,255,255)',
  ACCENT: 'rgb(70,155,255)',
  DIM_ALPHA: 1 - 140 / 256, // unselected area is drawn at 140/256 brightness
  FONT: '"Noto Sans", sans-serif', // snip uses fc-match "sans:bold"
};

const W = 1120, H = 630; // logical size of the fake screen

// ------------------------------------------------------------------ timeline helpers

const clamp01 = (v) => Math.max(0, Math.min(1, v));
const lerp = (a, b, k) => a + (b - a) * k;
const ease = {
  linear: (k) => k,
  inOut: (k) => (k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2),
  out: (k) => 1 - Math.pow(1 - k, 3),
  outBack: (k) => { const c = 1.6; return 1 + (c + 1) * Math.pow(k - 1, 3) + c * Math.pow(k - 1, 2); },
};
/** progress of t through [t0, t1], eased */
const prog = (t, t0, t1, e = ease.inOut) => e(clamp01((t - t0) / (t1 - t0)));

/** cursor path: [[t, x, y], ...]; eased movement between waypoints, holds otherwise */
function pathAt(points, t) {
  if (t <= points[0][0]) return [points[0][1], points[0][2]];
  for (let i = 1; i < points.length; i++) {
    const [t1, x1, y1, e] = points[i];
    const [t0, x0, y0] = points[i - 1];
    if (t <= t1) {
      const k = (e || ease.inOut)(clamp01((t - t0) / (t1 - t0)));
      return [lerp(x0, x1, k), lerp(y0, y1, k)];
    }
  }
  const p = points[points.length - 1];
  return [p[1], p[2]];
}

// ------------------------------------------------------------------ geometry

const span = (a, b) => {
  const x0 = Math.floor(Math.min(a[0], b[0])), x1 = Math.ceil(Math.max(a[0], b[0]));
  const y0 = Math.floor(Math.min(a[1], b[1])), y1 = Math.ceil(Math.max(a[1], b[1]));
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
};
const contains = (r, p) => p[0] >= r.x && p[0] < r.x + r.w && p[1] >= r.y && p[1] < r.y + r.h;
/** Ui::snap_at: topmost window under p, else the whole output */
const snapAt = (windows, p) => windows.find((r) => contains(r, p)) || { x: 0, y: 0, w: W, h: H };

// ------------------------------------------------------------------ drawing (main.rs)

const font = (px) => `bold ${px}px ${SNIP.FONT}`;

function drawAnn(ctx, a) {
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  if (a.kind === 'marker') {
    const [x, y] = a.p;
    ctx.beginPath();
    ctx.arc(x, y, 15, 0, Math.PI * 2);
    ctx.fillStyle = SNIP.RED;
    ctx.fill();
    ctx.lineWidth = 2.5;
    ctx.strokeStyle = SNIP.WHITE;
    ctx.stroke();
    ctx.font = font(18);
    const s = String(a.n);
    const m = ctx.measureText(s);
    // centre the glyph's ink box on the marker, like main.rs does with the path bounds
    const ix = x - (m.actualBoundingBoxRight - m.actualBoundingBoxLeft) / 2 + m.actualBoundingBoxLeft;
    const iy = y + (m.actualBoundingBoxAscent - m.actualBoundingBoxDescent) / 2;
    ctx.fillStyle = SNIP.WHITE;
    ctx.textAlign = 'left';
    ctx.fillText(s, ix, iy);
  } else if (a.kind === 'arrow') {
    const [ax, ay] = a.a, [bx, by] = a.b;
    const dx = bx - ax, dy = by - ay;
    const len = Math.hypot(dx, dy);
    if (len < 2) return;
    const ux = dx / len, uy = dy / len;
    const head = Math.min(22, len);
    const base = [bx - ux * head, by - uy * head];
    ctx.strokeStyle = ctx.fillStyle = SNIP.RED;
    if (len > head) {
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.lineTo(base[0] + ux, base[1] + uy);
      ctx.lineWidth = 5;
      ctx.stroke();
    }
    const hw = head * 0.5;
    ctx.beginPath();
    ctx.moveTo(bx, by);
    ctx.lineTo(base[0] - uy * hw, base[1] + ux * hw);
    ctx.lineTo(base[0] + uy * hw, base[1] - ux * hw);
    ctx.closePath();
    ctx.fill();
  } else if (a.kind === 'pen' && a.pts.length) {
    ctx.beginPath();
    ctx.moveTo(a.pts[0][0], a.pts[0][1]);
    for (const p of a.pts.slice(1)) ctx.lineTo(p[0], p[1]);
    if (a.pts.length === 1) ctx.lineTo(a.pts[0][0] + 0.1, a.pts[0][1]);
    ctx.strokeStyle = SNIP.RED;
    ctx.lineWidth = 4;
    ctx.stroke();
  }
}

function drawHint(ctx, segs, cx, y) {
  const size = 15, gap = 18, pad = 16, h = 34;
  ctx.font = font(size);
  const widths = segs.map(([s]) => ctx.measureText(s).width);
  const total = widths.reduce((a, b) => a + b, 0) + gap * (segs.length - 1);
  const x0 = Math.round(cx - total / 2);
  ctx.fillStyle = 'rgba(24,24,28,0.882)'; // [24, 24, 28] at alpha 225
  ctx.fillRect(x0 - pad, y, total + 2 * pad, h);
  let x = x0;
  segs.forEach(([s, hi], i) => {
    ctx.fillStyle = hi ? SNIP.ACCENT : 'rgb(215,215,220)';
    ctx.fillText(s, x, y + h / 2 + size * 0.36);
    x += widths[i] + gap;
  });
}

/**
 * draw_screen: st = { purpose: 'shot'|'ocr', phase: 'idle'|'selecting'|'annotating',
 *                     sel, tool: null|'marker'|'arrow'|'pen', anns: [], drawing }
 */
function drawOverlay(ctx, st) {
  ctx.clearRect(0, 0, W, H);
  // dimmed frozen screen with the selection cut out at full brightness
  ctx.beginPath();
  ctx.rect(0, 0, W, H);
  if (st.sel) ctx.rect(st.sel.x, st.sel.y, st.sel.w, st.sel.h);
  ctx.fillStyle = `rgba(0,0,0,${SNIP.DIM_ALPHA})`;
  ctx.fill('evenodd');

  const r = st.sel;
  if (r) {
    ctx.lineWidth = 1.5;
    ctx.strokeStyle = SNIP.ACCENT;
    ctx.strokeRect(r.x - 0.5, r.y - 0.5, r.w + 1, r.h + 1);
    const label = `${r.w} × ${r.h}`;
    const ly = r.y > 26 ? r.y - 9 : r.y + 20;
    ctx.font = font(14);
    ctx.textAlign = 'left';
    ctx.fillStyle = 'rgba(0,0,0,0.78)';
    ctx.fillText(label, r.x + 3, ly + 1);
    ctx.fillStyle = SNIP.WHITE;
    ctx.fillText(label, r.x + 2, ly);
  }
  for (const a of st.anns.concat(st.drawing ? [st.drawing] : [])) drawAnn(ctx, a);

  const t = st.tool;
  let segs;
  if (st.purpose === 'ocr') segs = [['click window or drag region to read text', false], ['esc cancel', false]];
  else if (st.phase === 'annotating')
    segs = [['1 markers', t === 'marker'], ['2 arrow', t === 'arrow'], ['3 draw', t === 'pen'],
      ['ctrl+z undo', false], ['enter copy', false], ['ctrl+s save', false], ['esc cancel', false]];
  else
    segs = [['click window or drag region to copy', !t], ['or press 1 / 2 / 3 first to annotate:', !!t],
      ['1 markers', t === 'marker'], ['2 arrow', t === 'arrow'], ['3 draw', t === 'pen'], ['esc cancel', false]];
  drawHint(ctx, segs, W / 2, H - 70);
}

// ------------------------------------------------------------------ demo chrome (not part of snip)

/** keys: ['Meta', 'Shift', 'X'] shown as keycaps at the top of the frame; k = 0..1 visibility */
function setKeys(el, keys, k, label) {
  if (el.dataset.keys !== keys.join('+') + label) {
    el.dataset.keys = keys.join('+') + label;
    el.innerHTML = (label ? `<span class="klabel">${label}</span>` : '') +
      keys.map((s) => `<kbd>${s}</kbd>`).join('<span class="kplus">+</span>');
  }
  el.style.opacity = k;
  el.style.transform = `translateY(${(1 - k) * -10}px) scale(${0.96 + 0.04 * k})`;
  el.style.transformOrigin = "right top";
}

function setCursor(el, p, shape, down) {
  el.style.transform = `translate(${p[0]}px, ${p[1]}px)`;
  el.dataset.shape = shape;
  el.classList.toggle('down', !!down);
}

// ------------------------------------------------------------------ player

/** Drives scene.frame(t). Opened normally it loops in real time; render.mjs calls renderAt(t). */
async function startPlayer(scene) {
  const canvas = document.getElementById('overlay');
  const dpr = Number(new URLSearchParams(location.search).get('dpr')) || window.devicePixelRatio || 1;
  canvas.width = W * dpr;
  canvas.height = H * dpr;
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  await Promise.all([font(15), font(14), font(18), '400 13px "Noto Sans"', '400 13px "Noto Sans Mono"']
    .map((f) => document.fonts.load(f)));
  await document.fonts.ready;
  window.renderAt = (t) => scene.frame(t, ctx);
  window.sceneDuration = scene.duration;
  if (new URLSearchParams(location.search).has('capture')) {
    window.sceneReady = true;
    return;
  }
  const t0 = performance.now();
  const loop = (now) => {
    scene.frame(((now - t0) / 1000) % scene.duration, ctx);
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
}
