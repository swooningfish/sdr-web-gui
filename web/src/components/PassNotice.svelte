<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type RadioState } from '../lib/api';
  import { formatMHz } from '../lib/format';
  import { schedule } from '../lib/schedule.svelte';

  /** Below the receiver: warns before the scheduler retunes for a satellite
   * pass (and the page switches to Passes), and lets admins stay put. */
  let {
    isAdmin,
    radio,
    keepPage,
    onkeeppage,
    onopen,
  }: {
    isAdmin: boolean;
    radio: RadioState | null;
    /** AOS of the pass this browser chose not to switch pages for. */
    keepPage: number | null;
    onkeeppage: (aos: number | null) => void;
    onopen: () => void;
  } = $props();

  onMount(() => schedule.use());

  let busy = $state(false);
  let err = $state<string | null>(null);

  const cfg = $derived(schedule.info?.config ?? null);
  const st = $derived(schedule.info?.status ?? null);
  const now = $derived(schedule.now);
  const up = $derived.by(() => {
    const u = st?.upcoming;
    if (!u || !cfg) return null;
    return now >= u.tune_ms - cfg.notice_s * 1000 && now < u.los_ms ? u : null;
  });
  const active = $derived(st?.active ?? null);

  /** "1:05", "12:40", "1:02:03" */
  function clock(ms: number): string {
    const s = Math.max(0, Math.round(ms / 1000));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const ss = String(s % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
  }
  const at = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

  async function skip(aos: number, yes: boolean) {
    busy = true;
    err = null;
    try {
      await api.skipPass(aos, yes);
      await schedule.load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if active}
  <div class="notice live" role="status">
    <span class="icon" aria-hidden="true">🛰</span>
    <div class="text">
      <strong>{active.satellite} {active.test ? 'test pass' : 'pass'} in progress</strong>
      <span class="muted">· ends in {clock(active.los_ms - now)}{active.recording ? ' · recording' : ''}</span>
    </div>
    <div class="actions">
      <button type="button" class="small" onclick={onopen}>Open Passes</button>
    </div>
  </div>
{:else if up && cfg}
  {@const tuneIn = up.tune_ms - now}
  <div class="notice" class:quiet={up.skipped || !cfg.auto_switch} role="status" aria-live="polite">
    <span class="icon" aria-hidden="true">🛰</span>
    <div class="text">
      <strong>{up.satellite} pass {tuneIn > 0 ? `in ${clock(up.aos_ms - now)}` : 'starting'}</strong>
      <span class="muted">· max {Math.round(up.max_el_deg)}° · {at(up.aos_ms)}–{at(up.los_ms)}</span>
      <p class="small-text detail">
        {#if !cfg.auto_switch}
          Auto-switch is off, so the receiver stays where it is.
        {:else if up.skipped}
          Staying on {radio ? formatMHz(radio.settings.center_hz, 3) : 'the current frequency'} for this pass (an admin chose
          to).
        {:else}
          In <strong class="num">{clock(tuneIn)}</strong> the receiver will tune to {formatMHz(up.center_hz, 3)}{keepPage ===
          up.aos_ms
            ? ''
            : ' and this page will switch to Passes'}.
          {#if isAdmin}Choose “Stay” to keep the current tuning; otherwise it switches automatically.{/if}
        {/if}
      </p>
      {#if err}<p class="error small-text">{err}</p>{/if}
    </div>
    <div class="actions">
      {#if cfg.auto_switch && isAdmin}
        {#if up.skipped}
          <button type="button" class="small" disabled={busy} onclick={() => skip(up.aos_ms, false)}>Switch after all</button>
        {:else}
          <button
            type="button"
            class="small primary"
            disabled={busy}
            title="Skip this pass: keep the receiver where it is"
            onclick={() => skip(up.aos_ms, true)}
            >Stay on {radio ? formatMHz(radio.settings.center_hz, 3) : 'this frequency'}</button
          >
        {/if}
      {/if}
      {#if cfg.auto_switch && !up.skipped}
        {#if keepPage === up.aos_ms}
          <button type="button" class="small" onclick={() => onkeeppage(null)}>Switch my page too</button>
        {:else}
          <button type="button" class="small" title="Don't switch this browser to the Passes page" onclick={() => onkeeppage(up.aos_ms)}
            >Keep this page</button
          >
        {/if}
      {/if}
      <button type="button" class="small" onclick={onopen}>Open Passes</button>
    </div>
  </div>
{/if}

<style>
  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding: 10px 14px;
    border: 1px solid var(--warn);
    border-left-width: 4px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--warn) 10%, var(--surface));
    min-width: 0;
  }
  .notice.quiet {
    border-color: var(--border);
    border-left-color: var(--muted);
    background: var(--surface);
  }
  .notice.live {
    border-color: var(--ok);
    background: color-mix(in srgb, var(--ok) 10%, var(--surface));
  }
  .icon {
    font-size: 20px;
    line-height: 1;
  }
  .text {
    flex: 1 1 260px;
    min-width: 0;
  }
  .detail {
    margin: 2px 0 0;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
</style>
