<script lang="ts">
  import RotationToggle from './RotationToggle.svelte';
  import { api, type RadioPatch, type RadioState, type SdrDevice } from '../lib/api';
  import { describeRanges, unsupported } from '../lib/coverage';
  import { formatMHz, formatRate, parseFrequency, PRESETS } from '../lib/format';

  let { radio, isAdmin }: { radio: RadioState | null; isAdmin: boolean } = $props();

  let freqInput = $state('');
  let editing = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let devices = $state<SdrDevice[]>([]);
  const GROUPS = [
    { source: 'rtlsdr', label: 'USB' },
    { source: 'rtl_tcp', label: 'Network (rtl_tcp)' },
    { source: 'replay', label: 'Recording' },
    { source: 'synthetic', label: 'Simulated' },
  ] as const;

  // Device list for the picker; refreshed when the receiver changes.
  $effect(() => {
    void radio?.device_id;
    void radio?.connected;
    if (!radio?.can_select) return;
    api.sdrDevices().then(
      (r) => (devices = r.devices),
      () => {},
    );
  });

  async function selectDevice(e: Event) {
    const id = (e.currentTarget as HTMLSelectElement).value;
    busy = true;
    err = null;
    try {
      await api.selectSdr(id);
    } catch (ex) {
      err = ex instanceof Error ? ex.message : String(ex);
    } finally {
      busy = false;
    }
  }

  // Show the radio's frequency when it changes, but never over something
  // typed and not yet submitted: clicking Tune blurs the box before the
  // click lands, so blur alone mustn't reset it. Typed text is dropped only
  // once the box has been left for a moment without submitting.
  let dirty = $state(false);
  let shownHz: number | null = null;
  let blurTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    const hz = radio?.settings.center_hz;
    if (hz === undefined || editing || dirty || hz === shownHz) return;
    shownHz = hz;
    freqInput = (hz / 1e6).toFixed(4);
  });
  function leaveFreq() {
    editing = false;
    if (blurTimer) clearTimeout(blurTimer);
    blurTimer = setTimeout(() => {
      if (!editing && dirty) {
        dirty = false;
        shownHz = null;
      }
    }, 400);
  }

  const locked = $derived(!isAdmin || !radio?.can_tune || busy);
  const gainValue = $derived(radio ? (radio.settings.gain_db === null ? 'auto' : String(radio.settings.gain_db)) : 'auto');
  const gainOptions = $derived.by(() => {
    if (!radio) return [] as number[];
    const g = [...radio.gains_db];
    const cur = radio.settings.gain_db;
    if (cur !== null && !g.includes(cur)) g.push(cur);
    return g.sort((a, b) => a - b);
  });

  async function apply(patch: RadioPatch) {
    busy = true;
    err = null;
    try {
      await api.configure(patch);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function submitFreq(e: SubmitEvent) {
    e.preventDefault();
    const hz = parseFrequency(freqInput);
    if (hz === null) {
      err = 'Enter a frequency like 145.8, 433.92M or 437550k';
      return;
    }
    editing = false;
    dirty = false;
    shownHz = hz;
    freqInput = (hz / 1e6).toFixed(4);
    apply({ center_hz: hz });
  }

  function nudge(deltaHz: number) {
    if (radio) apply({ center_hz: radio.settings.center_hz + deltaHz });
  }

  function onGain(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    apply(v === 'auto' ? { auto_gain: true } : { gain_db: Number(v) });
  }
</script>

<section class="panel">
  <h2>Receiver</h2>

  {#if !radio}
    <p class="muted">Connecting…</p>
  {:else}
    <div class="status">
      <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
      <div>
        <div>{radio.device ?? 'No device'}</div>
        <div class="muted small-text">
          {radio.source}{radio.tuner ? ` · ${radio.tuner}` : ''} · {formatRate(radio.settings.sample_rate)}
        </div>
        {#if radio.ranges.length}
          <div class="muted small-text">Covers {describeRanges(radio.ranges)}</div>
        {/if}
      </div>
    </div>
    {#if radio.can_select && devices.length > 1}
      <div class="device">
        <label for="sdr">Device</label>
        <select id="sdr" value={radio.device_id ?? ''} disabled={!isAdmin || busy} onchange={selectDevice}>
          {#if !devices.some((d) => d.id === radio.device_id)}
            <option value={radio.device_id ?? ''}>{radio.device ?? radio.device_id ?? 'None'}</option>
          {/if}
          {#each GROUPS as g (g.source)}
            {@const list = devices.filter((d) => d.source === g.source)}
            {#if list.length}
              <optgroup label={g.label}>
                {#each list as d (d.id)}
                  <option value={d.id}>{d.name}{d.tuner ? ` (${d.tuner})` : ''}</option>
                {/each}
              </optgroup>
            {/if}
          {/each}
        </select>
      </div>
    {/if}
    {#if radio.error}
      <p class="error">{radio.error}</p>
    {/if}

    <form class="freq" onsubmit={submitFreq}>
      <label for="freq">Centre frequency (MHz)</label>
      <div class="row">
        <input
          id="freq"
          inputmode="decimal"
          autocomplete="off"
          bind:value={freqInput}
          oninput={() => (dirty = true)}
          onfocus={() => (editing = true)}
          onblur={leaveFreq}
          disabled={locked}
        />
        <button type="submit" disabled={locked}>Tune</button>
      </div>
      <div class="row nudges">
        <button type="button" class="small" disabled={locked} onclick={() => nudge(-1e6)}>−1M</button>
        <button type="button" class="small" disabled={locked} onclick={() => nudge(-100e3)}>−100k</button>
        <button type="button" class="small" disabled={locked} onclick={() => nudge(100e3)}>+100k</button>
        <button type="button" class="small" disabled={locked} onclick={() => nudge(1e6)}>+1M</button>
      </div>
    </form>

    <div class="presets">
      {#each PRESETS as p (p.label)}
        {@const why = unsupported(radio, p.hz)}
        <button
          type="button"
          class="small preset"
          class:active={radio.settings.center_hz === p.hz}
          class:na={!!why}
          disabled={locked || !!why}
          onclick={() => apply({ center_hz: p.hz, ...(p.rate ? { sample_rate: p.rate } : {}) })}
          title={why ? `${formatMHz(p.hz)}: ${why}` : formatMHz(p.hz)}>{p.label}</button
        >
      {/each}
    </div>

    <div class="rotation-box">
      <RotationToggle {isAdmin} />
    </div>

    <div class="grid">
      <label for="rate">Sample rate</label>
      <select
        id="rate"
        disabled={locked}
        value={radio.settings.sample_rate}
        onchange={(e) => apply({ sample_rate: Number((e.currentTarget as HTMLSelectElement).value) })}
      >
        {#each radio.sample_rates as r (r)}
          <option value={r}>{formatRate(r)}</option>
        {/each}
      </select>

      <label for="gain">Gain</label>
      <select id="gain" disabled={locked} value={gainValue} onchange={onGain}>
        <option value="auto">Auto (AGC)</option>
        {#each gainOptions as g (g)}
          <option value={String(g)}>{g.toFixed(1)} dB</option>
        {/each}
      </select>

      <label for="ppm">PPM correction</label>
      <input
        id="ppm"
        type="number"
        min="-200"
        max="200"
        step="1"
        disabled={locked}
        value={radio.settings.ppm}
        onchange={(e) => apply({ ppm: Number((e.currentTarget as HTMLInputElement).value) })}
      />

      <label for="ds">Direct sampling</label>
      <select
        id="ds"
        disabled={locked}
        value={radio.settings.direct_sampling}
        onchange={(e) => apply({ direct_sampling: Number((e.currentTarget as HTMLSelectElement).value) })}
      >
        <option value={0}>Off</option>
        <option value={1}>I branch</option>
        <option value={2}>Q branch (HF)</option>
      </select>

      <label for="bias">Bias-T (powers LNA)</label>
      <input
        id="bias"
        type="checkbox"
        disabled={locked}
        checked={radio.settings.bias_tee}
        onchange={(e) => apply({ bias_tee: (e.currentTarget as HTMLInputElement).checked })}
      />
    </div>

    {#if err}<p class="error">{err}</p>{/if}
    {#if !isAdmin}
      <p class="muted small-text">View only — an admin can change receiver settings.</p>
    {:else if !radio.can_tune}
      <p class="muted small-text">The {radio.source} source can't be retuned.</p>
    {:else}
      <p class="muted small-text">Tip: double-click the spectrum to centre on a signal.</p>
    {/if}
  {/if}
</section>

<style>
  .status {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-bottom: 12px;
  }
  .device {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 12px;
    align-items: center;
    margin-bottom: 12px;
    font-size: 14px;
  }
  .device label {
    color: var(--muted);
  }
  .device select {
    min-width: 0;
  }
  .preset.na {
    border-style: dashed;
    text-decoration: line-through;
  }
  .freq {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 12px;
  }
  .freq label {
    color: var(--muted);
    font-size: 13px;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
    min-width: 0;
    font-size: 18px;
    font-variant-numeric: tabular-nums;
  }
  .nudges button {
    flex: 1;
  }
  .rotation-box {
    margin: 12px 0 4px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
  .preset.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 12px;
    align-items: center;
    font-size: 14px;
  }
  .grid label {
    color: var(--muted);
  }
  .grid input[type='checkbox'] {
    justify-self: start;
    width: 18px;
    height: 18px;
  }
</style>
