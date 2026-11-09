<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { BANDS, GROUPS, bandAt, describe, type BandGroup } from '../lib/bandplan';
  import { formatMHz } from '../lib/format';
  import type { FreqRange } from '../lib/api';
  import { colourOf, type Bookmark } from '../lib/bookmarks.svelte';
  import { logScale, zoomLabel } from '../lib/zoom';

  let {
    lo,
    hi,
    covered,
    win,
    pending = null,
    view,
    listenHz = null,
    bookmarks = [],
    enabled,
    canTune = false,
    ongo,
  }: {
    /** Ends of the map (Hz). */
    lo: number;
    hi: number;
    /** What the receiver can tune; empty = unknown, nothing greyed out. */
    covered: FreqRange[];
    /** Sampled window: centre and width. */
    win: { c: number; span: number };
    /** Centre the radio is being moved to. */
    pending?: number | null;
    /** What the spectrum shows. */
    view: { lo: number; hi: number };
    listenHz?: number | null;
    bookmarks?: Bookmark[];
    enabled: ReadonlySet<BandGroup>;
    canTune?: boolean;
    /** Clicked, dragged or stepped to a frequency. */
    ongo: (hz: number) => void;
  } = $props();

  const H = 34;
  const BAND_Y = 3;
  const BAND_H = 12;
  const LABEL_Y = 27;
  const GROUP_COLOUR = Object.fromEntries(GROUPS.map((g) => [g.id, g.colour])) as Record<BandGroup, string>;
  const TICKS_MHZ = [0.1, 0.2, 0.5, 1, 2, 3, 5, 10, 20, 30, 50, 100, 200, 300, 400, 500, 700, 1000, 1500, 2000];
  /** Zoomed right in, the map still shows at least this much. */
  const MIN_SPAN_HZ = 1e6;
  const ZOOM_KEY = 'sdr.minimap.zoom';

  let wrap: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(600);
  let dpr = $state(1);
  let hoverX = $state<number | null>(null);
  let preview = $state<number | null>(null);
  let drag: { id: number; x0: number; moved: boolean; pan: boolean; c0: number } | null = null;
  let keyTimer: ReturnType<typeof setTimeout> | null = null;
  const touches = new Map<number, number>();
  let pinch: { d0: number; h0: number } | null = null;

  // Zoom: the map shows ln-frequencies c ± h out of the whole range. mz is
  // how many times narrower than the whole range that is (1 = all of it);
  // mc is the centre, null until there's a radio to centre on.
  function loadZoom(): number {
    try {
      const z = Number(localStorage.getItem(ZOOM_KEY));
      return Number.isFinite(z) && z >= 1 ? z : 1;
    } catch {
      return 1;
    }
  }
  let mz = $state(loadZoom());
  let mc = $state<number | null>(null);
  $effect(() => {
    const z = mz;
    try {
      localStorage.setItem(ZOOM_KEY, String(z));
    } catch {
      /* private window: just don't remember it */
    }
  });

  const L0 = $derived(Math.log(lo));
  const L1 = $derived(Math.log(hi));
  const fullH = $derived((L1 - L0) / 2);
  /** Narrowest half-width about a centre, so the map never shows less than
   *  MIN_SPAN_HZ or the sampled band (span = 2·e^c·sinh h). */
  const minH = (c: number) =>
    Math.min(fullH, Math.asinh(Math.max(MIN_SPAN_HZ, win.span * 1.5) / (2 * Math.exp(c))));
  const shown = $derived.by(() => {
    let c = mc ?? Math.log(Math.min(hi, Math.max(lo, win.c)));
    const h = Math.min(fullH, Math.max(fullH / mz, minH(c)));
    c = Math.min(L1 - h, Math.max(L0 + h, c));
    return { c, h, lo: Math.exp(c - h), hi: Math.exp(c + h) };
  });
  const level = $derived(fullH / shown.h);
  const zoomed = $derived(level > 1.001);
  const atMax = $derived(shown.h <= minH(shown.c) * 1.01);
  const levelText = $derived(level >= 10 ? `×${Math.round(level)}` : zoomLabel(level));

  const scale = $derived(logScale(shown.lo, shown.hi, width));

  // Keep the radio on the map: when it moves off what's shown, bring it to
  // the middle (but don't follow every small move, or a click would slide
  // the map under the cursor).
  let lastC: number | null = null;
  $effect(() => {
    const c = win.c;
    if (c === lastC) return;
    lastC = c;
    untrack(() => {
      if (mc === null || c < shown.lo || c > shown.hi) mc = Math.log(c);
    });
  });

  /** Zoom by f about a point on the map (default: the radio if it's shown, else the middle). */
  function zoomBy(f: number, ax?: number) {
    const r = shown;
    const x0 = ax ?? (win.c >= r.lo && win.c <= r.hi ? scale.x(win.c) : width / 2);
    const frac = Math.min(1, Math.max(0, x0 / Math.max(1, width)));
    const a = r.c - r.h + frac * 2 * r.h;
    let h = Math.min(fullH, Math.max(minH(a), r.h / f));
    if (fullH / h < 1.05) h = fullH;
    mz = fullH / h;
    mc = a + h - 2 * frac * h;
  }

  function fit() {
    mz = 1;
  }

  function panBy(px: number) {
    const r = shown;
    mc = Math.min(L1 - r.h, Math.max(L0 + r.h, r.c + (px / Math.max(1, width)) * 2 * r.h));
  }

  /** Scale labels: 1-2-5 style across decades, even steps once zoomed in. */
  function ticks(a: number, b: number): { hz: number; text: string }[] {
    if (b / a > 3) {
      return TICKS_MHZ.filter((m) => m * 1e6 >= a && m * 1e6 <= b).map((m) => ({
        hz: m * 1e6,
        text: m >= 1000 ? `${m / 1000} GHz` : `${m}`,
      }));
    }
    const raw = (b - a) / Math.max(2, width / 90);
    const p = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 5, 10].map((k) => k * p).find((s) => s >= raw) ?? 10 * p;
    const dec = Math.max(0, -Math.floor(Math.log10(step / 1e6) + 1e-9));
    const out: { hz: number; text: string }[] = [];
    for (let i = Math.ceil(a / step); i * step <= b; i++) out.push({ hz: i * step, text: ((i * step) / 1e6).toFixed(dec) });
    return out;
  }

  function hzAt(clientX: number): number {
    const r = canvas.getBoundingClientRect();
    return Math.round(scale.hz(clientX - r.left) / 1000) * 1000;
  }

  function draw() {
    if (!canvas) return;
    const W = Math.max(1, Math.floor(width * dpr));
    if (canvas.width !== W) canvas.width = W;
    if (canvas.height !== H * dpr) canvas.height = H * dpr;
    const ctx = canvas.getContext('2d')!;
    const x = (hz: number) => scale.x(hz) * dpr;
    ctx.clearRect(0, 0, W, H * dpr);
    const lo = shown.lo;
    const hi = shown.hi;

    // Ranges the receiver can't reach.
    if (covered.length) {
      ctx.fillStyle = 'rgba(148,163,184,0.10)';
      let from = lo;
      for (const r of [...covered].sort((a, b) => a.lo_hz - b.lo_hz)) {
        if (r.lo_hz > from) ctx.fillRect(x(from), 0, x(r.lo_hz) - x(from), (BAND_Y + BAND_H + 2) * dpr);
        from = Math.max(from, r.hi_hz);
      }
      if (from < hi) ctx.fillRect(x(from), 0, x(hi) - x(from), (BAND_Y + BAND_H + 2) * dpr);
    }

    // Bands, one row, in their group colour.
    ctx.fillStyle = 'rgba(148,163,184,0.08)';
    ctx.fillRect(0, BAND_Y * dpr, W, BAND_H * dpr);
    for (const b of BANDS) {
      if (!enabled.has(b.group) || b.hi < lo || b.lo > hi) continue;
      const x0 = x(b.lo);
      ctx.fillStyle = GROUP_COLOUR[b.group] + 'b0';
      ctx.fillRect(x0, BAND_Y * dpr, Math.max(1, x(b.hi) - x0), BAND_H * dpr);
    }

    // Bookmarks: short ticks under the bands.
    for (const b of bookmarks) {
      if (b.freq_hz < lo || b.freq_hz > hi) continue;
      ctx.fillStyle = colourOf(b) + 'cc';
      ctx.fillRect(Math.round(x(b.freq_hz)), (BAND_Y + BAND_H + 1) * dpr, 1 * dpr, 3 * dpr);
    }

    // Where the radio is, written under its marker (unless hovering), and
    // the scale around it.
    ctx.font = `${10 * dpr}px ui-sans-serif, system-ui, sans-serif`;
    ctx.textBaseline = 'middle';
    const tipHz = preview ?? (hoverX !== null ? scale.hz(hoverX) : null);
    const hereText = (win.c / 1e6).toFixed(3);
    const hereW = ctx.measureText(hereText).width + 8 * dpr;
    const hereX = Math.min(W - hereW, Math.max(0, x(win.c) - hereW / 2));
    const hereShown = tipHz === null;
    if (hereShown) {
      ctx.fillStyle = 'rgba(14,116,144,0.95)';
      ctx.fillRect(hereX, (LABEL_Y - 7) * dpr, hereW, 14 * dpr);
      ctx.fillStyle = '#f0f9ff';
      ctx.textAlign = 'left';
      ctx.fillText(hereText, hereX + 4 * dpr, LABEL_Y * dpr);
    }
    ctx.textAlign = 'center';
    let lastEnd = -Infinity;
    for (const { hz, text } of ticks(lo, hi)) {
      const xx = x(hz);
      const w = ctx.measureText(text).width;
      if (xx - w / 2 < lastEnd + 8 * dpr || xx - w / 2 < 0 || xx + w / 2 > W) continue;
      if (hereShown && xx + w / 2 > hereX - 6 * dpr && xx - w / 2 < hereX + hereW + 6 * dpr) continue;
      lastEnd = xx + w / 2;
      ctx.fillStyle = 'rgba(148,163,184,0.35)';
      ctx.fillRect(Math.round(xx), (BAND_Y + BAND_H) * dpr, 1, 4 * dpr);
      ctx.fillStyle = 'rgba(148,163,184,0.85)';
      ctx.fillText(text, xx, LABEL_Y * dpr);
    }

    const box = (c: number, span: number, minPx: number) => {
      let x0 = x(c - span / 2);
      let x1 = x(c + span / 2);
      if (x1 - x0 < minPx * dpr) {
        const m = (x0 + x1) / 2;
        x0 = m - (minPx / 2) * dpr;
        x1 = m + (minPx / 2) * dpr;
      }
      return [x0, x1];
    };

    // What the spectrum shows, when it isn't just the sampled window.
    if (Math.abs(view.hi - view.lo - win.span) > win.span * 0.05 || Math.abs((view.lo + view.hi) / 2 - win.c) > win.span * 0.05) {
      const [v0, v1] = box((view.lo + view.hi) / 2, view.hi - view.lo, 3);
      ctx.fillStyle = 'rgba(248,250,252,0.16)';
      ctx.fillRect(v0, 1 * dpr, v1 - v0, (BAND_H + 4) * dpr);
    }

    // Listening frequency.
    if (listenHz !== null && listenHz >= lo && listenHz <= hi) {
      ctx.fillStyle = 'rgba(34,197,94,0.95)';
      ctx.fillRect(Math.round(x(listenHz)) - 0.5 * dpr, 0, 1.5 * dpr, (BAND_Y + BAND_H + 3) * dpr);
    }

    // Sampled window, and where it's heading.
    // (When it's off the map, an arrow at the edge points to it instead.)
    ctx.strokeStyle = '#38bdf8';
    ctx.lineWidth = 1.5 * dpr;
    if (win.c + win.span / 2 >= lo && win.c - win.span / 2 <= hi) {
      const [w0, w1] = box(win.c, win.span, 5);
      ctx.fillStyle = 'rgba(56,189,248,0.35)';
      ctx.fillRect(w0, 0.75 * dpr, w1 - w0, (BAND_Y + BAND_H + 2) * dpr);
      ctx.strokeRect(w0, 0.75 * dpr, w1 - w0, (BAND_Y + BAND_H + 2) * dpr);
    }
    const next = preview ?? pending;
    if (next !== null && Math.abs(next - win.c) > 500) {
      const [p0, p1] = box(next, win.span, 5);
      ctx.setLineDash([3 * dpr, 2 * dpr]);
      ctx.strokeStyle = '#f8fafc';
      ctx.strokeRect(p0, 0.75 * dpr, p1 - p0, (BAND_Y + BAND_H + 2) * dpr);
      ctx.setLineDash([]);
    }

    // The radio is off the part of the map shown: point to it.
    if (win.c + win.span / 2 < lo || win.c - win.span / 2 > hi) {
      const d = win.c < lo ? 1 : -1;
      const ax = d > 0 ? 2 * dpr : W - 2 * dpr;
      const my = (BAND_Y + BAND_H / 2) * dpr;
      ctx.fillStyle = '#38bdf8';
      ctx.beginPath();
      ctx.moveTo(ax, my);
      ctx.lineTo(ax + d * 7 * dpr, my - 6 * dpr);
      ctx.lineTo(ax + d * 7 * dpr, my + 6 * dpr);
      ctx.closePath();
      ctx.fill();
    }

    // Under the cursor (or where a drag will go): frequency and band.
    if (tipHz !== null) {
      const xx = x(tipHz);
      const band = describe(bandAt(tipHz, enabled));
      const text = `${formatMHz(tipHz, 3)}${band ? ' · ' + band : ''}`;
      ctx.textAlign = 'left';
      const w = ctx.measureText(text).width + 8 * dpr;
      const left = Math.min(W - w, Math.max(0, xx - w / 2));
      ctx.fillStyle = 'rgba(2,6,23,0.92)';
      ctx.fillRect(left, (LABEL_Y - 7) * dpr, w, 14 * dpr);
      ctx.fillStyle = '#f8fafc';
      ctx.fillText(text, left + 4 * dpr, LABEL_Y * dpr);
      ctx.fillStyle = 'rgba(255,255,255,0.7)';
      ctx.fillRect(Math.round(xx), 0, 1, (BAND_Y + BAND_H + 2) * dpr);
    }
  }

  $effect(() => {
    void [shown, covered, win.c, win.span, pending, view.lo, view.hi, listenHz, bookmarks, enabled, hoverX, preview, width, dpr];
    draw();
  });

  function onDown(e: PointerEvent) {
    if (e.button !== 0) return;
    canvas.setPointerCapture(e.pointerId);
    if (e.pointerType === 'touch') {
      touches.set(e.pointerId, e.clientX);
      if (touches.size >= 2) {
        // Second finger: pinch instead of dragging.
        const [a, b] = [...touches.values()];
        pinch = { d0: Math.max(10, Math.abs(a - b)), h0: shown.h };
        drag = null;
        preview = null;
        return;
      }
    }
    // Zoomed in, dragging the map pans it; dragging the radio's marker (or
    // anywhere when the whole range is shown) moves the radio.
    const r = canvas.getBoundingClientRect();
    const half = Math.max(8, (scale.x(win.c + win.span / 2) - scale.x(win.c - win.span / 2)) / 2 + 4);
    const onMarker = Math.abs(e.clientX - r.left - scale.x(win.c)) <= half;
    drag = { id: e.pointerId, x0: e.clientX, moved: false, pan: zoomed && !(canTune && onMarker), c0: shown.c };
  }

  function onMove(e: PointerEvent) {
    const r = canvas.getBoundingClientRect();
    hoverX = e.clientX - r.left;
    if (touches.has(e.pointerId)) touches.set(e.pointerId, e.clientX);
    if (pinch && touches.size >= 2) {
      const [a, b] = [...touches.values()];
      const want = pinch.h0 * (pinch.d0 / Math.max(10, Math.abs(a - b)));
      zoomBy(shown.h / want, (a + b) / 2 - r.left);
      hoverX = null;
      return;
    }
    if (!drag || e.pointerId !== drag.id) return;
    if (!drag.moved && Math.abs(e.clientX - drag.x0) < 4) return;
    drag.moved = true;
    if (drag.pan) {
      const h = shown.h;
      mc = Math.min(L1 - h, Math.max(L0 + h, drag.c0 - ((e.clientX - drag.x0) / Math.max(1, width)) * 2 * h));
    } else {
      preview = hzAt(e.clientX);
    }
  }

  /** A finger lifted; true if that ended a pinch. */
  function endTouch(e: PointerEvent): boolean {
    touches.delete(e.pointerId);
    if (pinch && touches.size < 2) {
      pinch = null;
      // The finger left behind doesn't start a drag.
      drag = null;
      return true;
    }
    return false;
  }

  function onUp(e: PointerEvent) {
    const wasPinch = endTouch(e);
    // Touch has no hover to leave: drop the label with the finger.
    if (e.pointerType !== 'mouse') hoverX = null;
    if (wasPinch || !drag || e.pointerId !== drag.id) return;
    const d = drag;
    drag = null;
    if (d.pan && d.moved) return;
    const hz = d.moved ? (preview ?? hzAt(e.clientX)) : hzAt(e.clientX);
    preview = null;
    ongo(hz);
  }

  function onWheel(e: WheelEvent) {
    if (e.ctrlKey || e.metaKey) {
      // Ctrl + wheel, and trackpad pinch (which arrives as Ctrl + wheel).
      e.preventDefault();
      const r = canvas.getBoundingClientRect();
      zoomBy(Math.exp(-e.deltaY * 0.004), e.clientX - r.left);
      return;
    }
    if (!zoomed) return;
    // Sideways swipe, or Shift + wheel, pans; a plain wheel scrolls the page.
    const dx = e.deltaX !== 0 ? e.deltaX : e.shiftKey ? e.deltaY : 0;
    if (dx === 0) return;
    e.preventDefault();
    panBy(dx * (e.deltaMode === 1 ? 16 : 1));
  }

  function onKey(e: KeyboardEvent) {
    const zoomKey = { '+': 2, '=': 2, '-': 0.5, '_': 0.5 }[e.key];
    if (zoomKey || e.key === '0') {
      e.preventDefault();
      if (zoomKey) zoomBy(zoomKey);
      else fit();
      return;
    }
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    // Steps in proportion to the frequency, like the map's scale.
    const f = 1 + (e.shiftKey ? 0.1 : 0.01) * (e.key === 'ArrowLeft' ? -1 : 1);
    preview = Math.round(Math.min(hi, Math.max(lo, (preview ?? win.c) * f)) / 1000) * 1000;
    if (keyTimer) clearTimeout(keyTimer);
    keyTimer = setTimeout(() => {
      keyTimer = null;
      if (preview !== null) ongo(preview);
      preview = null;
    }, 500);
  }

  onMount(() => {
    const ro = new ResizeObserver(() => {
      dpr = window.devicePixelRatio || 1;
      width = wrap.clientWidth;
    });
    ro.observe(wrap);
    // Not passive, so Ctrl + wheel zooms the map rather than the page.
    canvas.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      ro.disconnect();
      canvas.removeEventListener('wheel', onWheel);
      if (keyTimer) clearTimeout(keyTimer);
    };
  });
