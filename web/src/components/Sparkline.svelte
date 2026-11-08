<script lang="ts">
  /**
   * A compact single-series trend for dashboard cards: a 2px line, no axes,
   * with a hover crosshair + tooltip. Breaks where the sensor went quiet.
   */
  import type { Point } from '../lib/sensors';

  let {
    label,
    unit,
    digits = 1,
    points,
    from,
    to,
  }: { label: string; unit: string; digits?: number; points: Point[]; from: number; to: number } = $props();

  const H = 44;
  const PAD = 4;
  let width = $state(200);
  let hover = $state<Point | null>(null);

  const range = $derived.by(() => {
    let lo = Math.min(...points.map((p) => p.v));
    let hi = Math.max(...points.map((p) => p.v));
    if (!Number.isFinite(lo)) [lo, hi] = [0, 1];
    if (hi - lo < 1e-9) [lo, hi] = [lo - 1, hi + 1];
    return { lo, hi };
  });

  const x = (t: number) => ((t - from) / Math.max(1, to - from)) * width;
  const y = (v: number) => PAD + (H - 2 * PAD) * (1 - (v - range.lo) / (range.hi - range.lo));

  const path = $derived.by(() => {
    if (points.length === 0) return '';
    const dts = points.slice(1).map((p, i) => p.t - points[i].t).sort((a, b) => a - b);
    const median = dts.length ? dts[Math.floor(dts.length / 2)] : 0;
    const gap = Math.max(5 * median, 10 * 60_000);
    let d = '';
    points.forEach((p, i) => {
      const cmd = i === 0 || p.t - points[i - 1].t > gap ? 'M' : 'L';
      d += `${cmd}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`;
    });
    return d;
  });

  const fmt = (v: number) => `${v.toFixed(digits)} ${unit}`;

  function onMove(e: PointerEvent) {
    if (points.length === 0) return;
    const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
    const t = from + ((e.clientX - rect.left) / width) * (to - from);
    let best = points[0];
    for (const p of points) if (Math.abs(p.t - t) < Math.abs(best.t - t)) best = p;
    hover = best;
  }
</script>

<div class="spark" bind:clientWidth={width}>
  {#if points.length > 0}
    <svg
      {width}
      height={H}
      role="img"
      aria-label="{label} trend, {points.length} readings"
      onpointermove={onMove}
      onpointerleave={() => (hover = null)}
    >
      <path class="line" d={path} />
      {#if points.length === 1 || hover}
        {@const p = hover ?? points[0]}
        {#if hover}<line class="crosshair" x1={x(p.t)} x2={x(p.t)} y1="0" y2={H} />{/if}
        <circle class="dot" cx={x(p.t)} cy={y(p.v)} r="4" />
      {/if}
      <rect width={width} height={H} fill="transparent" />
    </svg>
    {#if hover}
      <div class="tip" style:left="{Math.min(Math.max(x(hover.t), 60), width - 60)}px">
        <strong>{fmt(hover.v)}</strong>
        <span class="muted">{new Date(hover.t).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}</span>
      </div>
    {/if}
  {/if}
</div>

<style>
  .spark {
    position: relative;
    width: 100%;
    height: 44px;
  }
  svg {
    display: block;
    touch-action: pan-y;
  }
  .line {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .dot {
    fill: var(--accent);
    stroke: var(--surface);
    stroke-width: 2;
  }
  .crosshair {
    stroke: var(--muted);
    stroke-width: 1;
    stroke-dasharray: 3 3;
  }
  .tip {
    position: absolute;
    bottom: calc(100% + 2px);
    transform: translateX(-50%);
    pointer-events: none;
    display: flex;
    gap: 6px;
    padding: 3px 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    z-index: 1;
  }
</style>
