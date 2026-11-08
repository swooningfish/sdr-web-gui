<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Device, type EventRow, type Pass, type ScheduleInfo } from '../lib/api';
  import { formatMHz, relTime } from '../lib/format';
  import { compass, countdown, duration } from '../lib/passes';
  import { imageUrl } from '../lib/sstv';
  import SkyPlot from './SkyPlot.svelte';

  let { isAdmin, onsettings }: { isAdmin: boolean; onsettings: () => void } = $props();

  let info = $state<ScheduleInfo | null>(null);
  let recordings = $state<EventRow[]>([]);
  let images = $state<EventRow[]>([]);
  let selected = $state<number | null>(null);
  let err = $state<string | null>(null);
  let now = $state(Date.now());

  const st = $derived(info?.status ?? null);
  const active = $derived(st?.active ?? null);
  const shown = $derived.by((): Pass | null => {
    if (!st) return null;
    if (selected !== null) return st.passes.find((p) => p.aos_ms === selected) ?? null;
    return st.passes[0] ?? null;
  });
  const rec = (e: EventRow) =>
    e.data as { id: string; aos_ms: number; los_ms: number; max_el_deg: number; test: boolean; audio?: string; seconds?: number };
  /** SSTV images received during a pass (plus a minute either side). */
  const passImages = (e: EventRow) => {
    const r = rec(e);
    return images.filter((i) => i.ts >= r.aos_ms - 60_000 && i.ts <= r.los_ms + 120_000);
  };
  const when = (ms: number) =>
    new Date(ms).toLocaleString(undefined, { weekday: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

  /** Live events from App: pass logs and SSTV images (for the thumbnails). */
  export function push(d: Device, ev: EventRow) {
    if (d.decoder === 'sstv') images = [ev, ...images];
    else recordings = [ev, ...recordings];
  }

  async function refresh() {
    try {
      info = await api.schedule();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    refresh();
    Promise.all([api.events(100, 'passes'), api.events(300, 'sstv')]).then(
      ([r, i]) => {
        recordings = r;
        images = i;
      },
      () => {},
    );
    const t = setInterval(() => {
      now = Date.now();
      refresh();
    }, 2000);
    return () => clearInterval(t);
  });

  async function remove(e: EventRow) {
    if (!confirm('Delete this pass recording? This cannot be undone.')) return;
    try {
      await api.deletePass(e.id);
      recordings = recordings.filter((x) => x.id !== e.id);
    } catch (x) {
      err = x instanceof Error ? x.message : String(x);
    }
  }
</script>

<div class="passes">
  <section class="panel">
    <div class="head">
      <h2>Passes</h2>
      {#if st}
        <span class="pill small-text">
          <span class="dot" class:ok={st.mode !== 'manual'}></span>
          {#if st.mode === 'pass' && active}
            Tracking {active.satellite}{active.test ? ' (test)' : ''}
          {:else if st.mode === 'rotation'}
            Rotating: {st.profile ?? '…'}{st.until_ms ? `, next switch ${countdown(st.until_ms, now)}` : ''}
            {st.holding ? ' (holding for an SSTV image)' : ''}
          {:else}
            Manual tuning
          {/if}
        </span>
      {/if}
    </div>
    {#if err}<p class="error">{err}</p>{/if}
    {#if st && !st.location}
      <p class="callout small-text">
        Set the station's location to predict passes.
        {#if isAdmin}<button type="button" class="small" onclick={onsettings}>Open settings</button>{/if}
      </p>
    {/if}
    {#if st}
      <p class="muted small-text tles">
        {#each st.tles as t (t.norad_id)}
          <span>
            {t.name}:
            {#if t.epoch_ms}TLE epoch {relTime(t.epoch_ms, now)}{t.source === 'manual' ? ' (entered by hand)' : ''}{:else}no
              TLE yet{/if}{#if t.error}<span class="error-text"> · {t.error}</span>{/if}
          </span>
        {/each}
      </p>
    {/if}

    <div class="now">
      <div class="plot">
        {#if active}
          <SkyPlot
            track={active.track}
            now={active.look ? { az: active.look.az_deg, el: active.look.el_deg } : null}
            label="Sky track of the {active.satellite} pass in progress"
          />
        {:else if shown}
          <SkyPlot track={shown.track} label="Sky track of the {shown.satellite} pass {when(shown.aos_ms)}" />
        {:else}
          <SkyPlot track={[]} label="Empty sky plot" />
        {/if}
      </div>
      <div class="readout">
        {#if active}
          <h3>{active.satellite} {active.test ? '(test pass)' : 'overhead'}</h3>
          <dl>
            {#if active.look}
              <dt>Azimuth</dt><dd>{active.look.az_deg.toFixed(1)}° {compass(active.look.az_deg)}</dd>
              <dt>Elevation</dt><dd>{active.look.el_deg.toFixed(1)}°</dd>
              <dt>Range</dt><dd>{Math.round(active.look.range_km)} km</dd>
            {/if}
            {#if active.doppler_hz !== null}
              <dt>Doppler</dt>
              <dd>{active.doppler_hz > 0 ? '+' : ''}{(active.doppler_hz / 1000).toFixed(2)} kHz at {formatMHz(active.record_hz, 3)}</dd>
            {/if}
            <dt>Ends</dt><dd>{countdown(active.los_ms, now)}</dd>
            <dt>Recording</dt><dd>{active.recording ? `yes, ${formatMHz(active.record_hz, 3)} FM` : 'no'}</dd>
          </dl>
        {:else if shown}
          <h3>{selected === null ? 'Next pass' : 'Selected pass'}: {shown.satellite}</h3>
          <dl>
            <dt>Starts</dt><dd>{when(shown.aos_ms)} ({countdown(shown.aos_ms, now)})</dd>
            <dt>Duration</dt><dd>{duration(shown.los_ms - shown.aos_ms)}</dd>
            <dt>Max elevation</dt><dd>{shown.max_el_deg.toFixed(0)}°</dd>
            <dt>Path</dt><dd>{compass(shown.aos_az_deg)} → {compass(shown.los_az_deg)}</dd>
          </dl>
        {:else if st?.location}
          <p class="muted small-text">No passes predicted in the next 48 hours.</p>
        {/if}
      </div>
    </div>
  </section>

  <section class="panel">
    <h2>Upcoming passes</h2>
    {#if st?.passes.length}
      <div class="table-wrap">
        <table>
          <thead>
            <tr><th>Satellite</th><th>Starts</th><th></th><th class="num">Duration</th><th class="num">Max el</th><th>Path</th></tr>
          </thead>
          <tbody>
            {#each st.passes as p (p.satellite + p.aos_ms)}
              <tr
                class:sel={selected === p.aos_ms}
                class:live={active && !active.test && active.aos_ms === p.aos_ms}
                onclick={() => (selected = selected === p.aos_ms ? null : p.aos_ms)}
              >
                <td><button type="button" class="link" onclick={(e) => { e.stopPropagation(); selected = p.aos_ms; }}>{p.satellite}</button></td>
                <td class="nowrap">{when(p.aos_ms)}</td>
                <td class="muted nowrap">{countdown(p.aos_ms, now)}</td>
                <td class="num nowrap">{duration(p.los_ms - p.aos_ms)}</td>
                <td class="num">{p.max_el_deg.toFixed(0)}°</td>
                <td class="nowrap">{compass(p.aos_az_deg)} → {compass(p.los_az_deg)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <p class="muted small-text">{st?.location ? 'Nothing above the minimum elevation in the next 48 hours.' : 'No location set.'}</p>
    {/if}
  </section>

  <section class="panel">
    <h2>Recordings</h2>
    {#if recordings.length === 0}
      <p class="muted small-text">Each pass is logged here, with its audio when recording is on.</p>
    {/if}
    <ul class="recs">
      {#each recordings as e (e.id)}
        {@const r = rec(e)}
        {@const imgs = passImages(e)}
        <li>
          <div class="rec-head">
            <strong>{r.id}</strong>
            <span class="muted small-text">{when(r.aos_ms)} · max {Math.round(r.max_el_deg)}°{r.seconds ? ` · ${duration(r.seconds * 1000)}` : ''}</span>
            {#if r.test}<span class="badge small-text">test</span>{/if}
            <span class="actions">
              {#if r.audio}<a class="button small" href="/api/passes/{e.id}/audio" download>Download</a>{/if}
              {#if isAdmin}<button type="button" class="small danger" onclick={() => remove(e)}>Delete</button>{/if}
            </span>
          </div>
          {#if r.audio}
            <audio controls preload="none" src="/api/passes/{e.id}/audio"></audio>
          {/if}
          {#if imgs.length}
            <div class="thumbs">
              {#each imgs as i (i.id)}
                <a href={imageUrl(i.id)} target="_blank" rel="noopener"
                  ><img src={imageUrl(i.id, true)} alt="{String(i.data.mode)} SSTV from this pass" width="96" loading="lazy" /></a
                >
              {/each}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
</div>

<style>
  .passes {
    display: grid;
    /* minmax(0, …) so the wide pass table scrolls instead of widening the page. */
    grid-template-columns: minmax(0, 1fr);
    gap: 16px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 6px 12px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .callout {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--warn);
    border-radius: 8px;
  }
  .tles {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    margin: 4px 0 8px;
  }
  .error-text {
    color: var(--bad);
  }
  .now {
    display: grid;
    grid-template-columns: minmax(200px, 300px) 1fr;
    gap: 16px;
    align-items: center;
  }
  h3 {
    margin: 0 0 8px;
    font-size: 15px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 14px;
    margin: 0;
    font-size: 14px;
    font-variant-numeric: tabular-nums;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
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
  }
  th {
    color: var(--muted);
    font-weight: 500;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover,
  tr.sel {
    background: var(--surface-2);
  }
  tr.live td:first-child {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .nowrap {
    white-space: nowrap;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font-weight: 500;
  }
  .recs {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .recs li {
    padding: 10px 0;
    border-bottom: 1px solid var(--border);
    display: grid;
    gap: 6px;
  }
  .rec-head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 10px;
  }
  .badge {
    padding: 0 7px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 6px;
  }
  audio {
    width: 100%;
    max-width: 520px;
    height: 36px;
  }
  .thumbs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .thumbs img {
    display: block;
    border-radius: 4px;
    height: auto;
  }
  a.button {
    display: inline-block;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 13px;
    text-decoration: none;
  }
  @media (max-width: 640px) {
    .now {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
