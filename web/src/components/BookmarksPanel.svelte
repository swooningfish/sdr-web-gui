<script lang="ts">
  import { onMount } from 'svelte';
  import { formatMHz, relTime } from '../lib/format';
  import type { Listen } from '../lib/audio';
  import { api, type RepeaterList } from '../lib/api';
  import { CALLING } from '../lib/calling';
  import { bookmarks, colourOf, matches, type Bookmark, type BookmarkInput, type Scope } from '../lib/bookmarks.svelte';
  import BookmarkEditor from './BookmarkEditor.svelte';

  /** Bookmarks: the saved list, standard calling frequencies and nearby
   * repeaters. Find one and click to listen. */
  let {
    listen,
    isAdmin,
    onselect,
    onmanage,
    bare = false,
  }: {
    listen: Listen;
    isAdmin: boolean;
    onselect: (b: Bookmark) => void;
    onmanage: () => void;
    /** Inside another panel: no frame or heading of its own. */
    bare?: boolean;
  } = $props();

  type Source = 'saved' | 'calling' | 'repeaters';
  const SRC_KEY = 'sdr.bookmarks.source';
  function savedSource(): Source {
    try {
      const v = localStorage.getItem(SRC_KEY);
      return v === 'calling' || v === 'repeaters' ? v : 'saved';
    } catch {
      return 'saved';
    }
  }
  let source = $state<Source>(savedSource());

  let q = $state('');
  let scope = $state<Scope>('all');
  let group = $state('');
  let adding = $state<BookmarkInput | null>(null);
  let saved = $state<string | null>(null);
  let rpt = $state<RepeaterList | null>(null);

  function pick(s: Source) {
    source = s;
    group = '';
    try {
      localStorage.setItem(SRC_KEY, s);
    } catch {
      /* private mode */
    }
    if (s === 'repeaters') loadRepeaters();
  }

  async function loadRepeaters() {
    try {
      rpt = await api.repeaters();
    } catch (e) {
      rpt = { enabled: true, error: e instanceof Error ? e.message : String(e), repeaters: [] };
    }
  }
  onMount(() => {
    loadRepeaters();
    // Check again soon while RepeaterBook is being asked, else every 10 minutes.
    let t: ReturnType<typeof setTimeout>;
    const again = () => {
      t = setTimeout(
        async () => {
          await loadRepeaters();
          again();
        },
        rpt?.fetching ? 4000 : 600_000,
      );
    };
    again();
    return () => clearTimeout(t);
  });

  /** Built-in rows look like bookmarks, so the list and onselect treat them alike. */
  function pseudo(
    id: number,
    name: string,
    freq_hz: number,
    mode: Bookmark['mode'],
    group: string,
    notes: string | null,
    colour: string,
  ): Bookmark {
    return { id, name, freq_hz, mode, group, colour, notes, shared: true, can_edit: false, updated_at: 0 };
  }
  const calling = CALLING.map((c, i) => pseudo(-1 - i, c.name, c.freq_hz, c.mode, c.group, null, '#a3e635'));
  const repeaters = $derived(
    (rpt?.repeaters ?? []).map((r, i) =>
      pseudo(
        -1000 - i,
        `${r.callsign}${r.city ? ` · ${r.city}` : ''}`,
        r.freq_hz,
        'nfm',
        r.mode,
        [
          r.input_hz ? `in ${(r.input_hz / 1e6).toFixed(4)}` : null,
          r.tone ? `CTCSS ${r.tone}` : null,
          `${r.distance_mi} mi`,
          r.status,
        ]
          .filter(Boolean)
          .join(' · '),
        '#fb923c',
      ),
    ),
  );
  const pool = $derived(source === 'saved' ? bookmarks.list : source === 'calling' ? calling : repeaters);
  const groups = $derived(
    source === 'saved' ? bookmarks.groups : [...new Set(pool.map((b) => b.group).filter((g): g is string => !!g))],
  );
  const shown = $derived(pool.filter((b) => matches(b, q, source === 'saved' ? scope : 'all', group)));
  const hasShared = $derived(bookmarks.list.some((b) => b.shared));
  const hasMine = $derived(bookmarks.list.some((b) => !b.shared));

  function startAdd() {
    saved = null;
    adding = { name: '', freq_hz: listen.hz, mode: listen.mode, group: null, colour: null, notes: null, shared: isAdmin };
  }

  /** Keep a calling frequency or repeater as a real bookmark. */
  function keep(b: Bookmark) {
    saved = null;
    adding = {
      name: (source === 'calling' ? `${b.group} ${b.name}` : b.name).slice(0, 64),
      freq_hz: b.freq_hz,
      mode: b.mode,
      group: source === 'calling' ? 'Calling' : 'Repeaters',
      colour: null,
      notes: b.notes,
      shared: isAdmin,
    };
  }

  async function save(b: BookmarkInput) {
    await bookmarks.add(b);
    adding = null;
    saved = `Saved ${b.name}`;
  }
