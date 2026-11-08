<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type AprsInfo, type Device, type EventRow, type RadioState, type Track } from '../lib/api';
  import { motion, stationName, stationText, symbolName } from '../lib/aprs';
  import { formatMHz, relTime } from '../lib/format';
  import { CHART_FIELDS, series } from '../lib/sensors';
  import AprsMap from './AprsMap.svelte';
  import TimeChart from './TimeChart.svelte';

  let { isAdmin, radio }: { isAdmin: boolean; radio: RadioState | null } = $props();

  const WINDOWS = [
    { label: '1 h', ms: 3600_000 },
    { label: '6 h', ms: 6 * 3600_000 },
    { label: '24 h', ms: 24 * 3600_000 },
  ];

  let info = $state<AprsInfo | null>(null);
  let stations = $state<Device[]>([]);
  let packets = $state<{ device: Device; event: EventRow }[]>([]);
  let tracks = $state<Track[]>([]);
  let windowMs = $state(6 * 3600_000);
  let query = $state('');
  let selectedId = $state<number | null>(null);
  let history = $state<EventRow[]>([]);
  let nameInput = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);
  let now = $state(Date.now());

  const visible = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return stations.filter(
      (d) =>
        d.last_seen >= now - windowMs &&
        (!q || `${stationName(d)} ${d.dev_id ?? ''} ${stationText(d.last_data)}`.toLowerCase().includes(q)),
    );
  });
  const selected = $derived(stations.find((d) => d.id === selectedId) ?? null);
  const shownTracks = $derived(tracks.filter((t) => visible.some((d) => d.id === t.device_id)));
  const allIdle = $derived(!!info && info.status.channels.every((c) => !c.enabled || !c.in_band));
  const charts = $derived(
    CHART_FIELDS.filter((f) => f.key !== 'rssi')
      .map((f) => ({ ...f, points: series(history, f.key) }))
      .filter((c) => c.points.length > 0),
  );

  /** Called by App for every live APRS event. */
  export function push(device: Device, event: EventRow) {
    now = Date.now();
    const i = stations.findIndex((d) => d.id === device.id);
    if (i >= 0) stations.splice(i, 1);
    stations.unshift(device);
    packets = [{ device, event }, ...packets].slice(0, 100);
    const { lat, lon } = event.data as { lat?: number; lon?: number };
    if (typeof lat === 'number' && typeof lon === 'number') {
      const t = tracks.find((x) => x.device_id === device.id);
      if (t) t.points.push([event.ts, lat, lon]);
      else tracks.push({ device_id: device.id, points: [[event.ts, lat, lon]] });
    }
    if (selectedId === device.id) history = [event, ...history];
  }

  function fail(e: unknown) {
    err = e instanceof Error ? e.message : String(e);
  }

  async function load() {
    try {
      const [i, d, ev, t] = await Promise.all([
        api.aprs(),
        api.devices('aprs'),
        api.events(100, 'aprs'),
        api.aprsTracks(Date.now() - 24 * 3600_000),
      ]);
      info = i;
      stations = d;
      tracks = t;
      const byId = new Map(d.map((x) => [x.id, x]));
      packets = ev.flatMap((event) => {
        const device = byId.get(event.device_id);
        return device ? [{ device, event }] : [];
      });
    } catch (e) {
      fail(e);
    }
  }

  onMount(() => {
    load();
    const t = setInterval(async () => {
      now = Date.now();
      try {
        info = await api.aprs();
      } catch {
        /* keep last status */
      }
    }, 5000);
    return () => clearInterval(t);
  });

  async function select(id: number) {
    if (selectedId === id) return;
    selectedId = id;
    const d = stations.find((x) => x.id === id);
    nameInput = d?.name ?? '';
    history = [];
    try {
      const h = await api.deviceEvents(id, { since: Date.now() - 24 * 3600_000, limit: 5000 });
      if (selectedId === id) history = h;
    } catch (e) {
      fail(e);
    }
  }

  async function update(d: Device, patch: { name?: string; tracked?: boolean }) {
    busy = true;
    err = null;
    try {
      const u = await api.updateDevice(d.id, patch);
      const i = stations.findIndex((x) => x.id === d.id);
      if (i >= 0) stations[i] = u;
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function remove(d: Device) {
    if (!confirm(`Delete ${stationName(d)} and its ${d.count} packets?`)) return;
    try {
      await api.deleteDevice(d.id);
      stations = stations.filter((x) => x.id !== d.id);
      packets = packets.filter((p) => p.device.id !== d.id);
      tracks = tracks.filter((t) => t.device_id !== d.id);
      selectedId = null;
    } catch (e) {
      fail(e);
    }
  }

  async function tuneVhf() {
    try {
      await api.configure({ center_hz: 145_300_000, sample_rate: 2_048_000 });
    } catch (e) {
      fail(e);
    }
  }

  function saveName(e: SubmitEvent) {
    e.preventDefault();
    if (selected) update(selected, { name: nameInput });
  }

  const time = (ms: number) => new Date(ms).toLocaleTimeString();
  const fmtPos = (d: Device) =>
    d.lat === null || d.lon === null
      ? 'no position'
      : `${Math.abs(d.lat).toFixed(4)}° ${d.lat >= 0 ? 'N' : 'S'}, ${Math.abs(d.lon!).toFixed(4)}° ${d.lon! >= 0 ? 'E' : 'W'}`;
</script>

<div class="aprs">
  <section class="panel top">
    <div class="head">
      <h2>APRS</h2>
      {#if info}
        <div class="channels small-text">
          {#each info.status.channels as c, i (i)}
            <span class="pill" title={c.error ?? (c.in_band ? `${c.packets} packets since start` : 'outside the tuned band')}>
              <span class="dot" class:ok={c.running && c.in_band} class:bad={!!c.error}></span>
              {c.label} {formatMHz(c.frequency_hz, 3)}{c.enabled ? (c.in_band ? '' : ' · idle') : ' · off'}
            </span>
          {/each}
          {#if allIdle && isAdmin && radio?.can_tune}
            <button type="button" class="small" onclick={tuneVhf}>Tune to VHF block</button>
          {/if}
        </div>
      {/if}
    </div>
    <div class="filters">
      <input type="search" placeholder="Filter by callsign or comment" bind:value={query} aria-label="Filter stations" />
      <div class="seg" role="group" aria-label="Time window">
        {#each WINDOWS as w (w.ms)}
          <button type="button" class="small" class:active={windowMs === w.ms} onclick={() => (windowMs = w.ms)}>{w.label}</button>
        {/each}
      </div>
      <span class="muted small-text">{visible.length} stations</span>
    </div>
    {#if err}<p class="error">{err}</p>{/if}
  </section>

  <div class="main">
    <section class="panel map-panel">
      {#if info}
        <AprsMap stations={visible} tracks={shownTracks} {selectedId} tileUrl={info.map_tiles} onselect={select} />
      {/if}
    </section>

    <aside class="panel side">
      {#if selected}
        <div class="head">
          <h2>{stationName(selected)}</h2>
          <button type="button" class="small" onclick={() => (selectedId = null)}>Back to list</button>
        </div>
        <dl class="small-text">
          {#if selected.name}<dt>Callsign</dt><dd>{selected.dev_id}</dd>{/if}
          {#if selected.model !== 'APRS'}<dt>Type</dt><dd>{selected.model} from {selected.last_data.from}</dd>{/if}
          <dt>Symbol</dt><dd>{symbolName(selected.symbol)}</dd>
          <dt>Position</dt><dd>{fmtPos(selected)}{selected.pos_ts ? ` (${relTime(selected.pos_ts, now)})` : ''}</dd>
          {#if motion(selected.last_data)}<dt>Moving</dt><dd>{motion(selected.last_data)}</dd>{/if}
          {#if stationText(selected.last_data)}<dt>Comment</dt><dd>{stationText(selected.last_data)}</dd>{/if}
          <dt>Last heard</dt><dd>{relTime(selected.last_seen, now)}</dd>
          <dt>Packets</dt><dd>{selected.count}</dd>
        </dl>
        {#if isAdmin}
          <form class="name" onsubmit={saveName}>
            <input placeholder="Name" maxlength="64" bind:value={nameInput} aria-label="Station name" />
            <button type="submit" disabled={busy}>Save</button>
          </form>
          <div class="actions">
            <label class="check">
              <input
                type="checkbox"
                checked={selected.tracked}
                disabled={busy}
                onchange={(e) => update(selected!, { tracked: (e.currentTarget as HTMLInputElement).checked })}
              />
              Track (keep, publish to Home Assistant)
            </label>
            <button type="button" class="small danger" onclick={() => remove(selected!)}>Delete</button>
          </div>
        {/if}
        {#each charts as c (c.key)}
          <TimeChart title={c.label} unit={c.unit} digits={c.digits} points={c.points} from={now - 24 * 3600_000} to={now} />
        {/each}
        <h3>Packets (24 h)</h3>
        <ul class="raw">
          {#each history.slice(0, 100) as ev (ev.id)}
            <li><span class="muted">{time(ev.ts)}</span> {ev.data.raw}</li>
          {/each}
        </ul>
      {:else}
        <h2>Stations</h2>
        {#if visible.length === 0}
          <p class="muted small-text">No stations heard in this window yet.</p>
        {/if}
        <ul class="stations">
          {#each visible as d (d.id)}
            <li>
              <button type="button" class="station" onclick={() => select(d.id)}>
                <span class="row">
                  <strong>{stationName(d)}</strong>
                  <span class="muted small-text">{relTime(d.last_seen, now)}</span>
                </span>
                <span class="muted small-text"
                  >{[symbolName(d.symbol), d.lat === null ? 'no position' : ''].filter(Boolean).join(' · ')}</span
                >
                {#if stationText(d.last_data)}<span class="small-text text">{stationText(d.last_data)}</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </aside>
  </div>

  <section class="panel">
    <h2>Packet log</h2>
    {#if packets.length === 0}
      <p class="muted small-text">Decoded packets appear here as they arrive.</p>
    {/if}
    <ul class="raw log">
      {#each packets as p (p.event.id)}
        <li>
          <span class="muted">{time(p.event.ts)}</span>
          <span class="muted">{p.event.freq_hz ? formatMHz(p.event.freq_hz, 3).replace(' MHz', '') : ''}</span>
          <button type="button" class="link" onclick={() => select(p.device.id)}>{p.event.data.raw}</button>
        </li>
      {/each}
    </ul>
  </section>
</div>

<style>
  .aprs {
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
  .channels {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
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
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .filters input {
    flex: 1 1 200px;
    min-width: 0;
  }
  .seg {
    display: flex;
    gap: 4px;
  }
  .seg .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 380px;
    gap: 16px;
    align-items: stretch;
  }
  .map-panel {
    padding: 6px;
    min-height: 560px;
    display: flex;
  }
  .side {
    min-width: 0;
    max-height: 760px;
    overflow-y: auto;
  }
  .stations,
  .raw {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .station {
    display: grid;
    gap: 2px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    padding: 8px 4px;
  }
  .station:hover {
    background: var(--surface-2);
  }
  .row {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .text {
    overflow-wrap: anywhere;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 12px;
    margin: 0 0 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .name {
    display: flex;
    gap: 8px;
  }
  .name input {
    flex: 1;
    min-width: 0;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 10px 0 14px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  h3 {
    margin: 16px 0 6px;
    font-size: 15px;
  }
  .raw li {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
    padding: 3px 0;
    border-bottom: 1px solid var(--border);
    overflow-wrap: anywhere;
  }
  .log {
    max-height: 320px;
    overflow-y: auto;
  }
  .log li {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--text);
    font: inherit;
    text-align: left;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  button.link:hover {
    color: var(--accent);
  }
  @media (max-width: 1000px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
    .map-panel {
      min-height: 420px;
    }
  }
</style>
