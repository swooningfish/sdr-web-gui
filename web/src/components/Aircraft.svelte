<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type AdsbInfo, type AdsbRange, type AdsbTrailPoint, type Aircraft, type RadioState } from '../lib/api';
  import {
    CATEGORIES,
    LEGEND_FT,
    aircraftName,
    compass,
    emergency,
    fmtAlt,
    fmtVrate,
    legendGradient,
  } from '../lib/adsb';
  import { unsupported } from '../lib/coverage';
  import { relTime } from '../lib/format';
  import AdsbMap, { type RangeMode, type TrailMode } from './AdsbMap.svelte';

  let {
    isAdmin,
    radio,
    active,
    onsettings,
  }: {
    isAdmin: boolean;
    radio: RadioState | null;
    /** The tab is showing: poll only then. */
    active: boolean;
    onsettings: (tab: 'receiver' | 'passes' | 'rotation') => void;
  } = $props();

  type SortKey = 'name' | 'type' | 'alt' | 'speed' | 'dist' | 'squawk' | 'seen';
  const SORTS: { key: SortKey; label: string; numeric: boolean }[] = [
    { key: 'name', label: 'Callsign', numeric: false },
    { key: 'type', label: 'Type', numeric: false },
    { key: 'alt', label: 'Altitude', numeric: true },
    { key: 'speed', label: 'Speed', numeric: true },
    { key: 'dist', label: 'Distance', numeric: true },
    { key: 'squawk', label: 'Squawk', numeric: false },
    { key: 'seen', label: 'Seen', numeric: true },
  ];
  const PREFS = 'sdr.adsb';

  let info = $state<AdsbInfo | null>(null);
  let list = $state<Aircraft[]>([]);
  let trails = $state.raw(new Map<string, AdsbTrailPoint[]>());
  let range = $state<AdsbRange | null>(null);
  let selected = $state<string | null>(null);
  let query = $state('');
  let sortKey = $state<SortKey>('dist');
  let sortAsc = $state(true);
  let trailMode = $state<TrailMode>('all');
  let labels = $state(true);
  let rangeMode = $state<RangeMode>('rings');
  let follow = $state(false);
  let now = $state(Date.now());
  let since = 0;
  let err = $state<string | null>(null);
  let busy = $state(false);

  // Display choices are remembered per browser.
  try {
    const p = JSON.parse(localStorage.getItem(PREFS) ?? '{}');
    if (['all', 'selected', 'off'].includes(p.trailMode)) trailMode = p.trailMode;
    if (typeof p.labels === 'boolean') labels = p.labels;
    if (['off', 'rings', '24h', 'all'].includes(p.rangeMode)) rangeMode = p.rangeMode;
  } catch {
    /* no storage */
  }
  $effect(() => {
    const p = { trailMode, labels, rangeMode };
    try {
      localStorage.setItem(PREFS, JSON.stringify(p));
    } catch {
      /* no storage */
    }
  });

  /** Heard in the last minute. */
  const live = $derived(list.filter((a) => now - a.seen_ms < 60_000));
  const sel = $derived(live.find((a) => a.icao === selected) ?? list.find((a) => a.icao === selected) ?? null);
  const home = $derived(info?.status.home ?? null);

  function sortValue(a: Aircraft, k: SortKey): string | number | null {
    switch (k) {
      case 'name':
        return aircraftName(a);
      case 'type':
        return a.type_code ?? a.category ?? null;
      case 'alt':
        return a.on_ground ? -1000 : (a.alt_baro_ft ?? a.alt_gnss_ft);
      case 'speed':
        return a.gs_kt;
      case 'dist':
        return a.distance_km;
      case 'squawk':
        return a.squawk;
      case 'seen':
        return now - a.seen_ms;
    }
  }

  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const rows = live.filter(
      (a) =>
        !q ||
        [a.callsign, a.reg, a.icao, a.type_code, a.desc, a.squawk].some((v) => v && v.toLowerCase().includes(q)),
    );
    const dir = sortAsc ? 1 : -1;
    return rows.sort((a, b) => {
      // Emergencies first, whatever the sort.
      const ea = emergency(a) ? 0 : 1;
      const eb = emergency(b) ? 0 : 1;
      if (ea !== eb) return ea - eb;
      const va = sortValue(a, sortKey);
      const vb = sortValue(b, sortKey);
      if (va === null && vb === null) return 0;
      if (va === null) return 1;
      if (vb === null) return -1;
      return (typeof va === 'number' && typeof vb === 'number' ? va - vb : String(va).localeCompare(String(vb))) * dir;
    });
  });

  function sortBy(k: SortKey) {
    if (sortKey === k) sortAsc = !sortAsc;
    else {
      sortKey = k;
      sortAsc = k !== 'seen' ? !['alt', 'speed'].includes(k) : true;
    }
  }

  async function poll() {
    try {
      const r = await api.adsbAircraft(since);
      since = r.now;
      now = r.now;
      const next = new Map<string, AdsbTrailPoint[]>();
      for (const a of r.aircraft) {
        const old = trails.get(a.icao) ?? [];
        const last = old.length ? old[old.length - 1].ts : 0;
        const add = a.trail.filter((p) => p.ts > last);
        next.set(a.icao, add.length ? [...old, ...add].slice(-720) : old);
      }
      trails = next;
      list = r.aircraft;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function loadInfo() {
    try {
      info = await api.adsb();
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function loadRange() {
    try {
      range = await api.adsbRange();
    } catch {
      /* keep the last one */
    }
  }

  onMount(() => {
    loadInfo();
  });

  // Poll while the tab is showing.
  $effect(() => {
    if (!active) return;
    poll();
    loadInfo();
    loadRange();
    const fast = setInterval(poll, 1000);
    const slow = setInterval(loadInfo, 5000);
    const slower = setInterval(loadRange, 30_000);
    return () => {
      clearInterval(fast);
      clearInterval(slow);
      clearInterval(slower);
    };
  });

  const adsbWhy = $derived(unsupported(radio, 1_090_000_000));

  async function act(f: () => Promise<unknown>) {
    busy = true;
    err = null;
    try {
      await f();
      await loadInfo();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const tuneMain = () => act(() => api.configure({ center_hz: 1_090_000_000, sample_rate: 2_400_000 }));

  let inRotation = $state<boolean | null>(null);
  $effect(() => {
    if (!isAdmin || !info) return;
    void info.status.source;
    api.schedule().then(
      (s) => (inRotation = s.config.profiles.some((p) => Math.abs(p.center_hz - 1_090_000_000) < 300_000)),
      () => {},
    );
  });

  const addToRotation = () =>
    act(async () => {
      const s = await api.schedule();
      if (!s.config.profiles.some((p) => Math.abs(p.center_hz - 1_090_000_000) < 300_000)) {
        s.config.profiles.push({ name: 'ADS-B 1090', center_hz: 1_090_000_000, sample_rate: 2_400_000, dwell_s: 60 });
      }
      await api.setSchedule(s.config);
      inRotation = true;
    });

  const fmtKm = (km: number | null) => (km === null ? '–' : `${km.toFixed(km < 10 ? 1 : 0)} km`);
  const fmtNm = (km: number) => `${(km / 1.852).toFixed(0)} NM`;
  const fmtPos = (a: Aircraft) =>
    a.lat === null || a.lon === null
      ? 'no position yet'
      : `${Math.abs(a.lat).toFixed(4)}° ${a.lat >= 0 ? 'N' : 'S'}, ${Math.abs(a.lon).toFixed(4)}° ${a.lon >= 0 ? 'E' : 'W'}`;
  const maxRange = $derived(range ? Math.max(0, ...range.all_time) : 0);
  const max24 = $derived(range ? Math.max(0, ...range.last_24h) : 0);
</script>

<div class="adsb">
  <section class="panel top">
    <div class="head">
      <h2>Aircraft</h2>
      {#if info}
        <div class="pills small-text">
          <span class="pill" title={info.status.receiver.error ?? ''}>
            <span class="dot" class:ok={info.status.source !== 'none'} class:bad={info.status.source === 'none'}></span>
            {#if info.status.source === 'receiver'}
              ADS-B receiver: {info.status.receiver.device ?? info.status.receiver.device_id}
            {:else if info.status.source === 'main'}
              Main receiver on 1090 MHz
            {:else if !info.config.enabled}
              Decoding is off
            {:else}
              Nothing listening on 1090 MHz
            {/if}
          </span>
          <span class="pill">{live.length} aircraft · {live.filter((a) => a.lat !== null).length} on the map</span>
          <span class="pill">{info.status.msg_rate.toFixed(0)} msg/s</span>
          <span class="pill" title="Different aircraft heard in the last 24 hours">{info.seen_24h} in 24 h</span>
          {#if info.status.max_range_km !== null}
            <span class="pill" title="Furthest aircraft now">furthest {fmtKm(info.status.max_range_km)}</span>
          {/if}
        </div>
      {/if}
    </div>

    {#if info}
      {#if info.status.source === 'none' && info.config.enabled}
        <div class="hint small-text" role="status">
          <span>
            {#if info.status.receiver.device_id && info.status.receiver.device_id !== 'off'}
              The ADS-B receiver isn't delivering samples{info.status.receiver.error ? `: ${info.status.receiver.error}` : '.'}
            {:else}
              No receiver is on 1090 MHz. Use a second dongle just for ADS-B, or visit 1090 MHz with the main one
              (aircraft drop off while it's tuned elsewhere).
            {/if}
          </span>
          {#if isAdmin}
            <span class="actions">
              <button type="button" class="small" onclick={() => onsettings('receiver')}>Set up an ADS-B receiver</button>
              {#if radio?.can_tune && !adsbWhy}
                <button type="button" class="small" disabled={busy} onclick={tuneMain}>Tune the main receiver to 1090 MHz</button>
                {#if inRotation === false}
                  <button type="button" class="small" disabled={busy} onclick={addToRotation}>Add ADS-B to the rotation</button>
                {/if}
              {/if}
            </span>
          {/if}
        </div>
      {:else if info.status.source === 'main'}
        <p class="muted small-text">
          Listening with the main receiver while it's on 1090 MHz; aircraft drop off when it tunes elsewhere.
        </p>
      {/if}
      {#if !home}
        <p class="muted small-text">
          Set the station location for distances, range rings and range stats{#if isAdmin}
            <button type="button" class="link" onclick={() => onsettings('passes')}>in Settings → Passes</button>{/if}.
        </p>
      {/if}
      {#if info.status.db.rows === 0 && isAdmin}
        <p class="muted small-text">
          For registrations and aircraft types, download the aircraft database in
          <button type="button" class="link" onclick={() => onsettings('receiver')}>Settings → Receiver</button>.
        </p>
      {/if}
    {/if}

    <div class="filters">
      <input type="search" placeholder="Filter by callsign, registration, type, squawk" bind:value={query} aria-label="Filter aircraft" />
      <div class="seg" role="group" aria-label="Trails">
        <span class="muted small-text">Trails</span>
        {#each [['all', 'All'], ['selected', 'Selected'], ['off', 'Off']] as [v, l] (v)}
          <button type="button" class="small" class:active={trailMode === v} onclick={() => (trailMode = v as TrailMode)}>{l}</button>
        {/each}
      </div>
      <div class="seg" role="group" aria-label="Range">
        <span class="muted small-text">Range</span>
        {#each [['off', 'Off'], ['rings', 'Rings'], ['24h', '24 h'], ['all', 'All time']] as [v, l] (v)}
          <button
            type="button"
            class="small"
            class:active={rangeMode === v}
            disabled={!home && v !== 'off'}
            onclick={() => (rangeMode = v as RangeMode)}>{l}</button
          >
        {/each}
      </div>
      <label class="check small-text"><input type="checkbox" bind:checked={labels} /> Labels</label>
    </div>
    {#if err}<p class="error">{err}</p>{/if}
  </section>

  <div class="main">
    <section class="panel map-panel">
      {#if info}
        <AdsbMap
          aircraft={live}
          {trails}
          selected={sel?.icao ?? null}
          tileUrl={info.tile_url}
          {home}
          {trailMode}
          {labels}
          {range}
          {rangeMode}
          {follow}
          onselect={(id) => (selected = id)}
        />
      {/if}
      <div class="legend small-text" aria-label="Altitude colours">
        <span class="bar" style:background={legendGradient()}></span>
        <span class="ticks">
          {#each LEGEND_FT as ft (ft)}
            <span>{ft === 0 ? '0' : ft >= 10000 ? `${ft / 1000}k` : ft.toLocaleString('en-GB')}</span>
          {/each}
        </span>
        <span class="muted">ft · grey: on the ground</span>
      </div>
    </section>

    <aside class="panel side">
      {#if sel}
        {@const em = emergency(sel)}
        <div class="head">
          <h2>{aircraftName(sel)}</h2>
          <button type="button" class="small" onclick={() => (selected = null)}>Back to list</button>
        </div>
        {#if em}<p class="emergency-banner">Emergency: {em}{sel.squawk ? ` (squawk ${sel.squawk})` : ''}</p>{/if}
        <dl class="small-text">
          {#if sel.callsign}<dt>Callsign</dt><dd>{sel.callsign}</dd>{/if}
          <dt>ICAO address</dt><dd class="mono">{sel.icao.toUpperCase()}</dd>
          {#if sel.reg}<dt>Registration</dt><dd>{sel.reg}</dd>{/if}
          {#if sel.type_code || sel.desc}<dt>Aircraft</dt><dd>{[sel.desc, sel.type_code].filter(Boolean).join(' · ')}</dd>{/if}
          {#if sel.category}<dt>Category</dt><dd>{CATEGORIES[sel.category] ?? sel.category} ({sel.category})</dd>{/if}
          {#if sel.military}<dt>Operator</dt><dd>military</dd>{/if}
          <dt>Altitude</dt><dd>{fmtAlt(sel)}{sel.alt_gnss_ft !== null && sel.alt_baro_ft !== null ? ` (GNSS ${sel.alt_gnss_ft.toLocaleString('en-GB')} ft)` : ''}</dd>
          <dt>Climb</dt><dd>{fmtVrate(sel.vrate_fpm)}</dd>
          <dt>Speed</dt>
          <dd>
            {sel.gs_kt === null ? '–' : `${Math.round(sel.gs_kt)} kt ground`}{sel.airspeed_kt !== null
              ? ` · ${sel.airspeed_kt} kt ${sel.tas ? 'TAS' : 'IAS'}`
              : ''}
          </dd>
          <dt>Heading</dt>
          <dd>
            {sel.track_deg !== null
              ? `${Math.round(sel.track_deg)}° track (${compass(sel.track_deg)})`
              : sel.heading_deg !== null
                ? `${Math.round(sel.heading_deg)}° heading`
                : '–'}
          </dd>
          <dt>Squawk</dt><dd>{sel.squawk ?? '–'}</dd>
          <dt>Position</dt><dd>{fmtPos(sel)}</dd>
          {#if sel.distance_km !== null && sel.bearing_deg !== null}
            <dt>From home</dt>
            <dd>{fmtKm(sel.distance_km)} ({fmtNm(sel.distance_km)}) {compass(sel.bearing_deg)}, {Math.round(sel.bearing_deg)}°</dd>
          {/if}
          <dt>Signal</dt><dd>{sel.rssi.toFixed(1)} dBFS</dd>
          <dt>Messages</dt><dd>{sel.msgs.toLocaleString('en-GB')}</dd>
          <dt>First heard</dt><dd>{relTime(sel.first_ms, now)}</dd>
          <dt>Last heard</dt><dd>{relTime(sel.seen_ms, now)}</dd>
        </dl>
        {#if sel.lat !== null}
          <label class="check small-text"><input type="checkbox" bind:checked={follow} /> Keep it in the middle of the map</label>
        {/if}
      {:else}
        <h2>Aircraft</h2>
        {#if shown.length === 0}
          <p class="muted small-text">{live.length ? 'No aircraft match the filter.' : 'No aircraft heard in the last minute.'}</p>
        {:else}
          <div class="table-wrap">
            <table class="small-text">
              <thead>
                <tr>
                  {#each SORTS as s (s.key)}
                    <th class:num={s.numeric} class={`col-${s.key}`} aria-sort={sortKey === s.key ? (sortAsc ? 'ascending' : 'descending') : 'none'}>
                      <button type="button" class="sort" onclick={() => sortBy(s.key)}>
                        {s.label}{sortKey === s.key ? (sortAsc ? ' ▲' : ' ▼') : ''}
                      </button>
                    </th>
                  {/each}
                </tr>
              </thead>
              <tbody>
                {#each shown as a (a.icao)}
                  <tr
                    class:emergency={!!emergency(a)}
                    class:nopos={a.lat === null}
                    tabindex="0"
                    onclick={() => (selected = a.icao)}
                    onkeydown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        selected = a.icao;
                      }
                    }}
                  >
                    <td class="col-name"><strong>{aircraftName(a)}</strong></td>
                    <td class="col-type">{a.type_code ?? a.category ?? ''}</td>
                    <td class="num col-alt">{a.on_ground ? 'GND' : (a.alt_baro_ft ?? a.alt_gnss_ft)?.toLocaleString('en-GB') ?? ''}</td>
                    <td class="num col-speed">{a.gs_kt === null ? '' : Math.round(a.gs_kt)}</td>
                    <td class="num col-dist">{a.distance_km === null ? '' : a.distance_km.toFixed(0)}</td>
                    <td class="col-squawk">{a.squawk ?? ''}</td>
                    <td class="num col-seen">{Math.max(0, Math.round((now - a.seen_ms) / 1000))}s</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="muted small-text">Altitude in feet, speed in knots, distance in km. Greyed: no position yet.</p>
        {/if}
      {/if}
    </aside>
  </div>

  {#if range && home}
    <section class="panel stats">
      <h2>Range</h2>
      <p class="small-text">
        Furthest in the last 24 hours: <strong>{fmtKm(max24)}</strong>; since {new Date(range.since_ms).toLocaleDateString()}:
        <strong>{fmtKm(maxRange)}</strong>. Show the outline per direction with <em>Range → 24 h / All time</em> on the
        map; it shows where the antenna sees furthest and what blocks it.
      </p>
      {#if isAdmin}
        <button
          type="button"
          class="small"
          disabled={busy}
          onclick={() => {
            if (confirm('Clear the all-time range stats?')) act(() => api.resetAdsbRange().then(loadRange));
          }}>Reset range stats</button
        >
      {/if}
    </section>
  {/if}
</div>

<style>
  .adsb {
    display: grid;
    gap: 16px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 6px 12px;
  }
  .pills {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .hint {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 12px;
    padding: 8px 10px;
    margin: 10px 0 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 14px;
    margin-top: 10px;
  }
  .filters input[type='search'] {
    flex: 1 1 220px;
    min-width: 0;
  }
  .seg {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .seg .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 440px;
    gap: 16px;
    align-items: stretch;
  }
  .map-panel {
    padding: 6px;
    min-height: 600px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .legend {
    display: grid;
    grid-template-columns: minmax(0, 320px) auto;
    align-items: center;
    gap: 2px 10px;
    padding: 0 4px 2px;
  }
  .legend .bar {
    height: 8px;
    border-radius: 4px;
  }
  .legend .ticks {
    grid-column: 1;
    display: flex;
    justify-content: space-between;
    font-size: 11px;
  }
  .legend .muted {
    grid-row: 1 / span 2;
    grid-column: 2;
  }
  .side {
    min-width: 0;
    max-height: 760px;
    overflow-y: auto;
  }
  .table-wrap {
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  th {
    text-align: left;
    padding: 0;
    border-bottom: 1px solid var(--border);
  }
  th.num,
  td.num {
    text-align: right;
  }
  .sort {
    width: 100%;
    background: none;
    border: none;
    padding: 6px 4px;
    font: inherit;
    font-weight: 600;
    color: var(--muted);
    text-align: inherit;
    white-space: nowrap;
  }
  td {
    padding: 6px 4px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover,
  tbody tr:focus-visible {
    background: var(--surface-2);
  }
  tr.nopos td {
    color: var(--muted);
  }
  tr.emergency td {
    color: #ef4444;
  }
  .emergency-banner {
    padding: 6px 10px;
    border-radius: 6px;
    background: #7f1d1d;
    color: #fff;
    font-weight: 600;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px 12px;
    margin: 0 0 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    text-decoration: underline;
  }
  .stats p {
    margin: 0 0 10px;
  }
  @media (max-width: 1100px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
    .map-panel {
      min-height: 460px;
    }
  }
  @media (max-width: 600px) {
    .col-squawk,
    .col-seen,
    .col-speed {
      display: none;
    }
  }
</style>