</script>

<section class:panel={!bare} class:bare>
  <div class="head">
    {#if !bare}<h2>Bookmarks</h2>{/if}
    <div class="sources" role="group" aria-label="Which list">
      <button type="button" class="small" class:active={source === 'saved'} aria-pressed={source === 'saved'} onclick={() => pick('saved')}
        >Saved</button
      >
      <button
        type="button"
        class="small"
        class:active={source === 'calling'}
        aria-pressed={source === 'calling'}
        title="Standard calling frequencies (IARU Region 1 / UK band plans)"
        onclick={() => pick('calling')}>Calling</button
      >
      <button
        type="button"
        class="small"
        class:active={source === 'repeaters'}
        aria-pressed={source === 'repeaters'}
        title="Repeaters near the station, from RepeaterBook"
        onclick={() => pick('repeaters')}>Repeaters</button
      >
    </div>
    <div class="tools">
      <button
        type="button"
        class="small"
        onclick={startAdd}
        disabled={!!adding}
        title="Bookmark {formatMHz(listen.hz, 4)} ({listen.mode.toUpperCase()})">★ Save</button
      >
      <button type="button" class="small icon" aria-label="Manage bookmarks" title="Manage bookmarks" onclick={onmanage}
        ><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"
          ><path
            fill="currentColor"
            d="M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.49.49 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.48.48 0 0 0-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96a.48.48 0 0 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6a3.6 3.6 0 1 1 0-7.2 3.6 3.6 0 0 1 0 7.2z"
          /></svg
        ></button
      >
    </div>
  </div>

  {#if adding}
    <BookmarkEditor
      initial={adding}
      {isAdmin}
      title="Bookmark {formatMHz(adding.freq_hz, 4)}"
      onsave={save}
      oncancel={() => (adding = null)}
    />
  {/if}
  {#if saved}<p class="success small-text">{saved}</p>{/if}

  {#if source === 'repeaters'}
    {#if !rpt}
      <p class="muted small-text note">Loading…</p>
    {:else if !rpt.enabled}
      <p class="muted small-text note">
        Nearby repeaters come from RepeaterBook. {isAdmin
          ? 'Switch it on with your RepeaterBook API token in Settings → Integrations.'
          : 'An admin can switch this on.'}
      </p>
    {:else if rpt.error}
      <p class="small-text note error-text">{rpt.error}</p>
    {:else if rpt.fetching && !rpt.repeaters.length}
      <p class="muted small-text note">Fetching from RepeaterBook…</p>
    {:else if rpt.fetched_ms}
      <p class="muted small-text note">
        {rpt.repeaters.length} within {rpt.radius_miles} mi, updated {relTime(rpt.fetched_ms)}. Data courtesy of
        RepeaterBook.com.
      </p>
    {/if}
  {/if}

  {#if pool.length}
    <div class="filters">
      <input type="search" bind:value={q} placeholder="Search…" aria-label="Search bookmarks" />
      {#if source === 'saved' && hasShared && hasMine}
        <select bind:value={scope} aria-label="Which bookmarks">
          <option value="all">All</option>
          <option value="shared">Station</option>
          <option value="mine">Mine</option>
        </select>
      {/if}
      {#if groups.length}
        <select bind:value={group} aria-label="Group">
          <option value="">{source === 'saved' ? 'All groups' : source === 'calling' ? 'All bands' : 'All modes'}</option>
          {#each groups as g (g)}<option value={g}>{g}</option>{/each}
        </select>
      {/if}
    </div>
    <ul class="list" aria-label="Bookmarks">
      {#each shown as b (b.id)}
        <li>
          <button
            type="button"
            class="item"
            class:current={listen.hz === b.freq_hz && listen.mode === b.mode}
            onclick={() => onselect(b)}
            title={b.notes ?? ''}
          >
            <span class="swatch" style:background={colourOf(b)}></span>
            <span class="name"
              >{#if source === 'calling'}<span class="band">{b.group}</span>
              {/if}{b.name}{#if !b.shared}<span class="mine" title="Only you see this"> · mine</span
                >{/if}{#if source === 'repeaters' && b.notes}<span class="sub">{b.notes}</span>{/if}</span
            >
            <span class="freq">{(b.freq_hz / 1e6).toFixed(4)}</span>
            <span class="mode">{source === 'repeaters' ? b.group : b.mode.toUpperCase()}</span>
          </button>
          {#if b.id < 0}
            <button
              type="button"
              class="keep small"
              aria-label="Save {b.name} as a bookmark"
              title="Save as a bookmark"
              disabled={!!adding}
              onclick={() => keep(b)}>☆</button
            >
          {/if}
        </li>
      {:else}
        <li class="muted small-text empty">Nothing matches.</li>
      {/each}
    </ul>
  {:else if source === 'saved' && bookmarks.loaded && !adding}
    <p class="muted small-text note">No bookmarks yet. <strong>★ Save</strong> keeps the frequency you're tuned to.</p>
  {:else if source === 'repeaters' && rpt?.enabled && !rpt.error && !rpt.fetching && rpt.fetched_ms}
    <p class="muted small-text note">Nothing within {rpt.radius_miles} mi. Widen the radius in Settings → Integrations.</p>
  {/if}
  {#if bookmarks.error}<p class="error">{bookmarks.error}</p>{/if}
</section>

<style>
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .panel .head h2 {
    margin: 0;
  }
  .sources,
  .tools {
    display: flex;
    gap: 4px;
  }
  .sources .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .icon {
    display: inline-grid;
    place-items: center;
    padding: 4px 7px;
  }
  .note {
    margin: 8px 0 0;
  }
  .error-text {
    color: var(--bad);
  }
  .filters {
    display: flex;
    gap: 6px;
    margin: 10px 0 6px;
  }
  .filters input {
    flex: 1;
    min-width: 0;
    padding: 5px 9px;
  }
  .filters select {
    min-width: 0;
    max-width: 40%;
    padding: 5px 6px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 260px;
    overflow-y: auto;
    display: grid;
    gap: 2px;
  }
  li {
    display: flex;
    align-items: center;
    min-width: 0;
  }
  .item {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: 10px minmax(0, 1fr) auto 3.4em;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    text-align: left;
    background: transparent;
    border-color: transparent;
    font-size: 14px;
  }
  .item:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .item.current {
    background: var(--surface-2);
    border-color: var(--ok);
  }
  .keep {
    flex: none;
    padding: 2px 7px;
    background: transparent;
    border-color: transparent;
    color: var(--muted);
  }
  .keep:hover:not(:disabled) {
    color: var(--accent);
    border-color: var(--border);
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .band,
  .mine {
    color: var(--muted);
    font-size: 12px;
  }
  .sub {
    display: block;
    color: var(--muted);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .freq {
    font-variant-numeric: tabular-nums;
  }
  .mode {
    color: var(--muted);
    font-size: 12px;
    text-align: right;
  }
  .empty {
    padding: 6px 8px;
  }
  .success {
    margin: 6px 0 0;
  }
</style>
