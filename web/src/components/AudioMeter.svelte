<script lang="ts">
  import { onMount } from 'svelte';

  /** Audio output meter for under the S-meter: LED segments for the level
   * (RMS) with a peak-hold segment that waits, then falls. Reads `levels`
   * every animation frame. */
  let { levels }: { levels: () => { rms: number; peak: number } | null } = $props();

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
</script>

<div
  class="ameter"
  role="meter"
  aria-label="Audio level"
  aria-valuemin={FLOOR}
  aria-valuemax={0}
  aria-valuenow={live ? Math.round(Math.max(FLOOR, rms)) : undefined}
  aria-valuetext={live ? `${Math.round(rms)} dBFS, peak ${Math.round(peak)} dBFS` : 'no audio'}
  title={live ? `Audio ${rms.toFixed(1)} dBFS · peak ${peak.toFixed(1)} dBFS` : 'Audio level (press Play)'}
>
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
