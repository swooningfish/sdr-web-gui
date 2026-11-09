<script lang="ts">
  import { onMount } from 'svelte';
  import type { WaterfallFrame } from '../lib/stream';
  import type { FreqRange } from '../lib/api';
  import { PALETTES } from '../lib/colormap';
  import { formatMHz, formatSpan, MARKERS } from '../lib/format';
  import { channelEdges, CHANNEL_STEP, tunedHz, type Listen } from '../lib/audio';
  import { ALL_GROUPS, GROUPS, STRIP_H, drawStrip, type BandGroup } from '../lib/bandplan';
  import { colourOf, type Bookmark } from '../lib/bookmarks.svelte';
  import { fullRange, zoomLabel, zoomLimits } from '../lib/zoom';
  import MiniMap from './MiniMap.svelte';

  let {
    canTune = false,
    ontune,
    listen = null,
    sub = null,
    onpick,
    bookmarks = [],
    onbookmark,
    ranges = [],
    directSampling = 0,
    sampleRates = [],
    onrate,
  }: {
    canTune?: boolean;
    ontune?: (hz: number) => void;
    /** Channel being listened to, shaded on the spectrum. */
    listen?: Listen | null;
    /** The other VFO (not heard): a dim dashed marker. */
    sub?: { hz: number; mode: Listen['mode'] } | null;
    /** Single click: pick a frequency to listen to. */
    onpick?: (hz: number) => void;
    /** Bookmarks, flagged above the spectrum. */
    bookmarks?: Bookmark[];
    onbookmark?: (b: Bookmark) => void;
    /** What the receiver can tune (for the mini map); empty = unknown. */
    ranges?: FreqRange[];
    directSampling?: number;
    /** Sample rates the receiver offers, and a way to change it. */
    sampleRates?: number[];
    onrate?: (rate: number) => void;
  } = $props();

  const SPEC_CSS_H = 200;
  const WF_ROWS = 400;
  const FLAGS_H = 20;
  const TICK_STEPS = [
    100, 200, 500, 1e3, 2e3, 5e3, 10e3, 25e3, 50e3, 100e3, 200e3, 250e3, 500e3, 1e6, 2e6, 5e6, 10e6, 20e6, 50e6,
  ];
  const NICE_STEPS = [100, 500, 1e3, 5e3, 10e3, 25e3, 50e3, 100e3, 250e3, 500e3, 1e6, 2e6, 5e6, 10e6, 20e6];
  const MAX_HZ = 2_200_000_000;

  let container: HTMLDivElement;
  let plots: HTMLDivElement;
  let specCanvas: HTMLCanvasElement;
  let wfCanvas: HTMLCanvasElement;
  let bandCanvas = $state<HTMLCanvasElement | undefined>();

  let floorDb = $state(-100);
  let ceilDb = $state(-20);
  let peakHold = $state(false);
  let hover = $state<{ frac: number; hz: number; db: number } | null>(null);
  let info = $state<{ centerHz: number; sampleRate: number; bins: number } | null>(null);
  let fps = $state(0);
  let width = $state(800);

  function load<T>(key: string, fallback: T): T {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : (JSON.parse(v) as T);
    } catch {
      return fallback;
    }
  }
  function save(key: string, v: unknown) {
    try {
      localStorage.setItem(key, JSON.stringify(v));
    } catch {
      /* not available */
    }
  }

  // Colour scheme, remembered per browser (stored as a bare string).
  let palette = $state<string>(
    (() => {
      try {
        return localStorage.getItem('sdr.palette') ?? 'turbo';
      } catch {
        return 'turbo';
      }
    })(),
  );
  // Band plan groups shown, and the mini map, remembered per browser.
  let bandGroups = $state<BandGroup[]>(load('sdr.bands', ALL_GROUPS));
  const enabledBands = $derived(new Set(bandGroups));
  const showBands = $derived(bandGroups.length > 0);
  let showMap = $state<boolean>(load('sdr.minimap', true));

  // ------------------------------------------------------------- the view
  //
  // The radio samples a window (centre ± rate/2). The view is what's drawn:
  // `zoom` > 1 shows part of the window, < 1 shows more than it. `viewC` is
  // the view's centre, or null to stay centred on the window. Moving the
  // view past the window's edge retunes the radio (admins, zoom ≥ 1);
  // zoomed out, the window is outlined and admins drag it to retune.
  let zoom = $state(1);
  let viewC = $state<number | null>(null);
  /** Radio centre we've asked for and are waiting to see. */
  let target = $state<number | null>(null);
  /** Window being dragged (zoomed out): where it would go. */
  let winDrag = $state<number | null>(null);
  let commitTimer: ReturnType<typeof setTimeout> | null = null;
  let resetTimer: ReturnType<typeof setTimeout> | null = null;
  let drag = $state<{ x0: number; c0: number; moved: boolean; id: number; kind: 'view' | 'window' } | null>(null);
  let justDragged = false;
  const pointers = new Map<number, number>();
  let pinch: { d: number } | null = null;

  const span = $derived(info ? info.sampleRate / zoom : 1);
  const centre = $derived(info ? (viewC ?? info.centerHz) : 0);
  const vStart = $derived(centre - span / 2);
  const hzPerPx = $derived(span / Math.max(1, width));
  const winLo = $derived(info ? info.centerHz - info.sampleRate / 2 : 0);
  const winHi = $derived(info ? info.centerHz + info.sampleRate / 2 : 0);
  const limits = $derived(info ? zoomLimits(info.sampleRate, info.bins) : ([1, 1] as [number, number]));
  const full = $derived(fullRange(ranges, directSampling, winLo, winHi));
  /** Part of the view isn't sampled. */
  const overhang = $derived(!!info && (vStart < winLo - hzPerPx || vStart + span > winHi + hzPerPx));
  /** None of the view is sampled. */
  const offWindow = $derived(!!info && (vStart + span < winLo || vStart > winHi));
  const widest = $derived(sampleRates.length ? Math.max(...sampleRates) : 0);

  /** The waterfall image is the window; stretch and slide it into the view. */
  const wfTransform = $derived.by(() => {
    if (!info) return null;
    const k = info.sampleRate / span;
    const off = ((winLo - vStart) / span) * width;
    if (Math.abs(k - 1) < 1e-9 && Math.abs(off) < 0.01) return null;
    return `translateX(${off}px) scaleX(${k})`;
  });

  let cmap = PALETTES[0].make();
  let cmapId = '';
  const lut = new Uint8ClampedArray(256 * 4);
  let lutKey = '';
  let avg: Float32Array | null = null;
  let peak: Float32Array | null = null;
  let row: ImageData | null = null;
  let dpr = 1;
  let drawQueued = false;
  let framesSeen = 0;
  let fpsCount = 0;
  let fpsT0 = performance.now();
  let hatch: CanvasPattern | null = null;
  let hatchDpr = 0;

  function ensureLut(minDb: number, maxDb: number) {
    if (cmapId !== palette) {
      cmap = (PALETTES.find((p) => p.id === palette) ?? PALETTES[0]).make();
      cmapId = palette;
    }
    const key = `${minDb},${maxDb},${floorDb},${ceilDb},${palette}`;
    if (key === lutKey) return;
    lutKey = key;
    const step = (maxDb - minDb) / 255;
    const span = Math.max(1, ceilDb - floorDb);
    for (let v = 0; v < 256; v++) {
      const db = minDb + v * step;
      const t = Math.min(1, Math.max(0, (db - floorDb) / span));
      const c = Math.round(t * 255) * 3;
      lut[v * 4] = cmap[c];
      lut[v * 4 + 1] = cmap[c + 1];
      lut[v * 4 + 2] = cmap[c + 2];
      lut[v * 4 + 3] = 255;
    }
  }

  /** Called by the parent for every frame from the WebSocket. */
  export function push(f: WaterfallFrame) {
    const n = f.bins.length;
    if (!info || info.bins !== n || info.centerHz !== f.centerHz || info.sampleRate !== f.sampleRate) {
      const moved = !!info && info.centerHz !== f.centerHz;
      if (!info || info.bins !== n) {
        wfCanvas.width = n;
        wfCanvas.height = WF_ROWS;
        row = new ImageData(n, 1);
      } else if (info.sampleRate === f.sampleRate) {
        // Retuned: slide the history so it stays lined up with the new axis.
        const shift = Math.round(((f.centerHz - info.centerHz) / f.sampleRate) * n);
        const ctx = wfCanvas.getContext('2d')!;
        ctx.globalCompositeOperation = 'copy';
        ctx.drawImage(wfCanvas, -shift, 0);
        ctx.globalCompositeOperation = 'source-over';
      }
      avg = null;
      peak = null;
      info = { centerHz: f.centerHz, sampleRate: f.sampleRate, bins: n };
      if (zoom < limits[0] || zoom > limits[1]) zoom = Math.min(limits[1], Math.max(limits[0], zoom));
      if (moved) retuned(f.centerHz);
    }
    // Spectrum: light exponential smoothing in dB.
    const step = (f.maxDb - f.minDb) / 255;
    if (!avg) {
      avg = new Float32Array(n);
      for (let i = 0; i < n; i++) avg[i] = f.minDb + f.bins[i] * step;
    } else {
      for (let i = 0; i < n; i++) avg[i] = avg[i] * 0.5 + (f.minDb + f.bins[i] * step) * 0.5;
    }
    if (peakHold) {
      if (!peak) peak = Float32Array.from(avg);
      else for (let i = 0; i < n; i++) peak[i] = Math.max(peak[i] - 0.05, avg[i]);
    }

    // Level the colour scale from the very first frame, before painting it.
    framesSeen++;
    if (framesSeen === 1) autoLevel();
    ensureLut(f.minDb, f.maxDb);

    // Waterfall: scroll down one row and paint the new one at the top.
    const ctx = wfCanvas.getContext('2d')!;
    ctx.drawImage(wfCanvas, 0, 0, n, WF_ROWS - 1, 0, 1, n, WF_ROWS - 1);
    const d = row!.data;
    for (let i = 0; i < n; i++) {
      const v = f.bins[i] * 4;
      const o = i * 4;
      d[o] = lut[v];
      d[o + 1] = lut[v + 1];
      d[o + 2] = lut[v + 2];
      d[o + 3] = 255;
    }
    ctx.putImageData(row!, 0, 0);

    fpsCount++;
    const now = performance.now();
    if (now - fpsT0 >= 1000) {
      fps = Math.round((fpsCount * 1000) / (now - fpsT0));
      fpsCount = 0;
      fpsT0 = now;
    }
    queueDraw();
  }

  /** The radio's centre changed: ours arriving, or someone else's retune. */
  function retuned(hz: number) {
    if (!info) return;
    if (target !== null && Math.abs(hz - target) < 1000) {
      target = null;
      if (resetTimer) clearTimeout(resetTimer);
      resetTimer = null;
    } else if (viewC !== null && !drag) {
      // Retuned elsewhere (Receiver panel, a bookmark, rotation). Keep the
      // view if it still overlaps the window; otherwise follow the radio.
      const sr = info.sampleRate;
      if (offWindow) {
        const l = listen?.hz;
        viewC = zoom > 1 && l !== undefined && Math.abs(l - hz) < sr / 2 ? l : hz;
      }
      if (zoom >= 1) viewC = containView(viewC!);
    }
    settle();
  }

  /** At ×1 with the view back on the window, stop tracking a separate view. */
  function settle() {
    if (!info || viewC === null || drag || commitTimer) return;
    if (zoom === 1 && target === null && Math.abs(viewC - info.centerHz) < Math.max(1000, hzPerPx)) viewC = null;
  }

  export function autoLevel() {
    if (!avg) return;
    const sorted = Float32Array.from(avg).sort();
    const noise = sorted[Math.floor(sorted.length * 0.2)];
    const top = sorted[sorted.length - 1];
    floorDb = Math.round(noise - 8);
    ceilDb = Math.round(Math.max(top + 5, noise + 30));
  }

  function queueDraw() {
    if (drawQueued) return;
    drawQueued = true;
    requestAnimationFrame(() => {
      drawQueued = false;
      drawSpectrum();
      drawBands();
    });
  }

  function decimalsFor(stepHz: number): number {
    const mhz = stepHz / 1e6;
    for (let d = 0; d <= 6; d++) {
      const v = mhz * 10 ** d;
      if (Math.abs(Math.round(v) - v) < 1e-6) return d;
    }
    return 6;
  }

  function drawBands() {
    if (!bandCanvas || !info) return;
    const ctx = bandCanvas.getContext('2d')!;
    drawStrip(ctx, vStart, span, dpr, enabledBands, hover?.hz ?? null);
  }

  /** Diagonal hatching for parts of the view the radio isn't sampling. */
  function hatchPattern(ctx: CanvasRenderingContext2D): CanvasPattern | null {
    if (hatch && hatchDpr === dpr) return hatch;
    const s = Math.round(8 * dpr);
    const c = document.createElement('canvas');
    c.width = s;
    c.height = s;
    const g = c.getContext('2d')!;
    g.strokeStyle = 'rgba(148,163,184,0.16)';
    g.lineWidth = dpr;
    g.beginPath();
    g.moveTo(0, s);
    g.lineTo(s, 0);
    g.stroke();
    hatch = ctx.createPattern(c, 'repeat');
    hatchDpr = dpr;
    return hatch;
  }

  function drawSpectrum() {
    if (!specCanvas || !info) return;
    const ctx = specCanvas.getContext('2d')!;
    const W = specCanvas.width;
    const H = specCanvas.height;
    const axisH = 18 * dpr;
    const plotH = H - axisH;
    ctx.clearRect(0, 0, W, H);

    const dbSpan = Math.max(1, ceilDb - floorDb);
    const y = (db: number) => Math.min(plotH, Math.max(0, plotH - ((db - floorDb) / dbSpan) * plotH));
    const start = vStart;
    const end = vStart + span;
    const xOf = (hz: number) => ((hz - start) / span) * W;
    const inView = (hz: number) => hz >= start && hz <= end;
    const sr = info.sampleRate;
    const wx0 = xOf(winLo);
    const wx1 = xOf(winHi);

    ctx.font = `${11 * dpr}px ui-sans-serif, system-ui, sans-serif`;
    ctx.lineWidth = 1;

    // Not sampled: hatch either side of the window.
    if (overhang) {
      ctx.fillStyle = hatchPattern(ctx) ?? 'rgba(148,163,184,0.06)';
      if (wx0 > 0) ctx.fillRect(0, 0, Math.min(W, wx0), plotH);
      if (wx1 < W) ctx.fillRect(Math.max(0, wx1), 0, W - Math.max(0, wx1), plotH);
      ctx.fillStyle = 'rgba(148,163,184,0.55)';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      for (const [a, b] of [
        [0, Math.min(W, wx0)],
        [Math.max(0, wx1), W],
      ]) {
        if (b - a > 110 * dpr) ctx.fillText('not sampled', (a + b) / 2, plotH * 0.62);
      }
    }

    // dB grid
    ctx.strokeStyle = 'rgba(148,163,184,0.15)';
    ctx.fillStyle = 'rgba(148,163,184,0.75)';
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'left';
    for (let db = Math.ceil(floorDb / 10) * 10; db <= ceilDb; db += 10) {
      const yy = Math.round(y(db)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(0, yy);
      ctx.lineTo(W, yy);
      ctx.stroke();
      ctx.fillText(`${db} dB`, 4 * dpr, yy - 7 * dpr);
    }

    // Frequency grid + axis labels
    const maxTicks = W / (95 * dpr);
    const tick = TICK_STEPS.find((s) => span / s <= maxTicks) ?? 100e6;
    const dec = decimalsFor(tick);
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
    for (let hz = Math.ceil(start / tick) * tick; hz <= end; hz += tick) {
      const xx = Math.round(xOf(hz)) + 0.5;
      ctx.strokeStyle = 'rgba(148,163,184,0.15)';
      ctx.beginPath();
      ctx.moveTo(xx, 0);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
      ctx.fillStyle = 'rgba(148,163,184,0.9)';
      ctx.fillText((hz / 1e6).toFixed(dec), xx, H - 4 * dpr);
    }

    // Bookmarks: faint lines in their colour (the flags above say what they are).
    for (const b of bookmarks) {
      if (!inView(b.freq_hz)) continue;
      const xx = Math.round(xOf(b.freq_hz)) + 0.5;
      ctx.strokeStyle = colourOf(b) + '66';
      ctx.beginPath();
      ctx.moveTo(xx, 0);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
    }

    // Known-channel markers; labels that would overlap drop to the next line.
    ctx.textAlign = 'left';
    const lineEnds: number[] = [];
    for (const m of MARKERS) {
      if (!inView(m.hz)) continue;
      const xx = Math.round(xOf(m.hz)) + 0.5;
      const w = ctx.measureText(m.label).width;
      let line = lineEnds.findIndex((end) => xx + 3 * dpr > end + 6 * dpr);
      if (line === -1) line = lineEnds.length;
      lineEnds[line] = xx + 3 * dpr + w;
      const labelY = (11 + line * 13) * dpr;
      ctx.strokeStyle = 'rgba(250,204,21,0.55)';
      ctx.setLineDash([3 * dpr, 3 * dpr]);
      ctx.beginPath();
      ctx.moveTo(xx, labelY + 3 * dpr);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = 'rgba(250,204,21,0.95)';
      ctx.fillText(m.label, xx + 3 * dpr, labelY);
    }

    // Trace in view coordinates: the highest bin under each pixel, or
    // interpolated between bins when zoomed in past one bin per pixel.
    const trace = (data: Float32Array, stroke: string, fill: string | null) => {
      const n = data.length;
      const binAt = (x: number) => ((start + (x / W) * span - winLo) / sr) * n;
      let first = -1;
      let last = -1;
      ctx.beginPath();
      for (let x = 0; x < W; x++) {
        const f0 = binAt(x);
        const f1 = binAt(x + 1);
        if (f1 <= 0 || f0 >= n) continue;
        let m: number;
        if (f1 - f0 >= 1) {
          const b0 = Math.max(0, Math.floor(f0));
          const b1 = Math.min(n, Math.max(b0 + 1, Math.floor(f1)));
          m = -Infinity;
          for (let b = b0; b < b1; b++) m = Math.max(m, data[b]);
        } else {
          const fc = (f0 + f1) / 2 - 0.5;
          const i = Math.floor(fc);
          const t = fc - i;
          const a = data[Math.min(n - 1, Math.max(0, i))];
          const b = data[Math.min(n - 1, Math.max(0, i + 1))];
          m = a + (b - a) * t;
        }
        const yy = y(m);
        if (first < 0) {
          ctx.moveTo(x, yy);
          first = x;
        } else ctx.lineTo(x, yy);
        last = x;
      }
      if (first < 0) return;
      ctx.strokeStyle = stroke;
      ctx.lineWidth = 1.25 * dpr;
      ctx.stroke();
      if (fill) {
        ctx.lineTo(last, plotH);
        ctx.lineTo(first, plotH);
        ctx.closePath();
        ctx.fillStyle = fill;
        ctx.fill();
      }
    };
    // The other VFO: a dim outline so it can be found again.
    if (sub && !(listen && sub.hz === listen.hz && sub.mode === listen.mode)) {
      const [lo, hi] = channelEdges(sub.hz, sub.mode);
      const x0 = xOf(lo);
      const x1 = Math.max(x0 + 2 * dpr, xOf(hi));
      ctx.fillStyle = 'rgba(251,146,60,0.08)';
      ctx.fillRect(x0, 0, x1 - x0, plotH);
      const xs = Math.round(xOf(sub.hz)) + 0.5;
      ctx.strokeStyle = 'rgba(251,146,60,0.75)';
      ctx.lineWidth = dpr;
      ctx.setLineDash([3 * dpr, 3 * dpr]);
      ctx.beginPath();
      ctx.moveTo(xs, 0);
      ctx.lineTo(xs, plotH);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = 'rgba(251,146,60,0.95)';
      ctx.font = `${10 * dpr}px ui-sans-serif, system-ui, sans-serif`;
      ctx.fillText('B', xs + 3 * dpr, 11 * dpr);
      ctx.lineWidth = 1;
    }
    // Listening channel: shaded band plus a line where it is tuned. With AFC
    // following a signal the channel moves and a dashed line keeps the dial.
    if (listen) {
      const tuned = tunedHz(listen);
      const [lo, hi] = channelEdges(tuned, listen.mode);
      const x0 = xOf(lo);
      const x1 = Math.max(x0 + 2 * dpr, xOf(hi));
      ctx.fillStyle = listen.on ? 'rgba(34,197,94,0.18)' : 'rgba(148,163,184,0.12)';
      ctx.fillRect(x0, 0, x1 - x0, plotH);
      if (tuned !== listen.hz) {
        const xd = Math.round(xOf(listen.hz)) + 0.5;
        ctx.strokeStyle = 'rgba(148,163,184,0.7)';
        ctx.lineWidth = dpr;
        ctx.setLineDash([4 * dpr, 4 * dpr]);
        ctx.beginPath();
        ctx.moveTo(xd, 0);
        ctx.lineTo(xd, plotH);
        ctx.stroke();
        ctx.setLineDash([]);
      }
      const xx = Math.round(xOf(tuned)) + 0.5;
      ctx.strokeStyle = listen.on ? 'rgba(34,197,94,0.95)' : 'rgba(148,163,184,0.6)';
      ctx.lineWidth = 1.5 * dpr;
      ctx.beginPath();
      ctx.moveTo(xx, 0);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    if (peakHold && peak) trace(peak, 'rgba(244,114,182,0.7)', null);
    if (avg) trace(avg, '#38bdf8', 'rgba(56,189,248,0.12)');

    // The sampled window, outlined when the view shows more than it.
    if (overhang && !offWindow) {
      ctx.strokeStyle = 'rgba(56,189,248,0.9)';
      ctx.lineWidth = 1.5 * dpr;
      ctx.strokeRect(wx0, 0.75 * dpr, wx1 - wx0, plotH - 1.5 * dpr);
      if (zoom < 1) {
        const text = `Sampled ${formatSpan(sr)}${canTune ? ' · drag to move' : ''}`;
        ctx.font = `${10.5 * dpr}px ui-sans-serif, system-ui, sans-serif`;
        ctx.textAlign = 'left';
        ctx.textBaseline = 'middle';
        const w = ctx.measureText(text).width + 8 * dpr;
        const left = Math.min(W - w, Math.max(0, (wx0 + wx1) / 2 - w / 2));
        ctx.fillStyle = 'rgba(14,116,144,0.92)';
        ctx.fillRect(left, plotH - 16 * dpr, w, 14 * dpr);
        ctx.fillStyle = '#f0f9ff';
        ctx.fillText(text, left + 4 * dpr, plotH - 9 * dpr);
      }
      ctx.lineWidth = 1;
    }
    // Where the window is heading (being dragged, or waiting for the radio).
    const next = winDrag ?? (zoom < 1 ? target : null);
    if (next !== null) {
      const p0 = xOf(next - sr / 2);
      const p1 = xOf(next + sr / 2);
      ctx.strokeStyle = 'rgba(248,250,252,0.9)';
      ctx.setLineDash([4 * dpr, 3 * dpr]);
      ctx.lineWidth = 1.5 * dpr;
      ctx.strokeRect(p0, 0.75 * dpr, p1 - p0, plotH - 1.5 * dpr);
      ctx.setLineDash([]);
      ctx.lineWidth = 1;
    }

    // Hover cursor
    if (hover) {
      const xx = Math.round(hover.frac * W) + 0.5;
      ctx.strokeStyle = 'rgba(255,255,255,0.6)';
      ctx.beginPath();
      ctx.moveTo(xx, 0);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
    }
  }

  // ---------------------------------------------------------------- moving

  /** Where the view's centre may go. Viewers who can't retune stay inside
   * the window unless zoomed out. */
  function clampView(c: number): number {
    if (!info) return c;
    if (zoom >= 1 && !canTune) return containView(c);
    return Math.min(full.hi, Math.max(full.lo, c));
  }

  /** Centre that keeps a view no wider than the window inside it. */
  function containView(c: number): number {
    if (!info) return c;
    const half = info.sampleRate / 2 - info.sampleRate / zoom / 2;
    if (half < 0) return c;
    return Math.min(info.centerHz + half, Math.max(info.centerHz - half, c));
  }

  /** Step for keyboard panning: a round number near a tenth of the view. */
  function panStep(big: boolean): number {
    const want = span / (big ? 2 : 10);
    return [...NICE_STEPS].reverse().find((s) => s <= want) ?? NICE_STEPS[0];
  }

  function moveView(deltaHz: number, delayMs: number) {
    if (!info) return;
    viewC = clampView(centre + deltaHz);
    scheduleCommit(delayMs);
  }

  function scheduleCommit(delayMs: number) {
    if (commitTimer) clearTimeout(commitTimer);
    commitTimer = setTimeout(commit, delayMs);
  }

  /** After moving the view: retune if it has left the window (zoom ≥ 1). */
  function commit() {
    commitTimer = null;
    if (!info || viewC === null) return;
    if (canTune && zoom >= 1 && (vStart < winLo - hzPerPx || vStart + span > winHi + hzPerPx)) tuneRadio(viewC);
    settle();
  }

  /** Ask for a new radio centre. The view stays put meanwhile. */
  function tuneRadio(hz: number) {
    if (!info || !canTune) return;
    const half = info.sampleRate / 2;
    const want = Math.round(Math.min(MAX_HZ - half, Math.max(half, hz)) / 1000) * 1000;
    if (Math.abs(want - info.centerHz) < 500) {
      target = null;
      settle();
      return;
    }
    target = want;
    ontune?.(want);
    // If the radio doesn't follow (error, or rotation took it), give up.
    if (resetTimer) clearTimeout(resetTimer);
    resetTimer = setTimeout(() => {
      resetTimer = null;
      target = null;
      if (zoom === 1) viewC = null;
      else if (zoom > 1 && viewC !== null) viewC = containView(viewC);
    }, 4000);
  }

  /** Mini map: move the radio (and the view) there, or just look there. */
  function go(hz: number) {
    if (!info) return;
    if (canTune) {
      viewC = zoom === 1 ? hz : clampView(hz);
      tuneRadio(hz);
    } else if (zoom < 1) viewC = clampView(hz);
  }

  // ---------------------------------------------------------------- zoom

  /** Zoom by `f` about `anchor` (Hz), which stays where it is on screen. */
  function zoomBy(f: number, anchor?: number) {
    if (!info) return;
    const [zMin, zMax] = limits;
    let z = Math.min(zMax, Math.max(zMin, zoom * f));
    if (Math.abs(Math.log(z)) < 0.04) z = 1;
    if (z === zoom) return;
    const a = anchor ?? (listen && listen.hz >= vStart && listen.hz <= vStart + span ? listen.hz : centre);
    const frac = (a - vStart) / span;
    const newSpan = info.sampleRate / z;
    const c = a - frac * newSpan + newSpan / 2;
    zoom = z;
    viewC = clampView(c);
    // Back to ×1 near the window: snap onto it.
    if (z === 1 && viewC - newSpan / 2 < winHi && viewC + newSpan / 2 > winLo) viewC = null;
  }

  /** ×1, on the window. */
  function fit() {
    zoom = 1;
    viewC = null;
  }

  // -------------------------------------------------------------- pointer

  function hzAtClient(clientX: number): number {
    const rect = plots.getBoundingClientRect();
    return vStart + ((clientX - rect.left) / rect.width) * span;
  }

  function onWheel(e: WheelEvent) {
    if (!info) return;
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? width : 1;
    // Ctrl + wheel, or a trackpad pinch: zoom about the cursor.
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      const f = Math.exp(-Math.max(-200, Math.min(200, e.deltaY * unit)) * 0.0025);
      zoomBy(f, hzAtClient(e.clientX));
      return;
    }
    const horizontal = Math.abs(e.deltaX) > Math.abs(e.deltaY);
    // Vertical scrolling moves along the band only once the spectrum has
    // focus (click it first), so scrolling the page past it doesn't.
    if (!horizontal && document.activeElement !== plots) return;
    // Nothing to move for a viewer at ×1: let the page scroll.
    if (!canTune && zoom === 1) return;
    e.preventDefault();
    const px = horizontal ? e.deltaX * unit : -e.deltaY * unit;
    moveView(px * hzPerPx, 350);
  }

  function onPointerDown(e: PointerEvent) {
    if (!info || e.button !== 0) return;
    pointers.set(e.pointerId, e.clientX);
    if (pointers.size === 2) {
      // Two fingers: pinch to zoom.
      const [a, b] = [...pointers.values()];
      pinch = { d: Math.abs(a - b) };
      drag = null;
      winDrag = null;
      return;
    }
    const hz = hzAtClient(e.clientX);
    const onWin = zoom < 1 && canTune && hz >= winLo && hz <= winHi;
    if (!onWin && zoom === 1 && !canTune) return;
    drag = {
      x0: e.clientX,
      c0: onWin ? info.centerHz : centre,
      moved: false,
      id: e.pointerId,
      kind: onWin ? 'window' : 'view',
    };
  }

  function onPointerMove(e: PointerEvent) {
    if (pointers.has(e.pointerId)) pointers.set(e.pointerId, e.clientX);
    if (pinch && pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      const d = Math.abs(a - b);
      if (pinch.d > 20 && d > 20) zoomBy(d / pinch.d, hzAtClient((a + b) / 2));
      pinch.d = d;
      return;
    }
    if (!drag || e.pointerId !== drag.id || !info) return;
    const dx = e.clientX - drag.x0;
    if (!drag.moved && Math.abs(dx) < 5) return;
    if (!drag.moved) {
      drag.moved = true;
      plots.setPointerCapture(e.pointerId);
      if (commitTimer) clearTimeout(commitTimer);
      commitTimer = null;
    }
    if (drag.kind === 'window') {
      const half = info.sampleRate / 2;
      winDrag = Math.min(MAX_HZ - half, Math.max(half, drag.c0 + dx * hzPerPx));
    } else {
      viewC = clampView(drag.c0 - dx * hzPerPx);
    }
  }

  function onPointerUp(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pinch) {
      if (pointers.size < 2) {
        pinch = null;
        justDragged = true;
      }
      return;
    }
    if (!drag || e.pointerId !== drag.id) return;
    const { moved, kind } = drag;
    drag = null;
    if (!moved) return;
    justDragged = true;
    if (kind === 'window') {
      const hz = winDrag;
      winDrag = null;
      if (hz !== null) tuneRadio(hz);
    } else commit();
  }

  function onKey(e: KeyboardEvent) {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      if (!canTune && zoom === 1) return;
      e.preventDefault();
      moveView((e.key === 'ArrowLeft' ? -1 : 1) * panStep(e.shiftKey), 400);
    } else if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && listen && onpick) {
      e.preventDefault();
      const step = CHANNEL_STEP[listen.mode] * (e.shiftKey ? 10 : 1);
      onpick(listen.hz + (e.key === 'ArrowUp' ? step : -step));
    } else if (e.key === '+' || e.key === '=') {
      e.preventDefault();
      zoomBy(2);
    } else if (e.key === '-' || e.key === '_') {
      e.preventDefault();
      zoomBy(0.5);
    } else if (e.key === '0') {
      e.preventDefault();
      fit();
    }
  }

  function onMove(e: MouseEvent) {
    if (!info || drag?.moved) return;
    const rect = plots.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    const hz = vStart + frac * span;
    const bin = Math.floor(((hz - winLo) / info.sampleRate) * info.bins);
    hover = { frac, hz, db: avg && bin >= 0 && bin < info.bins ? avg[bin] : NaN };
  }

  function onClick() {
    if (justDragged) {
      justDragged = false;
      return;
    }
    if (onpick && hover) onpick(Math.round(hover.hz / 100) * 100);
  }

  function onDblClick() {
    if (!canTune || !hover) return;
    const hz = Math.round(hover.hz / 1000) * 1000;
    if (zoom !== 1) viewC = clampView(hz);
    tuneRadio(hz);
  }

  // ------------------------------------------------------------- bookmarks

  /** Flags in view; ones too close to the previous show as a dot only. */
  const flags = $derived.by(() => {
    if (!info) return [];
    let lastEnd = -Infinity;
    return bookmarks
      .filter((b) => b.freq_hz >= vStart && b.freq_hz <= vStart + span)
      .sort((a, b) => a.freq_hz - b.freq_hz)
      .map((b) => {
        const frac = (b.freq_hz - vStart) / span;
        const x = frac * width;
        const w = Math.min(140, b.name.length * 6.3 + 14);
        const full = x >= lastEnd && x + w <= width + 4;
        lastEnd = full ? x + w + 4 : Math.max(lastEnd, x + 8);
        return { b, frac, full };
      });
  });

  // Open the band plan menu towards whichever side has room.
  let menuOpen = $state(false);
  let menuEl = $state<HTMLDetailsElement | undefined>();
  let menuLeft = $state(false);
  $effect(() => {
    if (!menuOpen || !menuEl) return;
    const r = menuEl.getBoundingClientRect();
    menuLeft = r.right - 240 < 8;
  });

  function toggleGroup(g: BandGroup, on: boolean) {
    bandGroups = on ? [...new Set([...bandGroups, g])] : bandGroups.filter((x) => x !== g);
    save('sdr.bands', bandGroups);
  }

  $effect(() => {
    // Redraw when display settings or the view change.
    void floorDb;
    void ceilDb;
    void palette;
    void hover;
    void listen?.hz;
    void listen?.mode;
    void listen?.on;
    void sub?.hz;
    void sub?.mode;
    void bookmarks;
    void bandGroups;
    void vStart;
    void span;
    void winDrag;
    void target;
    if (!peakHold) peak = null;
    queueDraw();
  });

  // Size the band strip whenever it appears.
  $effect(() => {
    if (!bandCanvas) return;
    bandCanvas.width = Math.max(1, Math.floor(width * dpr));
    bandCanvas.height = Math.floor(STRIP_H * dpr);
    queueDraw();
  });

  onMount(() => {
    const resize = () => {
      dpr = window.devicePixelRatio || 1;
      width = container.clientWidth;
      specCanvas.width = Math.max(1, Math.floor(container.clientWidth * dpr));
      specCanvas.height = Math.floor(SPEC_CSS_H * dpr);
      if (bandCanvas) {
        bandCanvas.width = specCanvas.width;
        bandCanvas.height = Math.floor(STRIP_H * dpr);
      }
      queueDraw();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(container);
    resize();
    // Non-passive, so the wheel can tune or zoom instead of scrolling the page.
    plots.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      ro.disconnect();
      plots.removeEventListener('wheel', onWheel);
      if (commitTimer) clearTimeout(commitTimer);
      if (resetTimer) clearTimeout(resetTimer);
    };
  });
