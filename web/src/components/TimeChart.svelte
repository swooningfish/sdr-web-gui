<script lang="ts">
  /**
   * One measurement over time: a single 2px line on a recessive grid, with a
   * hover crosshair + tooltip. The line breaks where the sensor went quiet, so
   * gaps aren't drawn as fake straight segments.
   */
  import type { Point } from '../lib/sensors';

  let {
    title,
    unit,
    digits = 1,
    points,
    from,
    to,
  }: { title: string; unit: string; digits?: number; points: Point[]; from: number; to: number } = $props();

  const H = 150;
  const M = { top: 10, right: 12, bottom: 22, left: 44 };

  let width = $state(320);
  let hover = $state<Point | null>(null);

  const plotW = $derived(Math.max(50, width - M.left - M.right));
  const plotH = H - M.top - M.bottom;

  function niceStep(span: number, target: number): number {
    const raw = span / target;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const n = raw / mag;
    return (n >= 5 ? 10 : n >= 2 ? 5 : n >= 1 ? 2 : 1) * mag;
  }

  const yScale = $derived.by(() => {
    let lo = Math.min(...points.map((p) => p.v));
    let hi = Math.max(...points.map((p) => p.v));
    if (!Number.isFinite(lo)) [lo, hi] = [0, 1];
    if (hi - lo < 1e-9) [lo, hi] = [lo - 1, hi + 1];
    const step = niceStep(hi - lo, 3);
    const min = Math.floor(lo / step) * step;
    const max = Math.ceil(hi / step) * step;
    const ticks: number[] = [];
    for (let v = min; v <= max + step / 2; v += step) ticks.push(+v.toFixed(10));
    return { min, max, ticks };
  });

  const x = (t: number) => M.left + ((t - from) / Math.max(1, to - from)) * plotW;
  const y = (v: number) => M.top + plotH - ((v - yScale.min) / (yScale.max - yScale.min)) * plotH;

  /** Break the line when the gap is far longer than the usual reporting interval. */
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

  /** Lone points (no neighbour within the gap) would be invisible as a line. */
  const isolated = $derived(points.length === 1 ? points : []);

  const xTicks = $derived.by(() => {
    const n = Math.max(2, Math.min(5, Math.floor(plotW / 90)));
    return Array.from({ length: n }, (_, i) => from + ((to - from) * i) / (n - 1));
  });

  const longRange = $derived(to - from > 36 * 3600_000);
  // Over 12 h the same clock time appears on two days, so add the weekday.
  const fmtTick = (t: number) =>
    longRange
      ? new Date(t).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' })
      : to - from > 12 * 3600_000
        ? new Date(t).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' })
        : new Date(t).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  const fmtVal = (v: number) => `${v.toFixed(digits)} ${unit}`;

  const latest = $derived(points.at(-1) ?? null);
  const stats = $derived.by(() => {
    if (points.length < 2) return null;
    const vs = points.map((p) => p.v);
    return { min: Math.min(...vs), max: Math.max(...vs) };
  });

  function onMove(e: PointerEvent) {
    const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
    const t = from + ((e.clientX - rect.left - M.left) / plotW) * (to - from);
    // Binary search for the nearest point in time.
    let lo = 0;
    let hi = points.length - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (points[mid].t < t) lo = mid;
      else hi = mid;
    }
    const a = points[lo];
    const b = points[hi];
    hover = !a ? null : !b || Math.abs(a.t - t) <= Math.abs(b.t - t) ? a : b;
  }
</script>

<figure class="chart">
  <figcaption>
    <span class="title">{title}</span>
    {#if latest}<span class="latest">{fmtVal(latest.v)}</span>{/if}
    {#if stats}<span class="muted small-text">min {fmtVal(stats.min)} · max {fmtVal(stats.max)}</span>{/if}
  </figcaption>
  <div class="plot" bind:clientWidth={width}>
    {#if points.length === 0}
      <p class="muted small-text empty">No readings in this period</p>
    {:else}
      <svg
        width={width}
        height={H}
        role="img"
        aria-label="{title} over time, {points.length} readings"
        onpointermove={onMove}
        onpointerleave={() => (hover = null)}
      >
        {#each yScale.ticks as v (v)}
          <line class="grid" x1={M.left} x2={M.left + plotW} y1={y(v)} y2={y(v)} />
          <text class="axis" x={M.left - 6} y={y(v)} text-anchor="end" dominant-baseline="middle"
            >{+v.toFixed(Math.max(0, digits))}</text
          >
        {/each}
        {#each xTicks as t, i (i)}
          <text
            class="axis"
            x={x(t)}
            y={H - 6}
            text-anchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}>{fmtTick(t)}</text
          >
        {/each}
        <path class="line" d={path} />
        {#each isolated as p (p.t)}
          <circle class="dot" cx={x(p.t)} cy={y(p.v)} r="4" />
        {/each}
        {#if hover}
          <line class="crosshair" x1={x(hover.t)} x2={x(hover.t)} y1={M.top} y2={M.top + plotH} />
          <circle class="dot" cx={x(hover.t)} cy={y(hover.v)} r="4" />
        {/if}
        <!-- Hit area covers the whole plot so the pointer needn't touch the line. -->
        <rect x={M.left} y={M.top} width={plotW} height={plotH} fill="transparent" />
      </svg>
      {#if hover}
        <div
          class="tip"
          style:left="{Math.min(Math.max(x(hover.t), 70), width - 70)}px"
          style:top="{Math.max(0, y(hover.v) - 44)}px"
        >
          <strong>{fmtVal(hover.v)}</strong>
          <span class="muted">{new Date(hover.t).toLocaleString(undefined, { dateStyle: longRange ? 'short' : undefined, timeStyle: 'medium' })}</span>
        </div>
      {/if}
    {/if}
  </div>
</figure>

<style>
  .chart {
    margin: 0 0 14px;
  }
  figcaption {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    margin-bottom: 4px;
    font-size: 14px;
  }
  .title {
    font-weight: 600;
  }
  .latest {
    font-variant-numeric: tabular-nums;
  }
  .plot {
    position: relative;
    width: 100%;
  }
  svg {
    display: block;
    touch-action: pan-y;
  }
  .grid {
    stroke: var(--border);
    stroke-width: 1;
  }
  .axis {
    fill: var(--muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
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
    transform: translateX(-50%);
    pointer-events: none;
    display: grid;
    padding: 4px 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    margin: 8px 0;
  }
</style>
