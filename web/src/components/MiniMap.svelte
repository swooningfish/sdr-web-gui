<script lang="ts">
  import { onMount } from 'svelte';
  import { BANDS, GROUPS, bandAt, describe, type BandGroup } from '../lib/bandplan';
  import { formatMHz } from '../lib/format';
  import type { FreqRange } from '../lib/api';
  import { colourOf, type Bookmark } from '../lib/bookmarks.svelte';
  import { logScale } from '../lib/zoom';

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
  const TICKS_MHZ = [0.5, 1, 2, 5, 10, 20, 30, 50, 100, 200, 300, 500, 1000, 2000];

  let wrap: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(600);
  let dpr = $state(1);
  let hoverX = $state<number | null>(null);
  let preview = $state<number | null>(null);
  let drag: { id: number; x0: number; moved: boolean } | null = null;
  let keyTimer: ReturnType<typeof setTimeout> | null = null;

  const scale = $derived(logScale(lo, hi, width));

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
    for (const mhz of TICKS_MHZ) {
      const hz = mhz * 1e6;
      if (hz < lo || hz > hi) continue;
      const xx = x(hz);
      const text = mhz >= 1000 ? `${mhz / 1000} GHz` : `${mhz}`;
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
    const [w0, w1] = box(win.c, win.span, 5);
    ctx.strokeStyle = '#38bdf8';
    ctx.lineWidth = 1.5 * dpr;
    ctx.fillStyle = 'rgba(56,189,248,0.35)';
    ctx.fillRect(w0, 0.75 * dpr, w1 - w0, (BAND_Y + BAND_H + 2) * dpr);
    ctx.strokeRect(w0, 0.75 * dpr, w1 - w0, (BAND_Y + BAND_H + 2) * dpr);
    const next = preview ?? pending;
    if (next !== null && Math.abs(next - win.c) > 500) {
      const [p0, p1] = box(next, win.span, 5);
      ctx.setLineDash([3 * dpr, 2 * dpr]);
      ctx.strokeStyle = '#f8fafc';
      ctx.strokeRect(p0, 0.75 * dpr, p1 - p0, (BAND_Y + BAND_H + 2) * dpr);
      ctx.setLineDash([]);
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
    void [lo, hi, covered, win.c, win.span, pending, view.lo, view.hi, listenHz, bookmarks, enabled, hoverX, preview, width, dpr];
    draw();
  });

  function onDown(e: PointerEvent) {
    if (e.button !== 0) return;
    drag = { id: e.pointerId, x0: e.clientX, moved: false };
    canvas.setPointerCapture(e.pointerId);
  }

  function onMove(e: PointerEvent) {
    const r = canvas.getBoundingClientRect();
    hoverX = e.clientX - r.left;
    if (!drag || e.pointerId !== drag.id) return;
    if (!drag.moved && Math.abs(e.clientX - drag.x0) < 4) return;
    drag.moved = true;
    preview = hzAt(e.clientX);
  }

  function onUp(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.id) return;
    const hz = drag.moved ? (preview ?? hzAt(e.clientX)) : hzAt(e.clientX);
    drag = null;
    preview = null;
    // Touch has no hover to leave: drop the label with the finger.
    if (e.pointerType !== 'mouse') hoverX = null;
    ongo(hz);
  }

  function onKey(e: KeyboardEvent) {
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
    return () => {
      ro.disconnect();
      if (keyTimer) clearTimeout(keyTimer);
    };
  });
</script>

<div class="minimap" bind:this={wrap}>
  <canvas
    bind:this={canvas}
    style:height="{H}px"
    tabindex="0"
    role="slider"
    aria-label={canTune
      ? 'Whole receiver range. Click, drag or use the arrow keys to move the radio.'
      : 'Whole receiver range, showing where the radio is tuned.'}
    aria-valuemin={lo}
    aria-valuemax={hi}
    aria-valuenow={win.c}
    aria-valuetext={formatMHz(win.c, 3)}
    title={canTune ? 'Click or drag to move the radio' : 'Click to look here (zoomed out)'}
    onpointerdown={onDown}
    onpointermove={onMove}
    onpointerup={onUp}
    onpointercancel={() => {
      drag = null;
      preview = null;
    }}
    onpointerleave={() => (hoverX = null)}
    onkeydown={onKey}
  ></canvas>
</div>

<style>
  .minimap {
    background: #05080f;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
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
