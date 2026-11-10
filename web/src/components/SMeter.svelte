<script lang="ts">
  import { onMount } from 'svelte';
  import { needle, sReading } from '../lib/smeter';

  /** S-meter over an S1–S9 / +60 dB scale: a needle, or with `digital` a row
   * of LED segments and a numeric readout. `bar` draws it slim (a straight
   * scale) for the folded receiver bar. */
  let {
    dbm,
    s9 = -73,
    label = '',
    bar = false,
    digital = false,
  }: { dbm: number | null; s9?: number; label?: string; bar?: boolean; digital?: boolean } = $props();

  // Digital: 15 equal segments, S1–S9 then +10…+60 dB (the needle's scale, cut up).
  const SEGS = 15;
  const DIG_SCALE = [
    ...[1, 3, 5, 7, 9].map((s) => ({ seg: s, label: String(s), red: false })),
    ...[20, 40, 60].map((d) => ({ seg: 9 + d / 10, label: `+${d}`, red: true })),
  ];

  // Edgewise (bar) geometry: the scale runs from BX0 to BX1.
  const BX0 = 9;
  const BX1 = 141;
  const bx = (pos: number) => BX0 + pos * (BX1 - BX0);

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
  const lit = $derived(Math.round(shown * SEGS));
</script>

<div
  class="smeter"
  class:bar
  class:digital
  title={bar || digital ?(dbm === null ? 'No signal reading' : `${dbm.toFixed(1)} dBm · ${sReading(dbm, s9)}`) : undefined}
  role="meter"
  aria-label="Signal strength{label ? ` at ${label}` : ''}"
  aria-valuemin={s9 - 54}
  aria-valuemax={s9 + 60}
  aria-valuenow={dbm ?? undefined}
  aria-valuetext={dbm === null ? 'no signal reading' : `${dbm.toFixed(1)} dBm, ${sReading(dbm, s9)}`}
>
  {#if digital}
    {#if !bar}
      <div class="read" aria-hidden="true">
        <span class="sval">{dbm === null ? 'S —' : sReading(dbm, s9)}</span>
        <span class="dval">{dbm === null ? '— dBm' : `${dbm.toFixed(1)} dBm`}</span>
      </div>
    {/if}
    <div class="segs" aria-hidden="true">
      {#each Array.from({ length: SEGS }, (_, i) => i) as i (i)}
        <span class="s" class:over={i >= 9} class:on={i < lit}></span>
      {/each}
    </div>
    {#if !bar}
      <div class="scale" aria-hidden="true">
        {#each DIG_SCALE as t (t.seg)}
          <span class:over={t.red} style:left="{((t.seg - 0.5) / SEGS) * 100}%">{t.label}</span>
        {/each}
      </div>
    {/if}
  {:else if bar}
    <svg viewBox="0 0 150 26" aria-hidden="true">
      <defs>
        <linearGradient id="sm-bar-face" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#f2efdc" />
          <stop offset="1" stop-color="#ddd8bd" />
        </linearGradient>
      </defs>
      <rect x="0.5" y="0.5" width="149" height="25" rx="3" fill="url(#sm-bar-face)" stroke="#8a8778" />
      <line x1={BX0} y1="19" x2={bx(0.6)} y2="19" stroke="#1b1b1b" stroke-width="1.5" />
      <line x1={bx(0.6)} y1="19" x2={BX1} y2="19" stroke="#c81e1e" stroke-width="3.5" />
      {#each sTicks as t (t.pos)}
        <line x1={bx(t.pos)} y1="19" x2={bx(t.pos)} y2={t.label ? 14 : 16} stroke="#1b1b1b" stroke-width="1" />
        {#if t.label}<text x={bx(t.pos)} y="10" class="tick bar-tick">{t.label}</text>{/if}
      {/each}
      {#each dbTicks as t (t.pos)}
        <line x1={bx(t.pos)} y1="18" x2={bx(t.pos)} y2={t.label ? 14 : 16} stroke="#c81e1e" stroke-width="1" />
        {#if t.label}<text x={bx(t.pos)} y="10" class="tick bar-tick red">{t.label}</text>{/if}
      {/each}
      <line x1={bx(shown)} y1="2.5" x2={bx(shown)} y2="23.5" stroke="#2a2a2a" stroke-width="1.6" stroke-linecap="round" />
    </svg>
  {:else}
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
  {/if}
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
  .smeter.bar {
    max-width: 150px;
    border-radius: 5px;
    padding: 2px;
  }
  .smeter.digital {
    padding: 8px 9px 5px;
    border-radius: 6px;
    line-height: 1;
    background: #0b0d10;
    box-shadow:
      inset 0 1px 4px rgba(0, 0, 0, 0.8),
      0 0 0 2px var(--face-edge, #2d3038);
    font: 600 9px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: #8b93a1;
  }
  .smeter.digital.bar {
    padding: 6px 6px;
    align-self: center;
  }
  .read {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 8px;
    font-variant-numeric: tabular-nums;
  }
  .sval {
    font-size: 20px;
    font-weight: 700;
    color: #e5e7eb;
    white-space: nowrap;
  }
  .dval {
    font-size: 11px;
    white-space: nowrap;
  }
  .segs {
    display: grid;
    grid-template-columns: repeat(15, minmax(0, 1fr));
    gap: 2px;
    height: 14px;
  }
  .bar .segs {
    height: 10px;
    gap: 1.5px;
  }
  .s {
    border-radius: 1px;
    background: #22c55e;
    color: #22c55e;
    opacity: 0.16;
  }
  .s.over {
    background: #ef4444;
    color: #ef4444;
  }
  .s.on {
    opacity: 1;
    box-shadow: 0 0 4px currentColor;
  }
  .scale {
    position: relative;
    height: 10px;
    margin-top: 3px;
  }
  .scale span {
    position: absolute;
    top: 0;
    transform: translateX(-50%);
    font-size: 8px;
    font-weight: 400;
  }
  .scale span.over {
    color: #f87171;
  }
  .scale span:last-child {
    left: 100% !important;
    transform: translateX(-100%);
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
  .bar-tick {
    font-size: 8px;
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
