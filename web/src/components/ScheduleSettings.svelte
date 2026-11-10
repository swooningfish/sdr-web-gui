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
  let satFreq = $state<string[]>([]);
  let satRec = $state<string[]>([]);
  let tleText = $state('');
  let testSecs = $state(60);
  let msg = $state<{ ok: boolean; text: string } | null>(null);
  let busy = $state(false);
  const testWhy = $derived(info?.config.satellites[0] ? unsupported(radio, info.config.satellites[0].center_hz) : null);

  const mhz = (hz: number) => (hz / 1e6).toFixed(3);

  function fill(i: ScheduleInfo) {
    info = i;
    cfg = structuredClone(i.config);
    satFreq = i.config.satellites.map((s) => mhz(s.center_hz));
    satRec = i.config.satellites.map((s) => mhz(s.record_hz));
  }

  onMount(() => {
    api.schedule().then(fill, (e) => (msg = { ok: false, text: String(e.message ?? e) }));
    // Keep pass/TLE status current (e.g. so Stop appears while a test runs).
    const t = setInterval(async () => {
      try {
        const i = await api.schedule();
        if (info) info = { ...info, status: i.status };
      } catch {
        /* keep last */
      }
    }, 3000);
    return () => clearInterval(t);
  });

  const active = $derived(info?.status.active ?? null);

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

  function saveSchedule(e: SubmitEvent) {
    e.preventDefault();
    if (!cfg) return;
    const out = structuredClone($state.snapshot(cfg)) as ScheduleConfig;
    for (const [i, s] of out.satellites.entries()) {
      const hz = parseFrequency(satFreq[i]);
      const rec = parseFrequency(satRec[i]);
      if (hz === null || rec === null) return void (msg = { ok: false, text: 'Check the satellite frequencies' });
      s.center_hz = hz;
      s.record_hz = rec;
    }
    act(async () => {
      // Rotation lives in its own settings tab (and is switched from the
      // receiver and Passes page): keep whatever the server has now.
      const cur = (await api.schedule()).config;
      out.rotation = cur.rotation;
      out.profiles = cur.profiles;
      fill({ ...info!, config: (await api.setSchedule(out)).config });
    }, 'Schedule saved');
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
  <h2>Satellites &amp; schedule</h2>
  {#if !cfg || !info}
    <p class="muted">{msg?.text ?? 'Loading…'}</p>
  {:else}
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

      <h3>Auto-switch for passes</h3>
      <label class="check"
        ><input type="checkbox" bind:checked={cfg.auto_switch} /> Tune to each pass automatically and switch everyone's page to
        Passes</label
      >
      <p class="muted small-text hint">
        A notice appears under the receiver beforehand. Admins can choose <strong>Stay</strong> on it to keep the current
        tuning for that pass; if nobody does, it switches on its own. Off: passes are predicted and shown, but the radio
        stays where it is (and nothing is recorded).
      </p>
      <div class="row">
        <label
          >Warn <input class="n" type="number" bind:value={cfg.notice_s} min="10" max="1800" disabled={!cfg.auto_switch} /> s before
          tuning</label
        >
        <label>Tune <input class="n" type="number" bind:value={cfg.lead_s} min="0" max="300" /> s before the satellite rises</label>
      </div>
      <h3>ARISS SSTV events</h3>
      <label class="check"
        ><input type="checkbox" bind:checked={cfg.ariss_auto} /> During an ARISS SSTV event, receive ISS passes on the event's
        frequency</label
      >
      <p class="muted small-text hint">
        A few times a year the ISS sends SSTV pictures for several days, usually on 437.550 MHz instead of 145.800. The dates
        and frequency are read from ariss.org. While an event is on, ISS passes are tuned 550 kHz below the event frequency
        (2.048 MS/s) and recorded on it; afterwards the settings above apply again. For pictures to be decoded, the frequency
        must also be a channel under Settings → Decoders → SSTV.
      </p>
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
    {#if active}
      <div class="row">
        <span class="small-text">
          {active.test ? 'Test pass' : `${active.satellite} pass`} in progress{active.recording ? ', recording' : ''}.
        </span>
        <button type="button" class="small danger" disabled={busy} onclick={() => act(() => api.stopPass(), 'Pass stopped')}
          >Stop</button
        >
      </div>
    {/if}
    <div class="row" hidden={!!active}>
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
