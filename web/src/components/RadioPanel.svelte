<script lang="ts">
  import { api, type RadioPatch, type RadioState } from '../lib/api';
  import { formatMHz, formatRate, parseFrequency, PRESETS } from '../lib/format';

  let { radio, isAdmin }: { radio: RadioState | null; isAdmin: boolean } = $props();

  let freqInput = $state('');
  let editing = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);

  $effect(() => {
    if (radio && !editing) freqInput = (radio.settings.center_hz / 1e6).toFixed(4);
  });

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
      </div>
    </div>
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
          onfocus={() => (editing = true)}
          onblur={() => (editing = false)}
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
        <button
          type="button"
          class="small preset"
          class:active={radio.settings.center_hz === p.hz}
          disabled={locked}
          onclick={() => apply({ center_hz: p.hz, ...(p.rate ? { sample_rate: p.rate } : {}) })}
          title={formatMHz(p.hz)}>{p.label}</button
        >
      {/each}
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
