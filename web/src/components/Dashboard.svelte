<script lang="ts">
  import { untrack } from 'svelte';
  import { api, ApiError, type Device, type EventRow } from '../lib/api';
  import { CHART_FIELDS, deviceTitle, readings, series } from '../lib/sensors';
  import { relTime } from '../lib/format';
  import DeviceIdent from './DeviceIdent.svelte';
  import Sparkline from './Sparkline.svelte';

  let {
    isAdmin,
    active,
    onopen,
  }: { isAdmin: boolean; active: boolean; onopen: (id: number) => void } = $props();

  const DAY = 24 * 3600_000;
  /** Headline values per card; signal strength goes in the footer instead. */
  const HEADLINE = CHART_FIELDS.filter((f) => f.key !== 'rssi');

  let devices = $state<Device[]>([]);
  let history = $state<Record<number, EventRow[]>>({});
  let loaded = $state(false);
  let denied = $state(false);
  let err = $state<string | null>(null);
  let editing = $state(false);
  let busy = $state(false);
  let now = $state(Date.now());

  function fail(e: unknown) {
    if (e instanceof ApiError && e.status === 403) denied = true;
    else err = e instanceof Error ? e.message : String(e);
  }

  async function load() {
    err = null;
    try {
      const ds = await api.dashboard();
      const since = Date.now() - DAY;
      const hs = await Promise.all(ds.map((d) => api.deviceEvents(d.id, { since, limit: 5000 })));
      now = Date.now();
      devices = ds;
      history = Object.fromEntries(ds.map((d, i) => [d.id, hs[i]]));
    } catch (e) {
      fail(e);
    } finally {
      loaded = true;
    }
  }

  // Reload whenever the tab is opened: pins may have changed on the Sensors tab.
  $effect(() => {
    if (active) untrack(load);
  });

  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 5000);
    return () => clearInterval(t);
  });

  /** Called by App for every live sensor event. */
  export function push(device: Device, event: EventRow) {
    const i = devices.findIndex((d) => d.id === device.id);
    if (i < 0) return;
    devices[i] = device;
    now = Date.now();
    const cutoff = now - DAY;
    history[device.id] = [event, ...(history[device.id] ?? []).filter((e) => e.ts >= cutoff)];
  }

  /** Quiet for much longer than its usual reporting interval. */
  function isStale(d: Device, h: EventRow[]): boolean {
    const quiet = now - d.last_seen;
    if (h.length < 3) return quiet > 3600_000;
    const gaps = h.slice(1).map((e, i) => h[i].ts - e.ts).sort((a, b) => a - b);
    return quiet > Math.max(10 * 60_000, 3 * gaps[Math.floor(gaps.length / 2)]);
  }

  function metrics(d: Device, h: EventRow[]) {
    return HEADLINE.filter((f) => typeof d.last_data[f.key] === 'number')
      .slice(0, 3)
      .map((f) => ({ ...f, value: d.last_data[f.key] as number, points: series(h, f.key) }));
  }

  /** Chart start: the oldest reading, so a new sensor isn't squashed into the
   * right-hand edge of a 24 h axis; at least 1 h, at most 24 h. */
  function chartFrom(h: EventRow[]): number {
    const oldest = h.at(-1)?.ts ?? now;
    return Math.min(now - 3600_000, Math.max(now - DAY, oldest));
  }

  function spanLabel(from: number): string {
    const hrs = Math.round((now - from) / 3600_000);
    return hrs >= 24 ? '24 h' : `${hrs} h`;
  }

  async function move(i: number, by: number) {
    const j = i + by;
    if (j < 0 || j >= devices.length) return;
    const order = devices.map((d) => d.id);
    [order[i], order[j]] = [order[j], order[i]];
    busy = true;
    try {
      devices = await api.setDashboardOrder(order);
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function unpin(d: Device) {
    busy = true;
    try {
      await api.updateDevice(d.id, { dashboard: false });
      devices = devices.filter((x) => x.id !== d.id);
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if denied}
  <section class="panel">
    <h2>Dashboard</h2>
    <p class="muted">Sensor data is only shown to admins on this server.</p>
  </section>
{:else}
  <div class="head">
    <h2>Dashboard</h2>
    {#if isAdmin && devices.length > 0}
      <button type="button" class="small" class:active={editing} onclick={() => (editing = !editing)}
        >{editing ? 'Done' : 'Edit layout'}</button
      >
    {/if}
  </div>
  {#if err}<p class="error">{err}</p>{/if}

  {#if loaded && devices.length === 0}
    <section class="panel">
      <p class="muted">
        No sensors on the dashboard yet.
        {#if isAdmin}Open a sensor on the Sensors tab and tick <strong>Show on dashboard</strong>.{:else}An admin can
          add sensors from the Sensors tab.{/if}
      </p>
    </section>
  {:else}
    <div class="grid">
      {#each devices as d, i (d.id)}
        {@const h = history[d.id] ?? []}
        {@const stale = isStale(d, h)}
        {@const ms = metrics(d, h)}
        {@const r = d.last_data}
        {@const from = chartFrom(h)}
        <article class="panel card" class:stale>
          <header>
            <button type="button" class="title" onclick={() => onopen(d.id)} title="Open sensor details"
              >{deviceTitle(d)}</button
            >
            <span class="heard small-text" class:quiet={stale} title={new Date(d.last_seen).toLocaleString()}>
              {stale ? 'Quiet · ' : ''}{relTime(d.last_seen, now)}
            </span>
          </header>
          <DeviceIdent device={d} showModel={!!d.name} />

          {#if ms.length > 0}
            <div class="metrics">
              {#each ms as m (m.key)}
                <div class="metric">
                  <div class="value">
                    <span class="num">{m.value.toFixed(m.digits)}</span><span class="unit">{m.unit}</span>
                  </div>
                  <div class="label small-text">{m.label} · last {spanLabel(from)}</div>
                  <Sparkline label={m.label} unit={m.unit} digits={m.digits} points={m.points} {from} to={now} />
                </div>
              {/each}
            </div>
          {:else}
            <p class="raw small-text">{readings(r).slice(0, 4).join(' · ') || 'No readings'}</p>
          {/if}

          <footer class="small-text">
            {#if r.battery_ok === 0}
              <span class="battery-low">⚠ Battery low</span>
            {:else if r.battery_ok === 1}
              <span>Battery ok</span>
            {/if}
            {#if typeof r.rssi === 'number'}<span>{r.rssi.toFixed(1)} dB</span>{/if}
            <span>{h.length} readings in 24 h</span>
          </footer>

          {#if editing}
            <div class="edit">
              <button type="button" class="small" disabled={busy || i === 0} onclick={() => move(i, -1)}
                aria-label="Move {deviceTitle(d)} earlier">← Earlier</button
              >
              <button
                type="button"
                class="small"
                disabled={busy || i === devices.length - 1}
                onclick={() => move(i, 1)}
                aria-label="Move {deviceTitle(d)} later">Later →</button
              >
              <button type="button" class="small danger" disabled={busy} onclick={() => unpin(d)}>Remove</button>
            </div>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }
  .head h2 {
    margin: 0;
  }
  .head .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr));
    gap: 16px;
    align-items: start;
  }
  .card {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .card.stale .metrics,
  .card.stale .raw {
    opacity: 0.55;
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .title {
    background: none;
    border: none;
    padding: 0;
    color: var(--text);
    font-size: 17px;
    font-weight: 600;
    text-align: left;
    overflow-wrap: anywhere;
  }
  .title:hover {
    color: var(--accent);
  }
  .heard {
    color: var(--muted);
    white-space: nowrap;
  }
  .heard.quiet {
    color: var(--warn);
  }
  .metrics {
    display: grid;
    gap: 10px;
    margin-top: 6px;
  }
  .value {
    display: flex;
    align-items: baseline;
    gap: 4px;
  }
  .num {
    font-size: 30px;
    font-weight: 600;
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    color: var(--muted);
    font-size: 15px;
  }
  .label {
    color: var(--muted);
    margin-bottom: 2px;
  }
  .raw {
    margin: 6px 0;
    overflow-wrap: anywhere;
  }
  footer {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    color: var(--muted);
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .battery-low {
    color: var(--bad);
    font-weight: 500;
  }
  .edit {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .edit .danger {
    margin-left: auto;
  }
</style>