</script>

<div class="minimap">
  <div class="track" bind:this={wrap}>
  <canvas
    bind:this={canvas}
    style:height="{H}px"
    tabindex="0"
    role="slider"
    aria-label={canTune
      ? 'Whole receiver range. Click, drag or use the arrow keys to move the radio; + and − zoom.'
      : 'Whole receiver range, showing where the radio is tuned; + and − zoom.'}
    aria-valuemin={lo}
    aria-valuemax={hi}
    aria-valuenow={win.c}
    aria-valuetext={formatMHz(win.c, 3)}
    title={canTune
      ? `Click or drag to move the radio${zoomed ? '; drag elsewhere to pan' : ''}. Ctrl + wheel or pinch zooms.`
      : `Click to look here (zoomed out)${zoomed ? '; drag to pan' : ''}. Ctrl + wheel or pinch zooms.`}
    data-lo={Math.round(shown.lo)}
    data-hi={Math.round(shown.hi)}
    onpointerdown={onDown}
    onpointermove={onMove}
    onpointerup={onUp}
    onpointercancel={(e) => {
      endTouch(e);
      drag = null;
      preview = null;
    }}
    onpointerleave={() => (hoverX = null)}
    onkeydown={onKey}
  ></canvas>
  </div>
  <div class="mzoom" role="group" aria-label="Mini map range">
    <button
      type="button"
      aria-label="Zoom the mini map out"
      title="Show more of the range (−)"
      disabled={!zoomed}
      onclick={() => zoomBy(0.5)}>−</button
    >
    <button
      type="button"
      class="lvl"
      aria-label="Mini map {zoomed ? levelText : 'shows everything'}: fit the whole range"
      title="Fit the whole range (0)"
      disabled={!zoomed}
      onclick={fit}>{zoomed ? levelText : 'All'}</button
    >
    <button
      type="button"
      aria-label="Zoom the mini map in"
      title="Show less of the range, in more detail (+). Ctrl + wheel or pinch zooms about the cursor."
      disabled={atMax}
      onclick={() => zoomBy(2)}>+</button
    >
  </div>
</div>

<style>
  .minimap {
    display: flex;
    background: #05080f;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .track {
    flex: 1;
    min-width: 0;
  }
  .mzoom {
    display: flex;
    border-left: 1px solid var(--border);
  }
  .mzoom button {
    min-width: 26px;
    padding: 0 6px;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: var(--muted);
    font-size: 14px;
    font-variant-numeric: tabular-nums;
  }
  .mzoom button + button {
    border-left: 1px solid var(--border);
  }
  .mzoom button:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-2);
  }
  .mzoom .lvl {
    min-width: 40px;
    font-size: 12px;
  }
  canvas {
    display: block;
    width: 100%;
    cursor: pointer;
    touch-action: pan-y;
  }
  canvas:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
