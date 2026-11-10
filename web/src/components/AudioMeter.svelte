<script lang="ts">
  import { onMount } from 'svelte';

  /** Audio output meter for under the S-meter: the level (RMS) with a
   * peak-hold mark that waits, then falls. `digital` draws LED segments;
   * otherwise a needle with a red peak pointer. Reads `levels` every
   * animation frame. */
  let {
    levels,
    digital = true,
  }: { levels: () => { rms: number; peak: number } | null; digital?: boolean } = $props();

  // Needle geometry (a shallower cut of the S-meter's face).
  const CX = 120;
  const CY = 128;
  const R = 100;
  const SWEEP = 50; // degrees either side of upright
  const pos = (db: number) => Math.max(0, Math.min(1, (db - -54) / 54));
  function at(p: number, r: number): [number, number] {
    const a = ((p * 2 - 1) * SWEEP * Math.PI) / 180;
    return [CX + r * Math.sin(a), CY - r * Math.cos(a)];
  }
  function arc(from: number, to: number, r: number): string {
    const [x1, y1] = at(from, r);
    const [x2, y2] = at(to, r);
    return `M${x1.toFixed(1)} ${y1.toFixed(1)} A${r} ${r} 0 0 1 ${x2.toFixed(1)} ${y2.toFixed(1)}`;
  }
  const TICKS = Array.from({ length: 10 }, (_, i) => -54 + i * 6).concat(-3);

  const FLOOR = -54;
  const STEP = 3;
  const SEGS = -FLOOR / STEP; // 18 segments of 3 dB, −54…0 dBFS
  const HOLD_MS = 1500;
  const FALL_DB_PER_S = 18;

  let rms = $state(FLOOR - 1);
  let peak = $state(FLOOR - 1);
  let live = $state(false);
  let held = FLOOR - 1;
  let heldAt = 0;

  onMount(() => {
    let raf = 0;
    let last = performance.now();
    let shownRms = FLOOR - 1;
    const tick = (now: number) => {
      const dt = (now - last) / 1000;
      last = now;
      const l = levels();
      live = !!l;
      // Quick to rise, smooth fall for the bar.
      const target = l ? l.rms : FLOOR - 1;
      shownRms = target > shownRms ? target : Math.max(target, shownRms - 30 * dt);
      rms = shownRms;
      const p = l ? l.peak : FLOOR - 1;
      if (p >= held) {
        held = p;
        heldAt = now;
      } else if (now - heldAt > HOLD_MS) {
        held = Math.max(p, held - FALL_DB_PER_S * dt);
      }
      peak = held;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });

  /** Segment index a level lights up to (−1 = none). */
  const seg = (db: number) => Math.min(SEGS - 1, Math.floor((db - FLOOR) / STEP));
  const litTo = $derived(seg(rms));
  const peakSeg = $derived(seg(peak));
  const zone = (i: number) => {
    const top = FLOOR + (i + 1) * STEP;
    return top > -3 ? 'red' : top > -12 ? 'amber' : 'green';
  };
  const SCALE = [-48, -36, -24, -12, -6, 0];

  const tip = $derived(at(pos(rms), R + 5));
  const base = $derived(at(pos(rms), 44));
  const pkIn = $derived(at(pos(peak), R - 12));
  const pkOut = $derived(at(pos(peak), R + 5));
</script>

<div
  class="ameter"
  class:analogue={!digital}
  role="meter"
  aria-label="Audio level"
  aria-valuemin={FLOOR}
  aria-valuemax={0}
  aria-valuenow={live ? Math.round(Math.max(FLOOR, rms)) : undefined}
  aria-valuetext={live ? `${Math.round(rms)} dBFS, peak ${Math.round(peak)} dBFS` : 'no audio'}
  title={live ? `Audio ${rms.toFixed(1)} dBFS · peak ${peak.toFixed(1)} dBFS` : 'Audio level (press Play)'}
>
  {#if !digital}
    <svg viewBox="0 0 240 100" aria-hidden="true">
      <defs>
        <linearGradient id="am-face" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#f2efdc" />
          <stop offset="1" stop-color="#ddd8bd" />
        </linearGradient>
      </defs>
      <rect x="1" y="1" width="238" height="98" rx="6" fill="url(#am-face)" stroke="#8a8778" stroke-width="2" />
      <path d={arc(0, pos(-12), R)} fill="none" stroke="#1b1b1b" stroke-width="3" />
      <path d={arc(pos(-12), pos(-3), R)} fill="none" stroke="#d97706" stroke-width="5" />
      <path d={arc(pos(-3), 1, R)} fill="none" stroke="#c81e1e" stroke-width="7" />
      {#each TICKS as d (d)}
        {@const major = SCALE.includes(d)}
        {@const [x1, y1] = at(pos(d), R)}
        {@const [x2, y2] = at(pos(d), R + (major ? 8 : 5))}
        {@const [lx, ly] = at(pos(d), R + 16)}
        <line {x1} {y1} {x2} {y2} stroke={d > -3 ? '#c81e1e' : '#1b1b1b'} stroke-width="1.6" />
        {#if major}<text x={lx} y={ly} class="tick" class:red={d > -3}>{d}</text>{/if}
      {/each}
      <text x="16" y="91" class="readout" text-anchor="start">AF dBFS</text>
      <text x="224" y="91" class="readout" text-anchor="end">{live && peak > FLOOR ? `${peak.toFixed(0)} pk` : '— pk'}</text>
      {#if live && peak > FLOOR}
        <line x1={pkIn[0]} y1={pkIn[1]} x2={pkOut[0]} y2={pkOut[1]} stroke="#c81e1e" stroke-width="2.4" stroke-linecap="round" />
      {/if}
      <line x1={base[0]} y1={base[1]} x2={tip[0]} y2={tip[1]} stroke="#2a2a2a" stroke-width="2.2" stroke-linecap="round" />
    </svg>
  {:else}
  <span class="tag">AF</span>
  <div class="bar">
    <div class="segs">
      {#each Array.from({ length: SEGS }, (_, i) => i) as i (i)}
        <span class="s {zone(i)}" class:on={i <= litTo} class:pk={i === peakSeg && peakSeg > litTo}></span>
      {/each}
    </div>
    <div class="scale" aria-hidden="true">
      {#each SCALE as d (d)}
        <span style:left="{((d - FLOOR) / -FLOOR) * 100}%">{d}</span>
      {/each}
    </div>
  </div>
  <span class="pkread">{live && peak > FLOOR ? `${peak.toFixed(0)}` : '—'}<small>pk</small></span>
  {/if}
</div>

<style>
  .ameter {
    width: 100%;
    max-width: 240px;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) 3.2em;
    align-items: start;
    gap: 6px;
    padding: 5px 7px 3px;
    border-radius: 6px;
    background: #0b0d10;
    box-shadow:
      inset 0 1px 4px rgba(0, 0, 0, 0.8),
      0 0 0 2px var(--face-edge);
    font: 600 9px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: #8b93a1;
  }
  .ameter.analogue {
    display: block;
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
    font: 600 11px ui-sans-serif, system-ui, sans-serif;
    fill: #1b1b1b;
    text-anchor: middle;
    dominant-baseline: middle;
  }
  .tick.red {
    fill: #c81e1e;
  }
  .readout {
    font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    fill: #6b6857;
    font-variant-numeric: tabular-nums;
  }
  .tag {
    padding-top: 1px;
    letter-spacing: 0.08em;
  }
  .bar {
    min-width: 0;
  }
  .segs {
    display: grid;
    grid-template-columns: repeat(18, minmax(0, 1fr));
    gap: 2px;
    height: 9px;
  }
  .s {
    border-radius: 1px;
    opacity: 0.16;
  }
  .s.green {
    background: #22c55e;
  }
  .s.amber {
    background: #f59e0b;
  }
  .s.red {
    background: #ef4444;
  }
  .s.on,
  .s.pk {
    opacity: 1;
  }
  .s.on {
    box-shadow: 0 0 4px currentColor;
  }
  .scale {
    position: relative;
    height: 10px;
    margin-top: 2px;
  }
  .scale span {
    position: absolute;
    top: 0;
    transform: translateX(-50%);
    font-size: 8px;
    font-weight: 400;
  }
  .scale span:last-child {
    transform: translateX(-100%);
  }
  .pkread {
    text-align: right;
    color: #e5e7eb;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .pkread small {
    margin-left: 2px;
    font-size: 8px;
    color: #8b93a1;
  }
</style>
