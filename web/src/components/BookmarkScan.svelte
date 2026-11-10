<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ScanBand, type ScanFound, type ScanStatus } from '../lib/api';
  import { bookmarks } from '../lib/bookmarks.svelte';
  import { relTime } from '../lib/format';
  import { confirmAsk } from '../lib/confirm.svelte';

  /** Band scan in the bookmark manager: an admin steps the receiver through
   * broadcast FM or DAB, then anyone ticks the stations found and adds them
   * to the bookmarks. */
  let { isAdmin }: { isAdmin: boolean } = $props();

  let status = $state<ScanStatus | null>(null);
  let err = $state<string | null>(null);
  let msg = $state<string | null>(null);
  let busy = $state(false);
  let picked = $state<Set<string>>(new Set());
  // svelte-ignore state_referenced_locally
  let shared = $state(isAdmin);

  const running = $derived(!!status?.running);
  const found = $derived(status?.found ?? []);
  const fm = $derived(found.filter((f) => f.band === 'fm'));
  /** DAB stations by multiplex. */
  const muxes = $derived.by(() => {
    const m = new Map<string, ScanFound[]>();
    for (const f of found.filter((f) => f.band === 'dab')) {
      const key = `${f.channel ?? ''} · ${f.ensemble ?? ''}`;
      m.set(key, [...(m.get(key) ?? []), f]);
    }
    return [...m.entries()];
  });
  const addable = $derived(found.filter((f) => !f.saved));

  let timer: ReturnType<typeof setTimeout> | undefined;
  let wasRunning = false;

  async function load() {
    try {
      const s = await api.scan();
      // When a scan finishes, tick everything that isn't saved yet.
      if ((wasRunning && !s.running) || (!status && !s.running)) {
        picked = new Set(s.found.filter((f) => !f.saved).map((f) => f.id));
      }
      wasRunning = s.running;
      status = s;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
    clearTimeout(timer);
    if (status?.running) timer = setTimeout(load, 1000);
  }
  onMount(() => {
    load();
    return () => clearTimeout(timer);
  });

  async function start(band: ScanBand) {
    const what = band === 'fm' ? 'the FM band (87.5–108 MHz)' : 'DAB Band III (174–240 MHz)';
    if (
      !(await confirmAsk(
        `Scan ${what}? The receiver retunes for about a minute, for everyone listening, then goes back to where it is now.`,
        { ok: 'Scan' },
      ))
    )
      return;
    err = null;
    msg = null;
    busy = true;
    try {
      await api.startScan(band);
      wasRunning = true;
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function stop() {
    try {
      await api.stopScan();
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  function toggle(id: string) {
    const next = new Set(picked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    picked = next;
  }

  async function save() {
    const ids = [...picked].filter((id) => addable.some((f) => f.id === id));
    if (!ids.length) return;
    err = null;
    msg = null;
    busy = true;
    try {
      const r = await api.saveScan(ids, shared);
      await bookmarks.load();
      msg = `Added ${r.added} bookmark${r.added === 1 ? '' : 's'}${r.duplicates ? `, ${r.duplicates} already there` : ''}.`;
      picked = new Set();
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const mhz = (hz: number) => (hz / 1e6).toFixed(hz % 100_000 ? 3 : 1);
  const pickedCount = $derived(addable.filter((f) => picked.has(f.id)).length);
</script>

{#snippet row(f: ScanFound)}
  <li>
    <label class="row" class:saved={f.saved}>
      <input
        type="checkbox"
        checked={f.saved || picked.has(f.id)}
        disabled={f.saved || running}
        onchange={() => toggle(f.id)}
        aria-label="Add {f.name}"
      />
      <span class="name">{f.name}</span>
      <span class="freq">{mhz(f.freq_hz)}</span>
      <span class="meta muted">
        {#if f.saved}saved{:else if f.band === 'fm'}{f.ident ? `PI ${f.ident}` : 'no RDS'}{:else}SId {f.ident}{/if}
        · {Math.round(f.level_db)} dB
      </span>
    </label>
  </li>
{/snippet}

<section class="scan" aria-label="Scan for stations">
  <h3 class="sec-head">Scan for stations</h3>
  <p class="muted small-text">
    Finds broadcast FM stations (named from RDS) or DAB stations (named from each multiplex's station list) and adds the
    ones you tick to the bookmarks.
  </p>
  {#if isAdmin}
    <div class="buttons">
      {#if running}
        <button type="button" class="small danger" onclick={stop}>Stop scan</button>
      {:else}
        <button type="button" class="small" disabled={busy} onclick={() => start('fm')}>Scan FM</button>
        <button type="button" class="small" disabled={busy} onclick={() => start('dab')}>Scan DAB</button>
      {/if}
    </div>
  {:else if !status?.found.length && !running}
    <p class="muted small-text">An admin can run a scan; the stations it finds show here.</p>
  {/if}

  {#if status && (running || status.finished_ms)}
    <div class="state small-text" role="status" aria-live="polite">
      {#if running}
        <progress max="1" value={status.progress} aria-label="Scan progress"></progress>
        <span>{status.step}… {found.length ? `${found.length} found so far` : ''}</span>
      {:else}
        <span class="muted">
          {status.band === 'dab' ? 'DAB' : 'FM'} scan {status.error
            ? 'failed'
            : status.stopped
              ? status.step.toLowerCase()
              : 'finished'}
          {status.finished_ms ? relTime(status.finished_ms) : ''}{status.by ? ` (by ${status.by})` : ''}: {found.length} station{found.length ===
          1
            ? ''
            : 's'}.
        </span>
      {/if}
    </div>
  {/if}
  {#if status?.error}<p class="error">{status.error}</p>{/if}
  {#each status?.notes ?? [] as n, i (i)}<p class="muted small-text note">{n}</p>{/each}

  {#if found.length}
    {#if fm.length}
      <h4 class="small-text">FM <span class="muted">{fm.length}</span></h4>
      <ul class="list">
        {#each fm as f (f.id)}{@render row(f)}{/each}
      </ul>
    {/if}
    {#each muxes as [key, list] (key)}
      <h4 class="small-text">DAB {key} <span class="muted">{list.length}</span></h4>
      <ul class="list">
        {#each list as f (f.id)}{@render row(f)}{/each}
      </ul>
    {/each}
    {#if muxes.length}
      <p class="muted small-text">
        DAB stations are saved in DAB mode on their multiplex's frequency at 2.048 MS/s; picking one centres the receiver on
        the multiplex and plays that station.
      </p>
    {/if}

    {#if addable.length && !running}
      <div class="save">
        <button
          type="button"
          class="small"
          onclick={() => (picked = pickedCount === addable.length ? new Set() : new Set(addable.map((f) => f.id)))}
          >{pickedCount === addable.length ? 'Tick none' : 'Tick all'}</button
        >
        {#if isAdmin}
          <select bind:value={shared} aria-label="Add to which list">
            <option value={true}>Station list</option>
            <option value={false}>My bookmarks</option>
          </select>
        {/if}
        <button type="button" class="small primary" disabled={!pickedCount || busy} onclick={save}
          >Add {pickedCount} to bookmarks</button
        >
      </div>
    {:else if !running}
      <p class="muted small-text">All of these are in your bookmarks.</p>
    {/if}
  {/if}
  {#if msg}<p class="success small-text">{msg}</p>{/if}
  {#if err}<p class="error">{err}</p>{/if}
</section>

<style>
  .scan {
    margin-top: 18px;
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .sec-head {
    margin: 8px 0 4px;
    font-size: 14px;
  }
  .buttons,
  .save {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 8px 0;
  }
  .save select {
    padding: 3px 8px;
    font-size: 13px;
  }
  .state {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
  }
  progress {
    flex: 0 1 160px;
    accent-color: var(--accent);
  }
  .note {
    margin: 4px 0;
  }
  h4 {
    margin: 12px 0 4px;
    font-weight: 600;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    max-height: 320px;
    overflow-y: auto;
  }
  .list li + li {
    border-top: 1px solid var(--border);
  }
  .row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-areas: 'box name freq' 'box meta meta';
    column-gap: 10px;
    align-items: center;
    padding: 6px 10px;
    cursor: pointer;
  }
  .row.saved {
    cursor: default;
  }
  .row input {
    grid-area: box;
  }
  .name {
    grid-area: name;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }
  .freq {
    grid-area: freq;
    font-variant-numeric: tabular-nums;
    font-size: 13px;
  }
  .meta {
    grid-area: meta;
    font-size: 12px;
  }
  .saved .name {
    color: var(--muted);
  }
</style>
