<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type RadioState, type ScheduleConfig, type ScheduleInfo } from '../lib/api';
  import { unsupported } from '../lib/coverage';
  import { formatRate, parseFrequency, relTime } from '../lib/format';

  let { radio }: { radio: RadioState | null } = $props();
  const why = (mhzText: string | undefined) => unsupported(radio, parseFrequency(mhzText ?? '') ?? 0);

  const RATES = [250_000, 1_024_000, 1_536_000, 1_800_000, 1_920_000, 2_048_000, 2_400_000];

  let info = $state<ScheduleInfo | null>(null);
  let cfg = $state<ScheduleConfig | null>(null);
  // Frequencies are edited as MHz text.
  let profFreq = $state<string[]>([]);
  let satFreq = $state<string[]>([]);
  let satRec = $state<string[]>([]);
  let lat = $state('');
  let lon = $state('');
  let alt = $state('0');
  let tleText = $state('');
  let testSecs = $state(60);
  let msg = $state<{ ok: boolean; text: string } | null>(null);
  let busy = $state(false);
  const testWhy = $derived(info?.config.satellites[0] ? unsupported(radio, info.config.satellites[0].center_hz) : null);

  const mhz = (hz: number) => (hz / 1e6).toFixed(3);

  function fill(i: ScheduleInfo) {
    info = i;
    cfg = structuredClone(i.config);
    profFreq = i.config.profiles.map((p) => mhz(p.center_hz));
    satFreq = i.config.satellites.map((s) => mhz(s.center_hz));
    satRec = i.config.satellites.map((s) => mhz(s.record_hz));
    const l = i.status.location;
    if (l) {
      lat = String(l.lat_deg);
      lon = String(l.lon_deg);
      alt = String(l.alt_m);
    }
  }

  onMount(() => {
    api.schedule().then(fill, (e) => (msg = { ok: false, text: String(e.message ?? e) }));
  });

  async function act(f: () => Promise<unknown>, ok: string) {
    busy = true;
    msg = null;
    try {
      await f();
      msg = { ok: true, text: ok };
      const i = await api.schedule();
      info = i;
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }

  function saveLocation(e: SubmitEvent) {
    e.preventDefault();
    const obs = { lat_deg: Number(lat), lon_deg: Number(lon), alt_m: Number(alt || 0) };
    if (![obs.lat_deg, obs.lon_deg, obs.alt_m].every(Number.isFinite) || lat === '' || lon === '') {
      msg = { ok: false, text: 'Enter latitude and longitude in decimal degrees' };
      return;
    }
    act(() => api.setLocation(obs), 'Location saved; passes recalculated');
  }

  function useDevice() {
    if (!navigator.geolocation) {
      msg = { ok: false, text: 'This browser has no location service' };
      return;
    }
    navigator.geolocation.getCurrentPosition(
      (p) => {
        lat = p.coords.latitude.toFixed(4);
        lon = p.coords.longitude.toFixed(4);
        if (p.coords.altitude !== null) alt = String(Math.round(p.coords.altitude));
        msg = { ok: true, text: 'Filled in from this device; press Save location' };
      },
      (e) => (msg = { ok: false, text: `Couldn't get location: ${e.message}` }),
      { enableHighAccuracy: false, timeout: 10_000 },
    );
  }

  function saveSchedule(e: SubmitEvent) {
    e.preventDefault();
    if (!cfg) return;
    const out = structuredClone($state.snapshot(cfg)) as ScheduleConfig;
    for (const [i, p] of out.profiles.entries()) {
      const hz = parseFrequency(profFreq[i]);
      if (hz === null) return void (msg = { ok: false, text: `"${profFreq[i]}" is not a frequency` });
      p.center_hz = hz;
    }
    for (const [i, s] of out.satellites.entries()) {
      const hz = parseFrequency(satFreq[i]);
      const rec = parseFrequency(satRec[i]);
      if (hz === null || rec === null) return void (msg = { ok: false, text: 'Check the satellite frequencies' });
      s.center_hz = hz;
      s.record_hz = rec;
    }
    act(async () => fill({ ...info!, config: (await api.setSchedule(out)).config }), 'Schedule saved');
  }

  function addProfile() {
    cfg!.profiles.push({ name: 'Profile', center_hz: 145_300_000, sample_rate: 2_048_000, dwell_s: 60 });
    profFreq.push('145.300');
  }

  function addSatellite() {
    cfg!.satellites.push({
      name: 'Satellite',
      norad_id: 25544,
      enabled: true,
      min_elevation_deg: 10,
      center_hz: 145_300_000,
      sample_rate: 2_048_000,
      record: true,
      record_hz: 145_800_000,
    });
    satFreq.push('145.300');
    satRec.push('145.800');
  }

  function saveTle(e: SubmitEvent) {
    e.preventDefault();
    const lines = tleText.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
    const i1 = lines.findIndex((l) => l.startsWith('1 '));
    if (i1 < 0 || !lines[i1 + 1]?.startsWith('2 ')) {
      msg = { ok: false, text: 'Paste the two TLE lines (starting "1 " and "2 "), optionally with a name line above' };
      return;
    }
    const name = i1 > 0 ? lines[i1 - 1] : 'Satellite';
    act(() => api.setTle(name, lines[i1], lines[i1 + 1]), 'TLE saved');
  }
</script>

<section class="panel">
  <h2>Passes &amp; schedule</h2>
  {#if !cfg || !info}
    <p class="muted">{msg?.text ?? 'Loading…'}</p>
  {:else}
    <h3>Station location</h3>
    <form class="loc" onsubmit={saveLocation}>
      <label>Latitude <input bind:value={lat} inputmode="decimal" placeholder="51.5074" /></label>
      <label>Longitude <input bind:value={lon} inputmode="decimal" placeholder="-0.1278" /></label>
      <label>Altitude (m) <input bind:value={alt} inputmode="decimal" /></label>
      <div class="buttons">
        <button type="button" class="small" onclick={useDevice}>Use this device's location</button>
        <button type="submit" class="small primary" disabled={busy}>Save location</button>
      </div>
    </form>

    <form onsubmit={saveSchedule}>
      <h3>Satellites</h3>
      <p class="muted small-text hint">
        During a pass the radio is tuned to the satellite's centre frequency from {cfg.lead_s} s before it rises, and
        its downlink is recorded. Channels marked "Doppler" in the APRS and SSTV settings follow the Doppler shift.
      </p>
      {#each cfg.satellites as s, i (i)}
        {@const na = why(satFreq[i])}
        <fieldset class:na-row={!!na}>
          <div class="row">
            <label class="check"><input type="checkbox" bind:checked={s.enabled} /> On</label>
            <input class="grow" bind:value={s.name} maxlength="32" aria-label="Satellite name" />
            <label>NORAD <input class="id" type="number" bind:value={s.norad_id} min="1" max="999999" /></label>
            <button type="button" class="small danger" aria-label="Remove {s.name}" onclick={() => {
              cfg!.satellites.splice(i, 1);
              satFreq.splice(i, 1);
              satRec.splice(i, 1);
            }}>✕</button>
          </div>
          <div class="row">
            <label>Min elevation <input class="n" type="number" bind:value={s.min_elevation_deg} min="0" max="90" />°</label>
            <label>Tune <input class="f" bind:value={satFreq[i]} inputmode="decimal" /> MHz</label>
            <select bind:value={s.sample_rate} aria-label="Sample rate during the pass">
              {#each RATES as r (r)}<option value={r}>{formatRate(r)}</option>{/each}
            </select>
          </div>
          {#if na}
            <p class="small-text hint"><span class="na-tag">Not supported by this SDR</span> Passes are skipped. {na}</p>
          {/if}
          <div class="row">
            <label class="check"><input type="checkbox" bind:checked={s.record} /> Record</label>
            <label><input class="f" bind:value={satRec[i]} inputmode="decimal" disabled={!s.record} /> MHz FM</label>
          </div>
        </fieldset>
      {/each}
      <button type="button" class="small" disabled={cfg.satellites.length >= 4} onclick={addSatellite}>Add satellite</button>

      <h3>Rotation</h3>
      <label class="check"
        ><input type="checkbox" bind:checked={cfg.rotation} /> Cycle through these profiles when no pass is on</label
      >
      <p class="muted small-text hint">Holds while an SSTV image is being received (up to 5 minutes).</p>
      {#each cfg.profiles as p, i (i)}
        {@const na = why(profFreq[i])}
        <div class="row" class:na-row={!!na}>
          <input class="grow" bind:value={p.name} maxlength="32" aria-label="Profile name" />
          <input class="f" bind:value={profFreq[i]} inputmode="decimal" aria-label="{p.name} centre frequency in MHz" />
          <span class="muted small-text">MHz</span>
          {#if na}<span class="na-tag" title="{na}. Rotation skips this profile.">Not supported</span>{/if}
          <select bind:value={p.sample_rate} aria-label="{p.name} sample rate">
            {#each RATES as r (r)}<option value={r}>{formatRate(r)}</option>{/each}
          </select>
          <input class="n" type="number" bind:value={p.dwell_s} min="10" max="3600" aria-label="{p.name} dwell in seconds" />
          <span class="muted small-text">s</span>
          <button type="button" class="small danger" aria-label="Remove {p.name}" onclick={() => {
            cfg!.profiles.splice(i, 1);
            profFreq.splice(i, 1);
          }}>✕</button>
        </div>
      {/each}
      <button type="button" class="small" disabled={cfg.profiles.length >= 8} onclick={addProfile}>Add profile</button>
      <div class="buttons"><button type="submit" class="primary" disabled={busy}>Save schedule</button></div>
    </form>

    <h3>Orbit data (TLE)</h3>
    <ul class="tles small-text">
      {#each info.status.tles as t (t.norad_id)}
        <li>
          <strong>{t.name}</strong> ({t.norad_id}):
          {#if t.epoch_ms}epoch {relTime(t.epoch_ms)}, {t.source === 'manual' ? 'entered by hand' : 'from CelesTrak'}
            {t.fetched_ms ? relTime(t.fetched_ms) : ''}{:else}none yet{/if}
          {#if t.error}<span class="error-text"> · {t.error}</span>{/if}
        </li>
      {/each}
    </ul>
    <div class="buttons">
      <button type="button" class="small" disabled={busy} onclick={() => act(() => api.refreshTle(), 'Fetching from CelesTrak…')}
        >Refresh from CelesTrak</button
      >
    </div>
    <details>
      <summary class="small-text">Enter a TLE by hand (no internet access)</summary>
      <form onsubmit={saveTle}>
        <textarea bind:value={tleText} rows="3" spellcheck="false" placeholder={'ISS (ZARYA)\n1 25544U …\n2 25544 …'}></textarea>
        <button type="submit" class="small" disabled={busy}>Save TLE</button>
      </form>
    </details>

    <h3>Test</h3>
    <div class="row">
      <span class="small-text">Run a pretend pass now for</span>
      <input class="n" type="number" bind:value={testSecs} min="10" max="900" aria-label="Test pass length in seconds" />
      <span class="small-text">s</span>
      <button
        type="button"
        class="small"
        disabled={busy || !!testWhy}
        title={testWhy ?? ''}
        onclick={() => act(() => api.testPass(testSecs), 'Test pass started')}>Start</button
      >
    </div>
    {#if testWhy}<p class="small-text hint"><span class="na-tag">Disabled</span> {info.config.satellites[0].name}: {testWhy}</p>{/if}
    <p class="muted small-text hint">Tunes and records exactly like a real pass (without Doppler), then logs it.</p>

    {#if msg}<p class:error={!msg.ok} class:success={msg.ok}>{msg.text}</p>{/if}
  {/if}
</section>

<style>
  h3 {
    margin: 18px 0 6px;
    font-size: 15px;
  }
  form {
    display: grid;
    gap: 6px;
  }
  .loc {
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    align-items: end;
  }
  .loc label {
    display: grid;
    gap: 2px;
    font-size: 13px;
  }
  .loc input {
    min-width: 0;
  }
  .loc .buttons {
    grid-column: 1 / -1;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 6px;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px;
    margin: 0;
    display: grid;
    gap: 6px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }
  .row label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .grow {
    flex: 1 1 120px;
    min-width: 0;
  }
  .n {
    width: 5.5em;
  }
  .id {
    width: 7.5em;
  }
  .f {
    width: 6.5em;
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
  form > button.small {
    justify-self: start;
  }
  .tles {
    margin: 0;
    padding-left: 18px;
  }
  .error-text {
    color: var(--bad);
  }
  textarea {
    font: 12.5px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px;
    width: 100%;
  }
  details form {
    margin-top: 6px;
    justify-items: start;
  }
</style>
