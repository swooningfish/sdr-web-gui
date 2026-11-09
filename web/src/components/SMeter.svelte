<script lang="ts">
  import { onMount } from 'svelte';
  import { needle, sReading } from '../lib/smeter';

  /** Analogue S-meter: a needle over an S1–S9 / +60 dB scale. */
  let { dbm, s9 = -73, label = '' }: { dbm: number | null; s9?: number; label?: string } = $props();

  const CX = 120;
  const CY = 152;
  const R = 112;
  const SWEEP = 48; // degrees either side of upright

  function at(pos: number, r: number): [number, number] {
    const a = ((pos * 2 - 1) * SWEEP * Math.PI) / 180;
    return [CX + r * Math.sin(a), CY - r * Math.cos(a)];
  }
  function arc(from: number, to: number, r: number): string {
    const [x1, y1] = at(from, r);
    const [x2, y2] = at(to, r);
    return `M${x1.toFixed(1)} ${y1.toFixed(1)} A${r} ${r} 0 0 1 ${x2.toFixed(1)} ${y2.toFixed(1)}`;
  }

  // S units: 0..9 at 0..0.6; over S9: +10..+60 at 0.6..1.
  const sTicks = Array.from({ length: 9 }, (_, i) => ({ pos: (0.6 * (i + 1)) / 9, label: i % 2 === 0 ? String(i + 1) : '' }));
  const dbTicks = Array.from({ length: 6 }, (_, i) => ({
    pos: 0.6 + (0.4 * (i + 1)) / 6,
    label: i % 2 === 1 ? `+${(i + 1) * 10}` : '',
  }));

  // Needle ballistics: quick to rise, slow to fall, like a real meter.
  let shown = $state(0);
  const target = $derived(dbm === null ? 0 : needle(dbm, s9));
  onMount(() => {
    let raf = 0;
    const step = () => {
      const d = target - shown;
      if (Math.abs(d) > 0.0005) shown += d * (d > 0 ? 0.35 : 0.08);
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  });
  const tip = $derived(at(shown, R + 6));
  const base = $derived(at(shown, 34));
</script>

<div
  class="smeter"
  role="meter"
  aria-label="Signal strength{label ? ` at ${label}` : ''}"
  aria-valuemin={s9 - 54}
  aria-valuemax={s9 + 60}
  aria-valuenow={dbm ?? undefined}
  aria-valuetext={dbm === null ? 'no signal reading' : `${dbm.toFixed(1)} dBm, ${sReading(dbm, s9)}`}
>
  <svg viewBox="0 0 240 132" aria-hidden="true">
    <defs>
      <linearGradient id="sm-face" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#f2efdc" />
        <stop offset="1" stop-color="#ddd8bd" />
      </linearGradient>
    </defs>
    <rect x="1" y="1" width="238" height="130" rx="6" fill="url(#sm-face)" stroke="#8a8778" stroke-width="2" />
    <path d={arc(0, 0.6, R)} fill="none" stroke="#1b1b1b" stroke-width="3" />
    <path d={arc(0.6, 1, R)} fill="none" stroke="#c81e1e" stroke-width="7" />
    {#each sTicks as t (t.pos)}
      {@const [x1, y1] = at(t.pos, R)}
      {@const [x2, y2] = at(t.pos, R + (t.label ? 9 : 6))}
      {@const [lx, ly] = at(t.pos, R + 17)}
      <line {x1} {y1} {x2} {y2} stroke="#1b1b1b" stroke-width="1.6" />
      {#if t.label}<text x={lx} y={ly} class="tick">{t.label}</text>{/if}
    {/each}
    {#each dbTicks as t (t.pos)}
      {@const [x1, y1] = at(t.pos, R + 3)}
      {@const [x2, y2] = at(t.pos, R + (t.label ? 10 : 7))}
      {@const [lx, ly] = at(t.pos, R + 19)}
      <line {x1} {y1} {x2} {y2} stroke="#c81e1e" stroke-width="1.6" />
      {#if t.label}<text x={lx} y={ly} class="tick red">{t.label}</text>{/if}
    {/each}
    <text x="22" y="40" class="unit">S</text>
    <text x="218" y="40" class="unit red">dB</text>
    <text x="16" y="122" class="readout" text-anchor="start">{dbm === null ? '— dBm' : `${dbm.toFixed(1)} dBm`}</text>
    <text x="224" y="122" class="readout" text-anchor="end">{dbm === null ? '' : sReading(dbm, s9)}</text>
    <line x1={base[0]} y1={base[1]} x2={tip[0]} y2={tip[1]} stroke="#2a2a2a" stroke-width="2.2" stroke-linecap="round" />
  </svg>
</div>

<style>
  .smeter {
    width: 100%;
    max-width: 240px;
    line-height: 0;
    border-radius: 8px;
    padding: 3px;
    background: linear-gradient(#5b5e66, #2d3038);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.25);
  }
  svg {
    width: 100%;
    height: auto;
    display: block;
  }
  .tick {
    font: 600 12px ui-sans-serif, system-ui, sans-serif;
    fill: #1b1b1b;
    text-anchor: middle;
    dominant-baseline: middle;
  }
  .unit {
    font: 700 14px ui-sans-serif, system-ui, sans-serif;
    fill: #1b1b1b;
    text-anchor: middle;
  }
  .red {
    fill: #c81e1e;
  }
  .readout {
    font: 13px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    fill: #6b6857;
    font-variant-numeric: tabular-nums;
  }
</style>
