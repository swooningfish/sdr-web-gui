<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type AdsbDevice, type AdsbInfo } from '../lib/api';

  let info = $state<AdsbInfo | null>(null);
  let devices = $state<AdsbDevice[]>([]);
  let gain = $state('auto');
  let ppm = $state(0);
  let bias = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  // R820T/R828D steps, offered before a dongle has reported its own.
  const COMMON_GAINS = [0, 9.7, 19.7, 29.7, 37.2, 40.2, 42.1, 43.4, 44.5, 48, 49.6];

  async function load(fillForm: boolean) {
    try {
      const [i, d] = await Promise.all([api.adsb(), api.adsbDevices()]);
      info = i;
      devices = d.devices;
      if (fillForm) {
        const r = i.receiver_settings;
        gain = r.gain_db === null ? 'auto' : String(r.gain_db);
        ppm = r.ppm;
        bias = r.bias_tee;
      }
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    load(true);
    const t = setInterval(() => load(false), 4000);
    return () => clearInterval(t);
  });

  const receiver = $derived(info?.status.receiver ?? null);
  const selectedId = $derived(receiver?.device_id ?? 'off');
  const gains = $derived(receiver?.gains_db.length ? receiver.gains_db : COMMON_GAINS);

  async function act(f: () => Promise<unknown>) {
    busy = true;
    err = null;
    saved = false;
    try {
      await f();
      await load(false);
      saved = true;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const choose = (id: string) => act(() => api.selectAdsbDevice(id));
  const save = (e: SubmitEvent) => {
    e.preventDefault();
    act(() => api.setAdsbReceiver({ gain_db: gain === 'auto' ? null : Number(gain), ppm: Number(ppm) || 0, bias_tee: bias }));
  };
  const setEnabled = (on: boolean) => act(() => api.setAdsb({ enabled: on }));
  const download = () => act(() => api.downloadAircraftDb());

  function why(d: AdsbDevice): string | null {
    if (d.used_by) return `used for ${d.used_by}`;
    if (!d.covers_1090) return "can't receive 1090 MHz";
    return null;
  }
</script>

<section class="panel">
  <h2>ADS-B receiver</h2>
  <p class="muted small-text">
    A second dongle with a 1090 MHz antenna can listen for aircraft all the time, alongside the main receiver. Without
    one, aircraft are decoded whenever the main receiver is on 1090 MHz (for example as a rotation profile), and drop off
    while it's tuned elsewhere.
  </p>

  {#if info}
    <label class="check">
      <input type="checkbox" checked={info.config.enabled} disabled={busy} onchange={(e) => setEnabled(e.currentTarget.checked)} />
      Decode ADS-B
    </label>

    <div class="grid">
      <label for="adsb-dev">Receiver</label>
      <select id="adsb-dev" value={selectedId} disabled={busy} onchange={(e) => choose(e.currentTarget.value)}>
        <option value="off">None: use the main receiver when it's on 1090 MHz</option>
        {#each devices as d (d.id)}
          {@const w = why(d)}
          <option value={d.id} disabled={!!w && d.id !== selectedId}>{d.name}{d.tuner ? ` (${d.tuner})` : ''}{w ? ` · ${w}` : ''}</option>
        {/each}
      </select>
    </div>
    {#if receiver && selectedId !== 'off'}
      <p class="small-text status">
        <span class="dot" class:ok={receiver.connected} class:bad={!receiver.connected}></span>
        {receiver.connected ? `${receiver.device ?? selectedId} on 1090 MHz, ${info.status.msg_rate.toFixed(0)} msg/s` : (receiver.error ?? 'Connecting…')}
      </p>
    {/if}

    <form class="grid" onsubmit={save}>
      <label for="adsb-gain">Gain</label>
      <select id="adsb-gain" bind:value={gain} disabled={busy}>
        <option value="auto">Auto (tuner AGC)</option>
        {#each gains as g (g)}
          <option value={String(g)}>{g.toFixed(1)} dB</option>
        {/each}
      </select>
      <label for="adsb-ppm">PPM correction</label>
      <input id="adsb-ppm" type="number" min="-200" max="200" step="1" bind:value={ppm} disabled={busy} />
      <label for="adsb-bias">Bias-T (powers an LNA or filter)</label>
      <input id="adsb-bias" type="checkbox" bind:checked={bias} disabled={busy} />
      <span></span>
      <div><button type="submit" class="small primary" disabled={busy}>Save</button></div>
    </form>
    <p class="muted small-text">
      Weak aircraft need gain: the default is the maximum. Lower it if aircraft close by are missed (overload).
    </p>

    <h3>Aircraft database</h3>
    <p class="small-text">
      {#if info.status.db.downloading}
        Downloading and importing… (about a minute)
      {:else if info.status.db.rows}
        {info.status.db.rows.toLocaleString('en-GB')} aircraft, updated {new Date(info.status.db.updated_ms ?? 0).toLocaleDateString()}.
      {:else}
        Not downloaded. Registrations and types will show once it is.
      {/if}
    </p>
    {#if info.status.db.error}<p class="error small-text">{info.status.db.error}</p>{/if}
    <button type="button" class="small" disabled={busy || info.status.db.downloading} onclick={download}>
      {info.status.db.rows ? 'Update the database' : 'Download the database'}
    </button>
    <p class="muted small-text">
      Registration, type and description by ICAO address, from the community database maintained by Mictronics
      (via <span class="mono">tar1090-db</span>), about 9 MB from GitHub.
    </p>
  {/if}
  {#if err}<p class="error">{err}</p>{/if}
  {#if saved && !err}<p class="muted small-text" role="status">Saved.</p>{/if}
</section>

<style>
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    margin: 12px 0 4px;
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
  .check {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h3 {
    margin: 18px 0 6px;
    font-size: 15px;
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
</style>
