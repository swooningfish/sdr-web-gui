<script lang="ts">
  import { onMount } from 'svelte';
  import type { WaterfallFrame } from '../lib/stream';
  import { turbo } from '../lib/colormap';
  import { formatMHz, MARKERS } from '../lib/format';

  let { canTune = false, ontune }: { canTune?: boolean; ontune?: (hz: number) => void } = $props();

  const SPEC_CSS_H = 200;
  const WF_ROWS = 400;
  const TICK_STEPS = [1e3, 2e3, 5e3, 10e3, 25e3, 50e3, 100e3, 200e3, 250e3, 500e3, 1e6, 2e6];

  let container: HTMLDivElement;
  let specCanvas: HTMLCanvasElement;
  let wfCanvas: HTMLCanvasElement;

  let floorDb = $state(-100);
  let ceilDb = $state(-20);
  let peakHold = $state(false);
  let hover = $state<{ frac: number; hz: number; db: number } | null>(null);
  let info = $state<{ centerHz: number; sampleRate: number; bins: number } | null>(null);
  let fps = $state(0);

  const cmap = turbo();
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

  function ensureLut(minDb: number, maxDb: number) {
    const key = `${minDb},${maxDb},${floorDb},${ceilDb}`;
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
      if (!info || info.bins !== n) {
        wfCanvas.width = n;
        wfCanvas.height = WF_ROWS;
        row = new ImageData(n, 1);
      }
      avg = null;
      peak = null;
      info = { centerHz: f.centerHz, sampleRate: f.sampleRate, bins: n };
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

  function drawSpectrum() {
    if (!specCanvas || !info) return;
    const ctx = specCanvas.getContext('2d')!;
    const W = specCanvas.width;
    const H = specCanvas.height;
    const axisH = 18 * dpr;
    const plotH = H - axisH;
    ctx.clearRect(0, 0, W, H);

    const span = Math.max(1, ceilDb - floorDb);
    const y = (db: number) => Math.min(plotH, Math.max(0, plotH - ((db - floorDb) / span) * plotH));
    const start = info.centerHz - info.sampleRate / 2;
    const xOf = (hz: number) => ((hz - start) / info!.sampleRate) * W;

    ctx.font = `${11 * dpr}px ui-sans-serif, system-ui, sans-serif`;
    ctx.lineWidth = 1;

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
    const tick = TICK_STEPS.find((s) => info!.sampleRate / s <= maxTicks) ?? 5e6;
    const dec = decimalsFor(tick);
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
    for (let hz = Math.ceil(start / tick) * tick; hz <= start + info.sampleRate; hz += tick) {
      const xx = Math.round(xOf(hz)) + 0.5;
      ctx.strokeStyle = 'rgba(148,163,184,0.15)';
      ctx.beginPath();
      ctx.moveTo(xx, 0);
      ctx.lineTo(xx, plotH);
      ctx.stroke();
      ctx.fillStyle = 'rgba(148,163,184,0.9)';
      ctx.fillText((hz / 1e6).toFixed(dec), xx, H - 4 * dpr);
    }

    // Known-channel markers; labels that would overlap drop to the next line.
    ctx.textAlign = 'left';
    const lineEnds: number[] = [];
    for (const m of MARKERS) {
      if (m.hz < start || m.hz > start + info.sampleRate) continue;
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

    const trace = (data: Float32Array, stroke: string, fill: string | null) => {
      const n = data.length;
      ctx.beginPath();
      for (let x = 0; x < W; x++) {
        const b0 = Math.floor((x * n) / W);
        const b1 = Math.max(b0 + 1, Math.floor(((x + 1) * n) / W));
        let m = -Infinity;
        for (let b = b0; b < b1 && b < n; b++) m = Math.max(m, data[b]);
        const yy = y(m);
        if (x === 0) ctx.moveTo(x, yy);
        else ctx.lineTo(x, yy);
      }
      ctx.strokeStyle = stroke;
      ctx.lineWidth = 1.25 * dpr;
      ctx.stroke();
      if (fill) {
        ctx.lineTo(W, plotH);
        ctx.lineTo(0, plotH);
        ctx.closePath();
        ctx.fillStyle = fill;
        ctx.fill();
      }
    };
    if (peakHold && peak) trace(peak, 'rgba(244,114,182,0.7)', null);
    if (avg) trace(avg, '#38bdf8', 'rgba(56,189,248,0.12)');

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

  function onMove(e: MouseEvent) {
    if (!info) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    const hz = info.centerHz - info.sampleRate / 2 + frac * info.sampleRate;
    const bin = Math.min(info.bins - 1, Math.floor(frac * info.bins));
    hover = { frac, hz, db: avg ? avg[bin] : NaN };
  }

  function onDblClick() {
    if (canTune && hover && ontune) ontune(Math.round(hover.hz / 1000) * 1000);
  }

  $effect(() => {
    // Redraw when display settings change.
    void floorDb;
    void ceilDb;
    void hover;
    if (!peakHold) peak = null;
    queueDraw();
  });

  onMount(() => {
    const resize = () => {
      dpr = window.devicePixelRatio || 1;
      specCanvas.width = Math.max(1, Math.floor(container.clientWidth * dpr));
      specCanvas.height = Math.floor(SPEC_CSS_H * dpr);
      queueDraw();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(container);
    resize();
    return () => ro.disconnect();
  });
</script>

<div class="wf" bind:this={container}>
  <div class="toolbar">
    <div class="readout">
      {#if info}
        <span><strong>{formatMHz(info.centerHz)}</strong></span>
        <span class="muted">span {(info.sampleRate / 1e6).toFixed(3)} MHz · {info.bins} bins · {fps} fps</span>
      {:else}
        <span class="muted">Waiting for data…</span>
      {/if}
      {#if hover}
        <span class="cursor">{formatMHz(hover.hz, 4)} · {Number.isFinite(hover.db) ? hover.db.toFixed(1) : '–'} dB</span>
      {/if}
    </div>
    <div class="controls">
      <label title="Noise floor (bottom of the colour scale)">
        Floor <input type="range" min="-140" max="-20" bind:value={floorDb} /> <span class="num">{floorDb}</span>
      </label>
      <label title="Top of the colour scale">
        Top <input type="range" min="-90" max="10" bind:value={ceilDb} /> <span class="num">{ceilDb}</span>
      </label>
      <button type="button" class="small" onclick={autoLevel}>Auto</button>
      <label class="check"><input type="checkbox" bind:checked={peakHold} /> Peak</label>
    </div>
  </div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="plots"
    onmousemove={onMove}
    onmouseleave={() => (hover = null)}
    ondblclick={onDblClick}
    title={canTune ? 'Double-click to centre here' : ''}
  >
    <canvas class="spectrum" bind:this={specCanvas} style:height="{SPEC_CSS_H}px"></canvas>
    <canvas class="waterfall" bind:this={wfCanvas} width="1024" height={WF_ROWS}></canvas>
  </div>
</div>

<style>
  .wf {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 8px 16px;
    align-items: center;
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
  .num {
    width: 3.2em;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text);
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
  canvas {
    display: block;
    width: 100%;
  }
  .waterfall {
    height: clamp(220px, 45vh, 520px);
    image-rendering: auto;
  }
</style>