</script>

<div class="wf" bind:this={container}>
  <div class="toolbar">
    <div class="readout">
      {#if info}
        <span
          ><strong>{formatMHz(target ?? info.centerHz)}</strong>{#if target !== null}<span class="muted"> (tuning…)</span>{/if}</span
        >
        <span class="muted"
          >{#if zoom !== 1}view {formatSpan(span)} of{' '}{/if}span {(info.sampleRate / 1e6).toFixed(3)} MHz · {info.bins} bins · {fps} fps</span
        >
      {:else}
        <span class="muted">Waiting for data…</span>
      {/if}
      <!-- Always present (placeholder when not hovering) so the row height is stable. -->
      <span class="cursor" class:idle={!hover}
        >{hover ? `${formatMHz(hover.hz, 4)} · ${Number.isFinite(hover.db) ? hover.db.toFixed(1) : '–'} dB` : '–––.–––– MHz · – dB'}</span
      >
    </div>
    <div class="controls">
      <div class="zoom" role="group" aria-label="Zoom">
        <button
          type="button"
          class="small"
          aria-label="Zoom out"
          title="Zoom out (−). Past ×1 you see beyond what the radio samples."
          disabled={!info || zoom <= limits[0]}
          onclick={() => zoomBy(0.5)}>−</button
        >
        <button
          type="button"
          class="small level"
          aria-label="Zoom {zoomLabel(zoom)}: back to the sampled band"
          title="Back to ×1, the sampled band (0)"
          disabled={!info}
          onclick={fit}>{zoomLabel(zoom)}</button
        >
        <button
          type="button"
          class="small"
          aria-label="Zoom in"
          title="Zoom in (+). Ctrl + wheel or pinch zooms about the cursor."
          disabled={!info || zoom >= limits[1]}
          onclick={() => zoomBy(2)}>+</button
        >
      </div>
      <label title="Noise floor (bottom of the colour scale)">
        Floor <input type="range" min="-140" max="-20" bind:value={floorDb} /> <span class="num">{floorDb}</span>
      </label>
      <label title="Top of the colour scale">
        Top <input type="range" min="-90" max="10" bind:value={ceilDb} /> <span class="num">{ceilDb}</span>
      </label>
      <button type="button" class="small" onclick={autoLevel}>Auto</button>
      <label class="check"><input type="checkbox" bind:checked={peakHold} /> Peak</label>
      <select
        bind:value={palette}
        aria-label="Waterfall colours"
        onchange={() => {
          try {
            localStorage.setItem('sdr.palette', palette);
          } catch {
            /* not available */
          }
        }}
      >
        {#each PALETTES as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
      </select>
      <details class="bands-menu" bind:open={menuOpen} bind:this={menuEl}>
        <summary>Band plan</summary>
        <div class="menu" class:left={menuLeft} role="group" aria-label="Band plan groups">
          {#each GROUPS as g (g.id)}
            <label
              ><input
                type="checkbox"
                checked={bandGroups.includes(g.id)}
                onchange={(e) => toggleGroup(g.id, e.currentTarget.checked)}
              /><span class="swatch" style:background={g.colour}></span>{g.label}</label
            >
          {/each}
          <label class="map-toggle"
            ><input
              type="checkbox"
              bind:checked={showMap}
              onchange={() => save('sdr.minimap', showMap)}
            />Mini map of the whole range</label
          >
          <p class="muted small-text">UK plans, simplified: check the RSGB band plan before transmitting.</p>
        </div>
      </details>
    </div>
  </div>

  {#if info && showMap}
    <MiniMap
      lo={full.lo}
      hi={full.hi}
      covered={ranges}
      win={{ c: info.centerHz, span: info.sampleRate }}
      pending={target}
      view={{ lo: vStart, hi: vStart + span }}
      listenHz={listen?.hz ?? null}
      {bookmarks}
      enabled={enabledBands}
      {canTune}
      ongo={go}
    />
  {/if}

  {#if info && (offWindow || zoom < 1)}
    <div class="notice" role="status">
      {#if offWindow}
        <span>The radio isn't sampling this part of the band.</span>
        {#if canTune}
          <button type="button" class="small" onclick={() => tuneRadio(centre)}>Move the radio here</button>
        {/if}
      {:else}
        <span
          >Zoomed out: the radio samples the outlined {formatSpan(info.sampleRate)}.{#if canTune}{' '}Drag it, or
            click the mini map, to move it.{/if}</span
        >
        {#if canTune && onrate && widest > info.sampleRate}
          <button type="button" class="small" onclick={() => onrate?.(widest)}>Widen to {formatSpan(widest)}</button>
        {/if}
      {/if}
      <button type="button" class="small" onclick={fit}>Back to ×1</button>
    </div>
  {/if}

  <!-- Pointer and keyboard shortcuts; the same actions are available as form
       fields in the Listen and Receiver panels. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="plots"
    class:tunable={canTune || zoom !== 1}
    class:dragging={drag?.moved}
    bind:this={plots}
    data-view-lo={Math.round(vStart)}
    data-view-hi={Math.round(vStart + span)}
    data-win-lo={Math.round(winLo)}
    data-win-hi={Math.round(winHi)}
    tabindex="0"
    role="group"
    aria-label={canTune
      ? 'Spectrum. Click to listen; drag, scroll or use the left and right arrow keys to move along the band; plus and minus zoom; up and down arrows step the listening frequency.'
      : 'Spectrum. Click to listen; plus and minus zoom; up and down arrows step the listening frequency.'}
    onmousemove={onMove}
    onmouseleave={() => (hover = null)}
    onclick={onClick}
    ondblclick={onDblClick}
    onkeydown={onKey}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    title={[
      onpick ? 'Click to listen here' : '',
      canTune ? 'drag or scroll to move along the band (click first to scroll)' : '',
      'Ctrl + wheel to zoom',
      canTune ? 'double-click to centre here' : '',
    ]
      .filter(Boolean)
      .join(' · ')}
  >
    {#if showBands}
      <canvas class="bands" bind:this={bandCanvas} style:height="{STRIP_H}px"></canvas>
    {/if}
    {#if bookmarks.length}
      <div class="flags" style:height="{FLAGS_H}px">
        {#each flags as f (f.b.id)}
          <button
            type="button"
            class="flag"
            class:dot-only={!f.full}
            style:left="{f.frac * 100}%"
            style:--c={colourOf(f.b)}
            title="{f.b.name} · {formatMHz(f.b.freq_hz, 4)} {f.b.mode.toUpperCase()}{f.b.shared ? '' : ' (yours)'}"
            onpointerdown={(e) => e.stopPropagation()}
            onclick={(e) => {
              e.stopPropagation();
              onbookmark?.(f.b);
            }}>{f.full ? f.b.name : ''}</button
          >
        {/each}
      </div>
    {/if}
    <canvas class="spectrum" bind:this={specCanvas} style:height="{SPEC_CSS_H}px"></canvas>
    <div class="wfbox" class:sharp={zoom >= 4}>
      <canvas class="waterfall" bind:this={wfCanvas} width="1024" height={WF_ROWS} style:transform={wfTransform}></canvas>
    </div>
  </div>
</div>

<style>
  .wf {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  /* Readout on the first row, controls always on the second, so the hover
     readout appearing never re-wraps the controls (checked by
     web/tests/layout.spec.ts). */
  .toolbar {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 8px;
    font-size: 13px;
  }
  .readout {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    align-items: baseline;
  }
  .cursor {
    font-variant-numeric: tabular-nums;
    color: var(--accent);
    white-space: nowrap;
  }
  .cursor.idle {
    color: var(--muted);
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    align-items: center;
  }
  .controls label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .controls input[type='range'] {
    width: 110px;
  }
  .zoom {
    display: flex;
    gap: 2px;
  }
  .zoom button {
    min-width: 30px;
    font-variant-numeric: tabular-nums;
  }
  .zoom .level {
    min-width: 46px;
  }
  .num {
    width: 3.2em;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text);
  }
  .bands-menu {
    position: relative;
  }
  .bands-menu summary {
    cursor: pointer;
    list-style: none;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface-2);
  }
  .bands-menu summary::-webkit-details-marker {
    display: none;
  }
  .bands-menu summary::after {
    content: ' ▾';
    color: var(--muted);
  }
  .menu {
    position: absolute;
    z-index: 4;
    right: 0;
    top: calc(100% + 4px);
    width: 230px;
    max-width: calc(100vw - 32px);
    display: grid;
    gap: 6px;
    padding: 10px 12px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  }
  .menu.left {
    right: auto;
    left: 0;
  }
  .menu label {
    color: var(--text);
  }
  .menu .map-toggle {
    margin-top: 4px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }
  .menu p {
    margin: 4px 0 0;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    flex: none;
  }
  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding: 6px 10px;
    font-size: 13px;
    border: 1px solid color-mix(in srgb, #38bdf8 45%, var(--border));
    border-radius: 8px;
    background: color-mix(in srgb, #38bdf8 10%, var(--surface));
  }
  .notice span {
    flex: 1 1 220px;
  }
  .plots {
    display: flex;
    flex-direction: column;
    background: #05080f;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    cursor: crosshair;
  }
  .plots.tunable {
    /* Horizontal drags move along the band; vertical ones still scroll the page. */
    touch-action: pan-y;
  }
  .plots.dragging {
    cursor: grabbing;
  }
  .plots:focus-visible {
    outline-offset: -2px;
  }
  canvas {
    display: block;
    width: 100%;
  }
  .wfbox {
    position: relative;
    height: clamp(220px, 45vh, 520px);
    overflow: hidden;
    background: repeating-linear-gradient(
      -45deg,
      transparent 0 6px,
      rgba(148, 163, 184, 0.08) 6px 7px
    );
  }
  .waterfall {
    position: absolute;
    inset: 0;
    height: 100%;
    transform-origin: 0 0;
    image-rendering: auto;
  }
  .wfbox.sharp .waterfall {
    image-rendering: pixelated;
  }
  .flags {
    position: relative;
    border-bottom: 1px solid rgba(148, 163, 184, 0.12);
  }
  .flag {
    position: absolute;
    top: 2px;
    max-width: 140px;
    height: 16px;
    padding: 0 5px 0 6px;
    border: none;
    border-left: 2px solid var(--c);
    border-radius: 0 3px 3px 0;
    background: color-mix(in srgb, var(--c) 28%, transparent);
    color: #f8fafc;
    font-size: 11px;
    line-height: 16px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
  }
  .flag:hover,
  .flag:focus-visible {
    background: color-mix(in srgb, var(--c) 55%, transparent);
    z-index: 1;
  }
  .flag.dot-only {
    width: 6px;
    padding: 0;
    border-radius: 0;
  }
</style>
