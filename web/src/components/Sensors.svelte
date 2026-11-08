<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type Device, type DevicePatch, type EventRow, type RadioState, type Rtl433Info } from '../lib/api';
  import { CHART_FIELDS, deviceLabel, deviceTitle, readings, series } from '../lib/sensors';
  import TimeChart from './TimeChart.svelte';
  import DeviceIdent from './DeviceIdent.svelte';
  import { unsupported } from '../lib/coverage';
  import { formatMHz, formatRate, relTime } from '../lib/format';
  import { Selection } from '../lib/selection.svelte';
  import { indeterminate } from '../lib/actions';

  let { isAdmin, radio }: { isAdmin: boolean; radio: RadioState | null } = $props();
  const canCopy = typeof navigator !== 'undefined' && !!navigator.clipboard;
  let copied = $state(false);

  let devices = $state<Device[]>([]);
  let feed = $state<{ device: Device; event: EventRow }[]>([]);
  let info = $state<Rtl433Info | null>(null);
  let denied = $state(false);
  let err = $state<string | null>(null);
  let query = $state('');
  let kind = $state<'all' | 'tpms' | 'other'>('all');
  let trackedOnly = $state(false);
  let selectedId = $state<number | null>(null);
  let history = $state<EventRow[]>([]);
  const RANGES = [
    { label: '1 h', ms: 3600_000 },
    { label: '24 h', ms: 24 * 3600_000 },
    { label: '7 days', ms: 7 * 24 * 3600_000 },
  ];
  let rangeMs = $state(24 * 3600_000);
  let nameInput = $state('');
  let busy = $state(false);
  let now = $state(Date.now());

  const selected = $derived(devices.find((d) => d.id === selectedId) ?? null);
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return devices.filter(
      (d) =>
        (kind === 'all' || (kind === 'tpms') === (d.kind === 'TPMS')) &&
        (!trackedOnly || d.tracked) &&
        (!q || `${deviceLabel(d)} ${d.model} ${d.dev_id ?? ''}`.toLowerCase().includes(q)),
    );
  });
  const tpmsCount = $derived(devices.filter((d) => d.kind === 'TPMS').length);

  // Sorting (remembered per browser).
  type SortKey = 'name' | 'signal' | 'seen' | 'count';
  const SORT_PREF = 'sdr.sensors.sort';
  const savedSort = (() => {
    try {
      return JSON.parse(localStorage.getItem(SORT_PREF) ?? 'null') as { key: SortKey; dir: 1 | -1 } | null;
    } catch {
      return null;
    }
  })();
  let sortKey = $state<SortKey>(savedSort?.key ?? 'seen');
  let sortDir = $state<1 | -1>(savedSort?.dir ?? -1);
  function sortBy(k: SortKey) {
    if (sortKey === k) sortDir = sortDir === 1 ? -1 : 1;
    else {
      sortKey = k;
      sortDir = k === 'name' ? 1 : -1; // names A→Z, numbers biggest/newest first
    }
    try {
      localStorage.setItem(SORT_PREF, JSON.stringify({ key: sortKey, dir: sortDir }));
    } catch {
      /* not available */
    }
  }
  const rssi = (d: Device) => (typeof d.last_data.rssi === 'number' ? d.last_data.rssi : null);
  const sorted = $derived.by(() => {
    const key = (d: Device): number | string | null =>
      sortKey === 'name' ? deviceTitle(d).toLowerCase() : sortKey === 'signal' ? rssi(d) : sortKey === 'count' ? d.count : d.last_seen;
    return [...shown].sort((a, b) => {
      const x = key(a);
      const y = key(b);
      // Devices without a value (e.g. no signal level) always go last.
      if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
      const c = typeof x === 'string' ? x.localeCompare(y as string) : (x as number) - (y as number);
      return c * sortDir || b.last_seen - a.last_seen;
    });
  });
  const ariaSort = (k: SortKey) => (sortKey === k ? (sortDir === 1 ? 'ascending' : 'descending') : 'none');
  const arrow = (k: SortKey) => (sortKey === k ? (sortDir === 1 ? '▲' : '▼') : '');

  // Multi-select (admins) for bulk delete.
  const sel = new Selection();
  const order = $derived(sorted.map((d) => d.id));
  $effect(() => sel.keepOnly(devices.map((d) => d.id)));

  async function removeSelected() {
    const ids = sel.list();
    if (!ids.length) return;
    const readingsTotal = devices.filter((d) => sel.has(d.id)).reduce((n, d) => n + d.count, 0);
    if (!confirm(`Delete ${ids.length} device${ids.length === 1 ? '' : 's'} and ${readingsTotal} readings? This cannot be undone.`))
      return;
    try {
      await api.deleteDevices(ids);
      const gone = new Set(ids);
      devices = devices.filter((d) => !gone.has(d.id));
      feed = feed.filter((f) => !gone.has(f.device.id));
      if (selectedId !== null && gone.has(selectedId)) selectedId = null;
      sel.clear();
    } catch (e) {
      fail(e);
    }
  }
  const charts = $derived(
    CHART_FIELDS.map((f) => ({ ...f, points: series(history, f.key) })).filter((c) => c.points.length > 0),
  );

  /** Called by App for every live event from the WebSocket. */
  export function push(device: Device, event: EventRow) {
    const i = devices.findIndex((d) => d.id === device.id);
    if (i >= 0) devices.splice(i, 1);
    devices.unshift(device);
    feed = [{ device, event }, ...feed].slice(0, 50);
    if (selectedId === device.id) {
      now = Date.now(); // keep the newest point inside the chart's time range
      history = [event, ...history];
    }
  }

  function fail(e: unknown) {
    if (e instanceof ApiError && e.status === 403) denied = true;
    else err = e instanceof Error ? e.message : String(e);
  }

  async function load() {
    try {
      const [d, ev, i] = await Promise.all([api.devices('rtl_433'), api.events(50, 'rtl_433'), api.rtl433()]);
      devices = d;
      info = i;
      const byId = new Map(d.map((x) => [x.id, x]));
      feed = ev.flatMap((event) => {
        const device = byId.get(event.device_id);
        return device ? [{ device, event }] : [];
      });
    } catch (e) {
      fail(e);
    }
  }

  async function refreshStatus() {
    if (denied) return;
    try {
      info = await api.rtl433();
    } catch {
      /* keep the last status */
    }
  }

  onMount(() => {
    load();
    const t = setInterval(() => {
      now = Date.now();
      refreshStatus();
    }, 5000);
    return () => clearInterval(t);
  });

  /** Open a device's details (used by the dashboard). */
  export async function open(id: number) {
    let d = devices.find((x) => x.id === id);
    if (!d) {
      try {
        d = (await api.devices('rtl_433')).find((x) => x.id === id);
      } catch (e) {
        fail(e);
      }
    }
    if (d && selectedId !== id) await select(d);
  }

  async function copyId(id: string) {
    try {
      await navigator.clipboard.writeText(id);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      /* clipboard blocked; the ID is still selectable */
    }
  }

  async function select(d: Device) {
    if (selectedId === d.id) {
      selectedId = null;
      return;
    }
    selectedId = d.id;
    copied = false;
    nameInput = d.name ?? '';
    await loadHistory();
  }

  async function loadHistory() {
    const id = selectedId;
    if (id === null) return;
    history = [];
    try {
      const h = await api.deviceEvents(id, { since: Date.now() - rangeMs, limit: 10_000 });
      if (selectedId === id) history = h;
    } catch (e) {
      fail(e);
    }
  }

  function setRange(ms: number) {
    rangeMs = ms;
    loadHistory();
  }

  async function update(d: Device, patch: DevicePatch) {
    busy = true;
    err = null;
    try {
      const updated = await api.updateDevice(d.id, patch);
      const i = devices.findIndex((x) => x.id === d.id);
      if (i >= 0) devices[i] = updated;
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function remove(d: Device) {
    if (!confirm(`Delete ${deviceLabel(d)} and its ${d.count} readings?`)) return;
    try {
      await api.deleteDevice(d.id);
      devices = devices.filter((x) => x.id !== d.id);
      feed = feed.filter((f) => f.device.id !== d.id);
      selectedId = null;
    } catch (e) {
      fail(e);
    }
  }

  async function tuneToIsm() {
    if (!info) return;
    try {
      // Sit 80 kHz above the target so the DC spike stays clear of it.
      await api.configure({ center_hz: info.config.frequency_hz + 80_000, sample_rate: 1_024_000 });
      setTimeout(refreshStatus, 500);
    } catch (e) {
      fail(e);
    }
  }

  function saveName(e: SubmitEvent) {
    e.preventDefault();
    if (selected) update(selected, { name: nameInput });
  }

  const time = (ms: number) => new Date(ms).toLocaleTimeString();
</script>

{#if denied}
  <section class="panel">
    <h2>Sensors</h2>
    <p class="muted">Sensor data (TPMS and other ISM devices) is only shown to admins on this server.</p>
  </section>
{:else}
  <div class="sensors">
    <section class="panel main">
      <div class="head">
        <h2>Sensors</h2>
        {#if info}
          {@const s = info.status}
          <div class="status small-text">
            <span class="dot" class:ok={s.running && s.in_band} class:bad={!!s.error}></span>
            {#if !s.enabled}
              <span>rtl_433 is turned off</span>
            {:else if unsupported(radio, s.frequency_hz)}
              <span class="na-tag">Disabled</span>
              <span>{formatMHz(s.frequency_hz, 3)}: {unsupported(radio, s.frequency_hz)}</span>
            {:else if s.error}
              <span class="error-text">{s.error}</span>
            {:else if !s.in_band}
              <span
                >Idle: {formatMHz(s.frequency_hz, 3)} is outside the tuned band{radio
                  ? ` (${formatMHz(radio.settings.center_hz, 3)})`
                  : ''}</span
              >
              {#if isAdmin && radio?.can_tune}
                <button type="button" class="small" onclick={tuneToIsm}>Tune to {formatMHz(s.frequency_hz, 2)}</button>
              {/if}
            {:else}
              <span>rtl_433 decoding {formatMHz(s.frequency_hz, 3)} at {formatRate(s.sample_rate)}</span>
            {/if}
            {#if info.mqtt}
              <span class="pill" title={info.mqtt.error ?? info.mqtt.server}>
                <span class="dot" class:ok={info.mqtt.connected} class:bad={!info.mqtt.connected}></span>
                MQTT
              </span>
            {/if}
          </div>
        {/if}
      </div>

      <div class="filters">
        <input type="search" placeholder="Filter by name, model or ID" bind:value={query} aria-label="Filter devices" />
        <select bind:value={kind} aria-label="Device type">
          <option value="all">All types ({devices.length})</option>
          <option value="tpms">TPMS ({tpmsCount})</option>
          <option value="other">Other ({devices.length - tpmsCount})</option>
        </select>
        <label class="check"><input type="checkbox" bind:checked={trackedOnly} /> Tracked only</label>
      </div>

      {#if err}<p class="error">{err}</p>{/if}

      {#if isAdmin && sel.size > 0}
        <div class="bulk small-text" role="toolbar" aria-label="Selected devices">
          <strong>{sel.size} selected</strong>
          <button type="button" class="small danger" onclick={removeSelected}>Delete</button>
          <button type="button" class="small" onclick={() => sel.clear()}>Clear</button>
        </div>
      {/if}

      {#if devices.length === 0}
        <p class="muted empty">
          No devices heard yet. rtl_433 only runs while {info ? formatMHz(info.config.frequency_hz, 2) : '433.92 MHz'} is
          inside the tuned band.
        </p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                {#if isAdmin}
                  <th class="pick">
                    <input
                      type="checkbox"
                      aria-label="Select all shown devices"
                      checked={sel.allOf(order)}
                      use:indeterminate={sel.someOf(order) && !sel.allOf(order)}
                      onchange={() => sel.toggleAll(order)}
                    />
                  </th>
                {/if}
                <th aria-sort={ariaSort('name')}><button type="button" class="sort" onclick={() => sortBy('name')}>Device {arrow('name')}</button></th>
                <th>Readings</th>
                <th class="num hide-sm" aria-sort={ariaSort('signal')}
                  ><button type="button" class="sort" onclick={() => sortBy('signal')}>Signal {arrow('signal')}</button></th
                >
                <th aria-sort={ariaSort('seen')}><button type="button" class="sort" onclick={() => sortBy('seen')}>Last heard {arrow('seen')}</button></th>
                <th class="num hide-sm" aria-sort={ariaSort('count')}
                  ><button type="button" class="sort" onclick={() => sortBy('count')}>Count {arrow('count')}</button></th
                >
                <th title="Tracked devices are kept forever and published to Home Assistant">Track</th>
              </tr>
            </thead>
            <tbody>
              {#each sorted as d (d.id)}
                {@const r = d.last_data}
                <tr class:sel={d.id === selectedId} class:picked={sel.has(d.id)} onclick={() => select(d)}>
                  {#if isAdmin}
                    <td class="pick">
                      <input
                        type="checkbox"
                        checked={sel.has(d.id)}
                        aria-label="Select {deviceLabel(d)}"
                        onclick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          sel.toggle(d.id, e.shiftKey, order);
                        }}
                      />
                    </td>
                  {/if}
                  <td>
                    <button type="button" class="link" onclick={(e) => { e.stopPropagation(); select(d); }}>
                      {deviceTitle(d)}
                    </button>
                    {#if d.dash_pos !== null}<span class="badge" title="On the dashboard">dashboard</span>{/if}
                    <div><DeviceIdent device={d} showModel={!!d.name} /></div>
                  </td>
                  <td class="readings">{readings(r).slice(0, 4).join(' · ')}</td>
                  <td class="num hide-sm">{typeof r.rssi === 'number' ? `${r.rssi.toFixed(1)} dB` : ''}</td>
                  <td class="nowrap" title={new Date(d.last_seen).toLocaleString()}>{relTime(d.last_seen, now)}</td>
                  <td class="num hide-sm">{d.count}</td>
                  <td>
                    <input
                      type="checkbox"
                      checked={d.tracked}
                      disabled={!isAdmin || busy}
                      aria-label="Track {deviceLabel(d)}"
                      onclick={(e) => e.stopPropagation()}
                      onchange={(e) => update(d, { tracked: (e.currentTarget as HTMLInputElement).checked })}
                    />
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>

    <aside class="side">
      {#if selected}
        <section class="panel">
          <div class="head">
            <h2>{deviceTitle(selected)}</h2>
            <button type="button" class="small" onclick={() => (selectedId = null)}>Close</button>
          </div>
          <dl class="small-text">
            <dt>Model</dt>
            <dd class="model">
              <span>{selected.model}</span>
              {#if selected.dev_id}
                <span class="id">
                  ID <code>{selected.dev_id}</code>
                  {#if canCopy}
                    <button type="button" class="small" onclick={() => copyId(selected!.dev_id!)}
                      >{copied ? 'Copied' : 'Copy'}</button
                    >
                  {/if}
                </span>
              {/if}
            </dd>
            {#if selected.channel}<dt>Channel</dt><dd>{selected.channel}</dd>{/if}
            {#if selected.kind}<dt>Type</dt><dd>{selected.kind}</dd>{/if}
            <dt>First heard</dt><dd>{new Date(selected.first_seen).toLocaleString()}</dd>
            <dt>Readings</dt><dd>{selected.count}</dd>
          </dl>
          {#if isAdmin}
            <form class="name" onsubmit={saveName}>
              <input placeholder="Name, e.g. Garden sensor" maxlength="64" bind:value={nameInput} aria-label="Device name" />
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
              <label class="check">
                <input
                  type="checkbox"
                  checked={selected.dash_pos !== null}
                  disabled={busy}
                  onchange={(e) => update(selected!, { dashboard: (e.currentTarget as HTMLInputElement).checked })}
                />
                Show on dashboard
              </label>
              <button type="button" class="small danger" onclick={() => remove(selected!)}>Delete</button>
            </div>
          {/if}
          <div class="range" role="group" aria-label="Time range">
            {#each RANGES as r (r.ms)}
              <button type="button" class="small" class:active={rangeMs === r.ms} onclick={() => setRange(r.ms)}
                >{r.label}</button
              >
            {/each}
          </div>
          {#each charts as c (c.key)}
            <TimeChart title={c.label} unit={c.unit} digits={c.digits} points={c.points} from={now - rangeMs} to={now} />
          {:else}
            <p class="muted small-text">No numeric readings to chart in this period.</p>
          {/each}
          <h3>History</h3>
          <div class="history">
            <table>
              <tbody>
                {#each history.slice(0, 200) as ev (ev.id)}
                  <tr>
                    <td class="nowrap muted" title={new Date(ev.ts).toLocaleString()}>{time(ev.ts)}</td>
                    <td>{readings(ev.data).join(' · ')}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {:else}
        <section class="panel">
          <h2>Live</h2>
          {#if feed.length === 0}
            <p class="muted small-text">Decoded transmissions appear here as they arrive.</p>
          {/if}
          <ul class="feed">
            {#each feed as f (f.event.id)}
              <li>
                <span class="muted nowrap">{time(f.event.ts)}</span>
                <button type="button" class="link" onclick={() => select(f.device)}>{deviceTitle(f.device)}</button>
                <DeviceIdent device={f.device} showModel={!!f.device.name} />
                <div class="small-text">{readings(f.event.data).slice(0, 3).join(' · ')}</div>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    </aside>
  </div>
{/if}

<style>
  .sensors {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 420px;
    gap: 16px;
    align-items: start;
  }
  .main {
    min-width: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 6px 12px;
  }
  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .error-text {
    color: var(--bad);
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
    gap: 8px;
    margin: 4px 0 10px;
  }
  .filters input[type='search'] {
    flex: 1 1 200px;
    min-width: 0;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  .empty {
    margin: 24px 0;
  }
  .bulk {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    margin: 0 0 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  th.pick,
  td.pick {
    width: 1%;
    padding-right: 0;
  }
  tr.picked td {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  button.sort {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    white-space: nowrap;
  }
  button.sort:hover {
    color: var(--text);
  }
  .table-wrap {
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
  }
  th,
  td {
    text-align: left;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  th {
    color: var(--muted);
    font-weight: 500;
    white-space: nowrap;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover,
  tr.sel {
    background: var(--surface-2);
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .nowrap {
    white-space: nowrap;
  }
  .readings {
    font-variant-numeric: tabular-nums;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font-weight: 500;
    text-align: left;
  }
  .side {
    display: grid;
    gap: 16px;
    min-width: 0;
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
  .model {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 12px;
  }
  .model .id {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .model code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
    user-select: all;
  }
  .badge {
    margin-left: 6px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--muted);
    font-size: 11px;
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
    margin: 10px 0 0;
  }
  .range {
    display: flex;
    gap: 4px;
    margin: 14px 0 10px;
  }
  .range .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  h3 {
    margin: 16px 0 6px;
    font-size: 15px;
  }
  .history {
    max-height: 420px;
    overflow-y: auto;
  }
  .history td {
    font-size: 13px;
    cursor: default;
  }
  .feed {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 640px;
    overflow-y: auto;
  }
  .feed li {
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
    font-size: 14px;
  }
  .feed .link {
    margin-left: 6px;
  }
  .feed .small-text {
    overflow-wrap: anywhere;
  }
  @media (max-width: 1000px) {
    .sensors {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  @media (max-width: 560px) {
    .hide-sm {
      display: none;
    }
  }
</style>
