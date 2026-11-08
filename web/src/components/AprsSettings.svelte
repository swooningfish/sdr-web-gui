<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type AprsChannel, type AprsInfo } from '../lib/api';
  import { formatMHz, parseFrequency } from '../lib/format';

  let info = $state<AprsInfo | null>(null);
  let enabled = $state(true);
  let channels = $state<(Omit<AprsChannel, 'frequency_hz'> & { freq: string })[]>([]);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  function fill(i: AprsInfo) {
    info = i;
    enabled = i.config.enabled;
    channels = i.config.channels.map((c) => ({
      label: c.label,
      enabled: c.enabled,
      doppler: c.doppler,
      freq: (c.frequency_hz / 1e6).toFixed(3),
    }));
  }

  onMount(() => {
    api.aprs().then(fill, (e) => (err = e instanceof Error ? e.message : String(e)));
    const t = setInterval(async () => {
      try {
        const i = await api.aprs();
        if (info) info = { ...info, status: i.status };
      } catch {
        /* ignore */
      }
    }, 5000);
    return () => clearInterval(t);
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    err = null;
    saved = false;
    const out: AprsChannel[] = [];
    for (const c of channels) {
      const hz = parseFrequency(c.freq);
      if (hz === null) {
        err = `"${c.freq}" is not a frequency`;
        return;
      }
      out.push({ label: c.label.trim() || formatMHz(hz, 3), frequency_hz: hz, enabled: c.enabled, doppler: c.doppler });
    }
    busy = true;
    try {
      const r = await api.setAprs({ enabled, channels: out });
      if (info) fill({ ...info, config: r.config });
      saved = true;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>APRS decoder (Direwolf)</h2>
  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else}
    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label>
      <p class="muted small-text hint">
        Each channel is decoded whenever its frequency is inside the tuned band (the VHF block covers 144.800 and
        145.825 MHz together). Up to 4 channels.
      </p>
      {#each channels as c, i (i)}
        <div class="row">
          <input type="checkbox" bind:checked={c.enabled} aria-label="Enable channel {i + 1}" />
          <input class="label" bind:value={c.label} maxlength="24" placeholder="Label" aria-label="Channel {i + 1} label" />
          <input class="freq" bind:value={c.freq} inputmode="decimal" aria-label="Channel {i + 1} frequency in MHz" />
          <span class="muted small-text">MHz</span>
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
        onclick={() => channels.push({ label: '', freq: '144.800', enabled: true, doppler: false })}>Add channel</button
      >
      <div class="save"><button type="submit" class="primary" disabled={busy}>Save and restart Direwolf</button></div>
      {#if err}<p class="error">{err}</p>{/if}
      {#if saved}<p class="success">Saved</p>{/if}
    </form>

    <h3>Status</h3>
    {#each info.status.channels as c, i (i)}
      <p class="small-text status">
        <span class="dot" class:ok={c.running && c.in_band} class:bad={!!c.error}></span>
        {c.label} ({formatMHz(c.frequency_hz, 3)}):
        {#if !c.enabled}off{:else if c.error}{c.error}{:else if !c.in_band}idle, outside the tuned band{:else if c.running}decoding,
          {c.packets} packets{:else}starting{/if}
      </p>
    {/each}
    {#if info.status.log.length}
      <pre class="log">{info.status.log.join('\n')}</pre>
    {/if}
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
  .dop {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 13px;
    flex: none;
  }
  .row .freq {
    width: 6.5em;
    flex: none;
  }
  form > button.small {
    justify-self: start;
  }
  .save {
    margin-top: 10px;
  }
  h3 {
    margin: 18px 0 6px;
    font-size: 15px;
  }
  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 4px 0;
  }
  .log {
    max-height: 200px;
    overflow: auto;
    margin: 6px 0 0;
    padding: 8px;
    background: var(--surface-2);
    border-radius: 6px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
  }
</style>
