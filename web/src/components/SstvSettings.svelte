<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Demod, type RadioState, type SstvChannel, type SstvInfo } from '../lib/api';
  import { unsupported } from '../lib/coverage';
  import { formatMHz, parseFrequency } from '../lib/format';

  let { radio }: { radio: RadioState | null } = $props();

  let info = $state<SstvInfo | null>(null);
  let enabled = $state(true);
  let channels = $state<(Omit<SstvChannel, 'frequency_hz'> & { freq: string })[]>([]);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  function fill(i: SstvInfo) {
    info = i;
    enabled = i.config.enabled;
    channels = i.config.channels.map((c) => ({
      label: c.label,
      demod: c.demod,
      enabled: c.enabled,
      doppler: c.doppler,
      freq: (c.frequency_hz / 1e6).toFixed(3),
    }));
  }

  onMount(() => {
    api.sstv().then(fill, (e) => (err = e instanceof Error ? e.message : String(e)));
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    err = null;
    saved = false;
    const out: SstvChannel[] = [];
    for (const c of channels) {
      const hz = parseFrequency(c.freq);
      if (hz === null) {
        err = `"${c.freq}" is not a frequency`;
        return;
      }
      out.push({
        label: c.label.trim() || formatMHz(hz, 3),
        frequency_hz: hz,
        demod: c.demod,
        enabled: c.enabled,
        doppler: c.doppler,
      });
    }
    busy = true;
    try {
      const r = await api.setSstv({ enabled, channels: out });
      if (info) fill({ ...info, config: r.config });
      saved = true;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const DEMODS: { value: Demod; label: string }[] = [
    { value: 'fm', label: 'FM' },
    { value: 'usb', label: 'USB' },
    { value: 'lsb', label: 'LSB' },
  ];
</script>

<section class="panel">
  <h2>SSTV decoder</h2>
  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else}
    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label>
      <p class="muted small-text hint">
        Robot 36/72, Martin M1/M2, Scottie S1/S2/DX and PD 50–240 are detected automatically from the VIS code. Use FM
        for the ISS and VHF/UHF; USB for HF (e.g. 14.230 MHz with direct sampling). Up to 4 channels.
      </p>
      {#each channels as c, i (i)}
        {@const why = unsupported(radio, parseFrequency(c.freq) ?? 0)}
        <div class="row" class:na-row={!!why}>
          <input type="checkbox" bind:checked={c.enabled} aria-label="Enable channel {i + 1}" />
          <input class="label" bind:value={c.label} maxlength="24" placeholder="Label" aria-label="Channel {i + 1} label" />
          <input class="freq" bind:value={c.freq} inputmode="decimal" aria-label="Channel {i + 1} frequency in MHz" />
          <select bind:value={c.demod} aria-label="Channel {i + 1} demodulation">
            {#each DEMODS as d (d.value)}<option value={d.value}>{d.label}</option>{/each}
          </select>
          {#if why}<span class="na-tag" title={why}>Not supported by this SDR</span>{/if}
          <label class="dop" title="Satellite downlink: follow the Doppler shift during passes"
            ><input type="checkbox" bind:checked={c.doppler} /> Doppler</label
          >
          <button type="button" class="small danger" onclick={() => channels.splice(i, 1)} aria-label="Remove channel {i + 1}"
            >✕</button
          >
        </div>
      {/each}
      <button
        type="button"
        class="small"
        disabled={channels.length >= 4}
        onclick={() => channels.push({ label: '', freq: '145.800', demod: 'fm', enabled: true, doppler: true })}>Add channel</button
      >
      <div class="save"><button type="submit" class="primary" disabled={busy}>Save</button></div>
      {#if err}<p class="error">{err}</p>{/if}
      {#if saved}<p class="success">Saved</p>{/if}
    </form>
  {/if}
</section>

<style>
  form {
    display: grid;
    gap: 6px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  .hint {
    margin: 0 0 4px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .row .label {
    flex: 1 1 8em;
    min-width: 0;
  }
  .row .freq {
    width: 6.5em;
    flex: none;
  }
  .dop {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 13px;
    flex: none;
  }
  .row select {
    flex: none;
    padding-left: 6px;
    padding-right: 6px;
  }
  form > button.small {
    justify-self: start;
  }
  .save {
    margin-top: 10px;
  }
</style>
