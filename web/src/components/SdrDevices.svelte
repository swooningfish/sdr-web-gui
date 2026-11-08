<script lang="ts">
  import { api, type RadioState, type SdrDevice, type SdrDevices } from '../lib/api';
  import { describeRanges } from '../lib/coverage';
  import { formatMHz } from '../lib/format';

  let { radio, isAdmin }: { radio: RadioState | null; isAdmin: boolean } = $props();

  let info = $state<SdrDevices | null>(null);
  let loading = $state(false);
  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);

  async function load() {
    loading = true;
    try {
      info = await api.sdrDevices();
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // Reload when the receiver changes (here, from the Receiver panel, or unplugged).
  $effect(() => {
    void radio?.device_id;
    void radio?.connected;
    void radio?.tuner;
    load();
  });

  async function select(d: SdrDevice) {
    busy = d.id;
    err = null;
    try {
      await api.selectSdr(d.id);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  const supportedCount = (d: SdrDevice) =>
    info ? info.features.filter((f, i) => f.enabled && d.supported[i]).length : 0;
  const enabledCount = $derived(info ? info.features.filter((f) => f.enabled).length : 0);
</script>

<section class="panel">
  <div class="head">
    <h2>SDR devices</h2>
    <button type="button" class="small" onclick={load} disabled={loading}>{loading ? 'Scanning…' : 'Rescan'}</button>
  </div>
  <p class="muted small-text hint">
    Features on frequencies the selected receiver can't tune are shown as disabled throughout the app, and the scheduler
    skips them.
  </p>

  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else if info.devices.length === 0}
    <p class="muted">No receivers found. Plug in an RTL-SDR and press Rescan.</p>
  {:else}
    {#each info.devices as d (d.id)}
      <article class="dev" class:active={d.active}>
        <div class="dev-head">
          <div class="dev-title">
            <div class="name">
              <span class="dot" class:ok={d.active && !!radio?.connected} class:bad={d.active && !radio?.connected}></span>
              <strong>{d.name}</strong>
              {#if d.active}<span class="badge">In use</span>{/if}
            </div>
            <div class="muted small-text">
              {d.tuner ?? 'Tuner unknown'} · {describeRanges(d.ranges)}
            </div>
            {#if d.note}<div class="muted small-text">{d.note}</div>{/if}
          </div>
          {#if isAdmin && info.can_select && !d.active}
            <button type="button" class="small" disabled={busy !== null} onclick={() => select(d)}
              >{busy === d.id ? 'Switching…' : 'Use this device'}</button
            >
          {/if}
        </div>

        {#if info.features.length}
          <p class="muted small-text count">
            {#if d.ranges.length === 0}
              Supported features unknown until the tuner can be read
            {:else}
              Receives {supportedCount(d)} of {enabledCount} enabled features
            {/if}
          </p>
          <ul class="feats">
            {#each info.features as f, i (i)}
              {@const ok = d.supported[i]}
              <li
                class:no={!ok}
                class:off={!f.enabled}
                title={ok
                  ? `${formatMHz(f.freq_hz, 3)} is in range${f.enabled ? '' : ' (turned off in settings)'}`
                  : `Disabled: ${formatMHz(f.freq_hz, 3)} is outside ${describeRanges(d.ranges)}`}
              >
                <span class="mark" aria-hidden="true">{ok ? '✓' : '✕'}</span>
                <span class="grp">{f.group}</span>
                {f.label}
                <span class="muted">{formatMHz(f.freq_hz, 3)}</span>
                {#if !ok}<span class="sr-only">(disabled, out of range)</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
      </article>
    {/each}
    {#if !info.can_select}
      <p class="muted small-text">The {radio?.source ?? 'current'} source has a single fixed device.</p>
    {:else if !isAdmin}
      <p class="muted small-text">View only — an admin can switch receivers.</p>
    {/if}
  {/if}
  {#if err && info}<p class="error">{err}</p>{/if}
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }
  .head h2 {
    margin: 0;
  }
  .hint {
    margin: 0 0 10px;
  }
  .dev {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-top: 8px;
  }
  .dev.active {
    border-color: var(--accent);
  }
  .dev-head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
  }
  .dev-title {
    min-width: 0;
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    overflow-wrap: anywhere;
  }
  .badge {
    font-size: 12px;
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }
  .count {
    margin: 8px 0 4px;
  }
  .feats {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .feats li {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 12.5px;
    background: var(--surface-2);
  }
  .mark {
    color: var(--ok);
    font-weight: 700;
  }
  .grp {
    color: var(--muted);
  }
  .feats li.no {
    background: transparent;
    border-style: dashed;
    color: var(--muted);
  }
  .feats li.no .mark {
    color: var(--bad);
  }
  .feats li.off {
    opacity: 0.6;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
