<script lang="ts">
  import { api, type ExtraReceiver, type ExtraReceiverDevice } from '../lib/api';
  import { formatMHz } from '../lib/format';

  let { rx, onchange }: { rx: ExtraReceiver; onchange: () => Promise<void> } = $props();

  const HELP: Record<ExtraReceiver['use'], string> = {
    mesh: 'Kept near 869.575 MHz for Meshtastic and MeshCore.',
    sensors: 'Kept on the rtl_433 frequency (Settings → Decoders) for weather stations, sensors and TPMS.',
    aprs: 'Kept on the APRS channels (Settings → Decoders), including the ISS downlink.',
  };
  // R820T/R828D steps, offered before a dongle has reported its own.
  const COMMON_GAINS = [0, 9.7, 19.7, 29.7, 37.2, 40.2, 42.1, 43.4, 44.5, 48, 49.6];

  let devices = $state<ExtraReceiverDevice[]>([]);
  let gain = $state('auto');
  let ppm = $state(0);
  let bias = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  // Fill the form once; later polls must not overwrite what's being typed.
  let filled = false;
  $effect(() => {
    if (filled) return;
    filled = true;
    gain = rx.settings.gain_db === null ? 'auto' : String(rx.settings.gain_db);
    ppm = rx.settings.ppm;
    bias = rx.settings.bias_tee;
  });

  async function loadDevices() {
    try {
      devices = (await api.receiverDevices(rx.use)).devices;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  // Reload the list when this or another receiver changes device.
  $effect(() => {
    void rx.selected;
    void rx.state.connected;
    loadDevices();
  });

  const selectedId = $derived(rx.selected ?? 'off');
  const gains = $derived(rx.state.gains_db.length ? rx.state.gains_db : COMMON_GAINS);

  async function act(f: () => Promise<unknown>) {
    busy = true;
    err = null;
    saved = false;
    try {
      await f();
      await onchange();
      await loadDevices();
      saved = true;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const choose = (id: string) => act(() => api.selectReceiver(rx.use, id));
  const save = (e: SubmitEvent) => {
    e.preventDefault();
    act(() =>
      api.setReceiver(rx.use, { gain_db: gain === 'auto' ? null : Number(gain), ppm: Number(ppm) || 0, bias_tee: bias }),
    );
  };

  function why(d: ExtraReceiverDevice): string | null {
    if (d.used_by) return `used for ${d.used_by}`;
    if (!d.covers) return `can't receive ${formatMHz(rx.tuning.center_hz, 1)}`;
    return null;
  }
</script>

<div class="rx">
  <h3>{rx.label}</h3>
  <p class="muted small-text">{HELP[rx.use]}</p>
  <div class="grid">
    <label for="rx-{rx.use}-dev">Receiver</label>
    <select id="rx-{rx.use}-dev" value={selectedId} disabled={busy} onchange={(e) => choose(e.currentTarget.value)}>
      <option value="off">None: use the main receiver when it's on the band</option>
      {#each devices as d (d.id)}
        {@const w = why(d)}
        <option value={d.id} disabled={!!w && d.id !== selectedId}
          >{d.name}{d.tuner ? ` (${d.tuner})` : ''}{w ? ` · ${w}` : ''}</option
        >
      {/each}
    </select>
  </div>

  {#if selectedId !== 'off'}
    <p class="small-text status">
      <span class="dot" class:ok={rx.dedicated} class:bad={!rx.dedicated}></span>
      {#if rx.dedicated}
        {rx.state.device ?? selectedId} on {formatMHz(rx.state.settings.center_hz, 3)}
      {:else}
        {rx.state.error ?? 'Connecting…'} Using the main receiver meanwhile.
      {/if}
    </p>
    <form class="grid" onsubmit={save}>
      <label for="rx-{rx.use}-gain">Gain</label>
      <select id="rx-{rx.use}-gain" bind:value={gain} disabled={busy}>
        <option value="auto">Auto (tuner AGC)</option>
        {#each gains as g (g)}
          <option value={String(g)}>{g.toFixed(1)} dB</option>
        {/each}
      </select>
      <label for="rx-{rx.use}-ppm">PPM correction</label>
      <input id="rx-{rx.use}-ppm" type="number" min="-200" max="200" step="1" bind:value={ppm} disabled={busy} />
      <label for="rx-{rx.use}-bias">Bias-T (powers an LNA or filter)</label>
      <input id="rx-{rx.use}-bias" type="checkbox" bind:checked={bias} disabled={busy} />
      <span></span>
      <div><button type="submit" class="small primary" disabled={busy}>Save</button></div>
    </form>
  {/if}
  {#if err}<p class="error small-text">{err}</p>{/if}
  {#if saved && !err}<p class="muted small-text" role="status">Saved.</p>{/if}
</div>

<style>
  .rx + :global(.rx) {
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }
  h3 {
    margin: 14px 0 2px;
    font-size: 15px;
  }
  p {
    margin: 2px 0;
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    margin: 8px 0 4px;
    font-size: 14px;
    max-width: 640px;
  }
  .grid label {
    color: var(--muted);
  }
  .grid select,
  .grid input[type='number'] {
    min-width: 0;
  }
  .grid input[type='checkbox'] {
    justify-self: start;
    width: 18px;
    height: 18px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 8px;
  }
</style>
