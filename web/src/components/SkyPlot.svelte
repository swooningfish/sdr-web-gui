<script lang="ts">
  /**
   * Polar sky plot: zenith in the centre, horizon at the edge, north up,
   * east to the right (looking up from the ground station).
   */
  import type { TrackPoint } from '../lib/api';

  let {
    track,
    now = null,
    label,
  }: { track: TrackPoint[]; now?: { az: number; el: number } | null; label: string } = $props();

  const R = 100;
  const C = 115;
  const xy = (az: number, el: number): [number, number] => {
    const r = ((90 - Math.max(0, el)) / 90) * R;
    const a = (az * Math.PI) / 180;
    return [C + r * Math.sin(a), C - r * Math.cos(a)];
  };

  const path = $derived(
    track.map(([, az, el], i) => `${i ? 'L' : 'M'}${xy(az, el).map((v) => v.toFixed(1)).join(',')}`).join(''),
  );
  const aos = $derived(track.length ? xy(track[0][1], track[0][2]) : null);
  const los = $derived(track.length ? xy(track[track.length - 1][1], track[track.length - 1][2]) : null);
  const pos = $derived(now && now.el >= 0 ? xy(now.az, now.el) : null);
</script>

<svg viewBox="0 0 230 230" role="img" aria-label={label}>
  {#each [0, 30, 60] as el (el)}
    <circle class="ring" cx={C} cy={C} r={((90 - el) / 90) * R} />
    {#if el}<text class="tick" x={C + 3} y={C - ((90 - el) / 90) * R + 10}>{el}°</text>{/if}
  {/each}
  <line class="ring" x1={C - R} y1={C} x2={C + R} y2={C} />
  <line class="ring" x1={C} y1={C - R} x2={C} y2={C + R} />
  <text class="dir" x={C} y={C - R - 4} text-anchor="middle">N</text>
  <text class="dir" x={C + R + 4} y={C + 4}>E</text>
  <text class="dir" x={C} y={C + R + 12} text-anchor="middle">S</text>
  <text class="dir" x={C - R - 4} y={C + 4} text-anchor="end">W</text>
  {#if path}
    <path class="track" d={path} />
    {#if aos}<circle class="end" cx={aos[0]} cy={aos[1]} r="3.5"><title>AOS</title></circle>{/if}
    {#if los}<rect class="end" x={los[0] - 3} y={los[1] - 3} width="6" height="6"><title>LOS</title></rect>{/if}
  {/if}
  {#if pos}
    <circle class="sat" cx={pos[0]} cy={pos[1]} r="6" />
  {/if}
</svg>

<style>
  svg {
    display: block;
    width: 100%;
    max-width: 300px;
    height: auto;
    margin: 0 auto;
  }
  .ring {
    fill: none;
    stroke: var(--border);
    stroke-width: 1;
  }
  .tick,
  .dir {
    fill: var(--muted);
    font-size: 10px;
  }
  .dir {
    font-weight: 600;
    font-size: 11px;
  }
  .track {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .end {
    fill: var(--surface);
    stroke: var(--accent);
    stroke-width: 2;
  }
  .sat {
    fill: var(--accent);
    stroke: var(--surface);
    stroke-width: 2;
  }
</style>
