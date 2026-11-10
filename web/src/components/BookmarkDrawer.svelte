<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { formatMHz, formatRate, relTime } from '../lib/format';
  import type { Listen } from '../lib/audio';
  import { api, type RadioState, type RepeaterList } from '../lib/api';
  import { CALLING } from '../lib/calling';
  import {
    bookmarks,
    colourOf,
    inputOf,
    matches,
    type Bookmark,
    type BookmarkInput,
    type ImportResult,
    type Scope,
  } from '../lib/bookmarks.svelte';
  import { fkeys, SLOTS } from '../lib/fkeys.svelte';
  import { Selection } from '../lib/selection.svelte';
  import BookmarkEditor from './BookmarkEditor.svelte';
  import BookmarkScan from './BookmarkScan.svelte';
  import { confirmAsk } from '../lib/confirm.svelte';

  /** The bookmark manager: slides out over the page. Bookmarks are tiles:
   * click one to listen. Manage mode edits, deletes, imports and exports,
   * and any tile can be put on an F-key. */
  let {
    open = $bindable(false),
    assign = $bindable(null),
    adding = $bindable(false),
    listen,
    isAdmin,
    radio = null,
    onselect,
  }: {
    open?: boolean;
    /** Picking a bookmark for this F-key (1–8). */
    assign?: number | null;
    /** Open with the add form for what's being heard. */
    adding?: boolean;
    listen: Listen;
    isAdmin: boolean;
    radio?: RadioState | null;
    onselect: (b: Bookmark) => void;
  } = $props();

  type Source = 'saved' | 'calling' | 'repeaters';
  const PREFS = 'sdr.bookmarks.drawer';
  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem(PREFS) ?? '{}');
    } catch {
      return {};
    }
  })();
  let source = $state<Source>(['calling', 'repeaters'].includes(saved.source) ? saved.source : 'saved');
  let grouped = $state<boolean>(saved.grouped ?? true);
  $effect(() => {
    try {
      localStorage.setItem(PREFS, JSON.stringify({ source, grouped }));
    } catch {
      /* private mode */
    }
  });

  let q = $state('');
  let scope = $state<Scope>('all');
  let group = $state('');
  let manage = $state(false);
  let editing = $state<{ id: number | null; input: BookmarkInput } | null>(null);
  let msg = $state<string | null>(null);
  let err = $state<string | null>(null);
  let rpt = $state<RepeaterList | null>(null);
  /** Tile whose F-key picker is open (manage mode). */
  let picking = $state<number | null>(null);
  const sel = new Selection();

  // svelte-ignore state_referenced_locally
  let importShared = $state(isAdmin);
  let importing = $state(false);
  let importResult = $state<ImportResult | null>(null);
  let fileInput = $state<HTMLInputElement | undefined>();
  let closeBtn = $state<HTMLButtonElement | undefined>();
  let returnFocus: HTMLElement | null = null;

  function pick(s: Source) {
    source = s;
    group = '';
    if (s !== 'saved') manage = false;
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

  /** Built-in rows look like bookmarks, so the tiles treat them alike. */
  function pseudo(
    id: number,
    name: string,
    freq_hz: number,
    mode: Bookmark['mode'],
    group: string,
    notes: string | null,
    colour: string,
  ): Bookmark {
    return {
      id,
      name,
      freq_hz,
      mode,
      group,
      colour,
      notes,
      sample_rate: null,
      service_id: null,
      shared: true,
      can_edit: false,
      updated_at: 0,
    };
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
  /** Tiles in sections by group (or band / repeater mode). */
  const sections = $derived.by(() => {
    if (!grouped || group) return [{ name: '', items: shown }];
    const m = new Map<string, Bookmark[]>();
    for (const b of shown) {
      const k = b.group ?? '';
      if (!m.has(k)) m.set(k, []);
      m.get(k)!.push(b);
    }
    return [...m.entries()]
      .sort(([a], [b]) => (a === '' ? 1 : b === '' ? -1 : source === 'calling' ? 0 : a.localeCompare(b)))
      .map(([name, items]) => ({ name: name || 'No group', items }));
  });
  const editable = $derived(shown.filter((b) => b.can_edit).map((b) => b.id));
  const hasShared = $derived(bookmarks.list.some((b) => b.shared));
  const hasMine = $derived(bookmarks.list.some((b) => !b.shared));

  $effect(() => sel.keepOnly(bookmarks.list.filter((b) => b.can_edit).map((b) => b.id)));

  // Opening: remember where focus was, and start in the right state.
  $effect(() => {
    if (!open) return;
    returnFocus = document.activeElement as HTMLElement | null;
    msg = null;
    err = null;
    tick().then(() => closeBtn?.focus());
  });
  $effect(() => {
    if (open && adding) {
      adding = false;
      startAdd();
    }
  });
  $effect(() => {
    if (open && assign !== null) manage = false;
  });

  function close() {
    open = false;
    assign = null;
    editing = null;
    picking = null;
    returnFocus?.focus?.();
  }

  function startAdd() {
    msg = null;
    source = 'saved';
    editing = {
      id: null,
      input: {
        name: '',
        freq_hz: listen.hz,
        mode: listen.mode,
        group: null,
        colour: null,
        notes: null,
        sample_rate: rateNow,
        service_id: listen.mode === 'dab' ? (listen.service ?? null) : null,
        shared: isAdmin,
      },
    };
  }

  /** The receiver's sample rate now (saved with new bookmarks). */
  const rateNow = $derived(radio?.settings.sample_rate ?? null);
  const tunedNow = $derived({ hz: listen.hz, mode: listen.mode, rate: rateNow });

  /** A bookmark you can change on the frequency you're tuned to, whose mode
   * or sample rate differs from what you're using: offer to update it. */
  const stale = $derived(
    bookmarks.list.find(
      (b) =>
        b.can_edit &&
        b.freq_hz === listen.hz &&
        // Several DAB stations share a multiplex: only this station's.
        !(b.mode === 'dab' && listen.mode === 'dab' && b.service_id !== (listen.service ?? null)) &&
        (b.mode !== listen.mode || (rateNow !== null && b.sample_rate !== rateNow)),
    ) ?? null,
  );

  /** Overwrite a bookmark's frequency, mode and sample rate with what's tuned now. */
  async function updateToCurrent(b: Bookmark, ask: boolean) {
    const what = `${formatMHz(listen.hz, 4)} ${listen.mode.toUpperCase()}${rateNow ? ` at ${formatRate(rateNow)}` : ''}`;
    if (ask && !(await confirmAsk(`Update ${b.name} to ${what}?`, { ok: 'Update' }))) return;
    msg = null;
    err = null;
    try {
      await bookmarks.update(b.id, {
        ...inputOf(b),
        freq_hz: listen.hz,
        mode: listen.mode,
        sample_rate: rateNow ?? b.sample_rate,
      });
      msg = `Updated ${b.name} to ${what}.`;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  /** Keep a calling frequency or repeater as a real bookmark. */
  function keep(b: Bookmark) {
    msg = null;
    editing = {
      id: null,
      input: {
        name: (source === 'calling' ? `${b.group} ${b.name}` : b.name).slice(0, 64),
        freq_hz: b.freq_hz,
        mode: b.mode,
        group: source === 'calling' ? 'Calling' : 'Repeaters',
        colour: null,
        notes: b.notes,
        sample_rate: null,
        shared: isAdmin,
      },
    };
  }

  function edit(b: Bookmark) {
    msg = null;
    editing = { id: b.id, input: inputOf(b) };
  }

  async function save(input: BookmarkInput) {
    if (!editing) return;
    if (editing.id === null) await bookmarks.add(input);
    else await bookmarks.update(editing.id, input);
    msg = `Saved ${input.name}.`;
    editing = null;
  }

  async function remove(ids: number[], what: string) {
    if (!(await confirmAsk(`Delete ${what}? This can't be undone.`, { ok: 'Delete', danger: true }))) return;
    err = null;
    try {
      const r = await bookmarks.remove(ids);
      msg = `Deleted ${r.deleted} bookmark${r.deleted === 1 ? '' : 's'}.`;
      sel.clear();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function putOnKey(slot: number, b: Bookmark) {
    err = null;
    try {
      await fkeys.assign(slot, b);
      msg = `F${slot} now recalls ${b.name}.`;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function tileClick(b: Bookmark, e: MouseEvent) {
    if (assign !== null) {
      const slot = assign;
      await putOnKey(slot, b);
      assign = null;
      if (!err) close();
      return;
    }
    if (manage && b.can_edit) {
      sel.toggle(b.id, e.shiftKey, editable);
      return;
    }
    onselect(b);
  }

  async function importFile() {
    const f = fileInput?.files?.[0];
    if (!f) return;
    importing = true;
    importResult = null;
    err = null;
    try {
      if (f.size > 1_000_000) throw new Error('That file is over 1 MB.');
      importResult = await bookmarks.import(await f.text(), importShared);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      importing = false;
      if (fileInput) fileInput.value = '';
    }
  }

  const keysOf = (b: Bookmark) =>
    b.id > 0
      ? fkeys.slotsOf(b.id)
      : fkeys.keys.filter((k) => k && !k.bookmark_id && k.freq_hz === b.freq_hz && k.mode === b.mode).map((k) => k!.slot);
</script>

<svelte:window
  onkeydown={(e) => {
    if (open && e.key === 'Escape' && !editing) {
      e.preventDefault();
      close();
    }
  }}
/>

<div class="scrim" class:open onclick={close} aria-hidden="true"></div>
<aside class="drawer" class:open aria-label="Bookmarks" aria-hidden={!open} inert={!open}>
  <header class="head">
    <h2>Bookmarks</h2>
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
    <button type="button" class="small close" bind:this={closeBtn} aria-label="Close bookmarks" onclick={close}>✕</button>
  </header>

  {#if assign !== null}
    <div class="assign-bar small-text" role="status">
      <span>Pick a bookmark for <strong>F{assign}</strong></span>
      <button type="button" class="small" onclick={() => (assign = null)}>Cancel</button>
    </div>
  {/if}

  <div class="toolbar">
    <input type="search" bind:value={q} placeholder="Search name, group, notes or MHz…" aria-label="Search bookmarks" />
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
  <div class="actions">
    <button
      type="button"
      class="small primary"
      onclick={startAdd}
      disabled={!!editing}
      title="Bookmark {formatMHz(listen.hz, 4)} ({listen.mode.toUpperCase()})">★ Save {formatMHz(listen.hz, 4)}</button
    >
    {#if stale}
      <button
        type="button"
        class="small"
        onclick={() => updateToCurrent(stale!, false)}
        disabled={!!editing}
        title="Save {listen.mode.toUpperCase()}{rateNow ? ` and ${formatRate(rateNow)}` : ''} into {stale.name} (now {stale.mode.toUpperCase()}{stale.sample_rate
          ? ` at ${formatRate(stale.sample_rate)}`
          : ''})">↻ Update {stale.name}</button
      >
    {/if}
    <label class="small-text check"><input type="checkbox" bind:checked={grouped} /> By group</label>
    {#if source === 'saved'}
      <button
        type="button"
        class="small manage-btn"
        class:active={manage}
        aria-pressed={manage}
        disabled={assign !== null}
        onclick={() => {
          manage = !manage;
          picking = null;
          sel.clear();
        }}>{manage ? 'Done' : 'Manage'}</button
      >
    {/if}
  </div>

  <div class="body">
    {#if editing}
      <BookmarkEditor
        initial={editing.input}
        {isAdmin}
        sampleRates={radio?.sample_rates ?? []}
        current={tunedNow}
        title={editing.id === null ? `Bookmark ${formatMHz(editing.input.freq_hz, 4)}` : `Edit ${editing.input.name}`}
        onsave={save}
        oncancel={() => (editing = null)}
      />
    {/if}
    {#if msg}<p class="success small-text">{msg}</p>{/if}
    {#if err}<p class="error">{err}</p>{/if}

    {#if manage && sel.size}
      <div class="bulk small-text" role="toolbar" aria-label="Selected bookmarks">
        <strong>{sel.size} selected</strong>
        <button type="button" class="small danger" onclick={() => remove(sel.list(), `${sel.size} bookmark${sel.size === 1 ? '' : 's'}`)}
          >Delete</button
        >
        <button type="button" class="small" onclick={() => sel.clear()}>Clear</button>
      </div>
    {:else if manage}
      <p class="muted small-text hint">Click tiles to select them; ✎ edits one and F puts it on a function key.</p>
    {/if}

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
      <div class="sections">
      {#each sections as s (s.name)}
        <section class="section" class:wide={s.items.length >= 4 || sections.length === 1} style:--n={s.items.length}>
          {#if s.name}<h3 class="sec-head">{s.name} <span class="muted">{s.items.length}</span></h3>{/if}
          <ul class="tiles" aria-label={s.name ? `Bookmarks: ${s.name}` : 'Bookmarks'}>
            {#each s.items as b (b.id)}
              {@const keys = keysOf(b)}
              {@const current =
                listen.hz === b.freq_hz &&
                listen.mode === b.mode &&
                (b.mode !== 'dab' || b.service_id == null || b.service_id === listen.service)}
              <li class="tile-cell">
                <button
                  type="button"
                  class="tile"
                  class:current
                  class:picked={manage && sel.has(b.id)}
                  class:assigning={assign !== null}
                  style:--sw={colourOf(b)}
                  aria-pressed={manage && b.can_edit ? sel.has(b.id) : undefined}
                  title={[b.name, formatMHz(b.freq_hz, 4), b.sample_rate ? `Sample rate ${formatRate(b.sample_rate)}` : '', b.notes]
                    .filter(Boolean)
                    .join('\n')}
                  onclick={(e) => tileClick(b, e)}
                >
                  <span class="t-name">{b.name}</span>
                  <span class="t-freq">{(b.freq_hz / 1e6).toFixed(4)}</span>
                  <span class="t-meta">
                    <span>{source === 'repeaters' ? b.group : b.mode.toUpperCase()}</span>
                    {#if source === 'saved' && !b.shared}<span class="mine" title="Only you see this">mine</span>{/if}
                    {#each keys as k (k)}<span class="fbadge" title="On F{k}">F{k}</span>{/each}
                  </span>
                </button>
                {#if manage}
                  <div class="t-tools">
                    {#if b.can_edit}
                      <button type="button" class="tool" aria-label="Edit {b.name}" title="Edit" onclick={() => edit(b)}>✎</button>
                      <button
                        type="button"
                        class="tool"
                        aria-label="Update {b.name} to what's tuned now"
                        title="Update to what's tuned now ({formatMHz(listen.hz, 4)} {listen.mode.toUpperCase()}{rateNow
                          ? `, ${formatRate(rateNow)}`
                          : ''})"
                        disabled={!!editing}
                        onclick={() => updateToCurrent(b, true)}>↻</button
                      >
                    {/if}
                    <button
                      type="button"
                      class="tool"
                      aria-label="Put {b.name} on a function key"
                      title="Put on an F-key"
                      aria-expanded={picking === b.id}
                      onclick={() => (picking = picking === b.id ? null : b.id)}>F</button
                    >
                  </div>
                {:else if b.id < 0 && assign === null}
                  <div class="t-tools">
                    <button
                      type="button"
                      class="tool"
                      aria-label="Save {b.name} as a bookmark"
                      title="Save as a bookmark"
                      disabled={!!editing}
                      onclick={() => keep(b)}>☆</button
                    >
                  </div>
                {/if}
                {#if picking === b.id}
                  <div class="fpick" role="group" aria-label="Function key for {b.name}">
                    {#each Array.from({ length: SLOTS }, (_, i) => i + 1) as slot (slot)}
                      <button
                        type="button"
                        class="small"
                        class:active={keys.includes(slot)}
                        title={fkeys.keys[slot - 1] ? `Replace ${fkeys.keys[slot - 1]!.name}` : 'Empty'}
                        onclick={async () => {
                          await putOnKey(slot, b);
                          picking = null;
                        }}>F{slot}</button
                      >
                    {/each}
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {:else}
        <p class="muted small-text note">Nothing matches.</p>
      {/each}
      </div>
    {:else if source === 'saved' && bookmarks.loaded && !editing}
      <p class="muted small-text note">No bookmarks yet. <strong>★ Save</strong> keeps the frequency you're tuned to.</p>
    {:else if source === 'repeaters' && rpt?.enabled && !rpt.error && !rpt.fetching && rpt.fetched_ms}
      <p class="muted small-text note">Nothing within {rpt.radius_miles} mi. Widen the radius in Settings → Integrations.</p>
    {/if}
    {#if bookmarks.error}<p class="error">{bookmarks.error}</p>{/if}

    {#if manage}
      <BookmarkScan {isAdmin} />
      <section class="io">
        <h3 class="sec-head">Import and export</h3>
        <div class="links">
          <span class="small-text muted">Export CSV:</span>
          <a class="button small" href="/api/bookmarks/export.csv?scope=all" download>All</a>
          <a class="button small" href="/api/bookmarks/export.csv?scope=shared" download>Station list</a>
          <a class="button small" href="/api/bookmarks/export.csv?scope=mine" download>Mine</a>
        </div>
        <p class="muted small-text">
          Import our own export, a CHIRP CSV or a gqrx <code>bookmarks.csv</code>. Bookmarks already there (same name
          and frequency) are skipped.
        </p>
        {#if isAdmin}
          <fieldset>
            <legend class="small-text muted">Into</legend>
            <label><input type="radio" bind:group={importShared} value={true} /> Station list</label>
            <label><input type="radio" bind:group={importShared} value={false} /> My bookmarks</label>
          </fieldset>
        {/if}
        <input
          type="file"
          accept=".csv,.txt,text/csv"
          bind:this={fileInput}
          aria-label="CSV file to import"
          onchange={importFile}
          disabled={importing}
        />
        {#if importing}<p class="muted small-text">Importing…</p>{/if}
        {#if importResult}
          <p class="success small-text">
            Added {importResult.added}{importResult.duplicates ? `, skipped ${importResult.duplicates} already there` : ''}.
          </p>
          {#if importResult.errors.length}
            <details class="small-text">
              <summary>{importResult.errors.length} line{importResult.errors.length === 1 ? '' : 's'} skipped</summary>
              <ul>
                {#each importResult.errors as e, i (i)}<li>{e}</li>{/each}
              </ul>
            </details>
          {/if}
        {/if}
      </section>
    {/if}
  </div>
</aside>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: rgba(2, 6, 23, 0.35);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s ease;
  }
  .scrim.open {
    opacity: 1;
    pointer-events: auto;
  }
  .drawer {
    position: fixed;
    z-index: 41;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(560px, 100vw);
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-left: 1px solid var(--border);
    box-shadow: -12px 0 32px rgba(0, 0, 0, 0.35);
    transform: translateX(100%);
    visibility: hidden;
    transition:
      transform 0.22s ease,
      visibility 0s linear 0.22s;
  }
  .drawer.open {
    transform: none;
    visibility: visible;
    transition: transform 0.22s ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .drawer,
    .scrim {
      transition: none;
    }
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px 8px;
  }
  .head h2 {
    margin: 0;
  }
  .sources {
    display: flex;
    gap: 4px;
  }
  .sources .active,
  .manage-btn.active,
  .fpick .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .close {
    margin-left: auto;
    padding: 4px 9px;
  }
  .assign-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0 14px 6px;
    padding: 6px 10px;
    border: 1px solid var(--warn);
    border-radius: 8px;
    background: color-mix(in srgb, var(--warn) 12%, transparent);
  }
  .toolbar,
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 0 14px 8px;
  }
  .toolbar input {
    flex: 1 1 200px;
    min-width: 0;
    padding: 5px 9px;
  }
  .toolbar select {
    min-width: 0;
    max-width: 40%;
    padding: 5px 6px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-left: auto;
    color: var(--muted);
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 4px 14px 20px;
    border-top: 1px solid var(--border);
  }
  /* Small groups sit side by side; big ones take the full width. */
  .sections {
    display: flex;
    flex-wrap: wrap;
    column-gap: 16px;
  }
  .section {
    flex: 0 0 auto;
    max-width: 100%;
    min-width: 0;
  }
  .section.wide {
    flex: 1 1 100%;
  }
  .section:not(.wide) .tiles {
    grid-template-columns: repeat(var(--n), minmax(96px, 124px));
  }
  .sec-head {
    margin: 10px 0 6px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .tiles {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(124px, 1fr));
    gap: 6px;
  }
  .tile-cell {
    position: relative;
    min-width: 0;
  }
  .tile {
    width: 100%;
    min-width: 0;
    height: 100%;
    display: grid;
    gap: 1px;
    padding: 6px 8px 6px 11px;
    text-align: left;
    border-radius: 6px;
    background:
      linear-gradient(90deg, var(--sw) 0 4px, transparent 4px),
      var(--surface-2);
    font-size: 13px;
  }
  .tile.current {
    border-color: var(--ok);
    box-shadow: inset 0 0 0 1px var(--ok);
  }
  .tile.picked {
    border-color: var(--accent);
    background:
      linear-gradient(90deg, var(--sw) 0 4px, transparent 4px),
      color-mix(in srgb, var(--accent) 18%, var(--surface-2));
  }
  .tile.assigning:hover {
    border-color: var(--warn);
  }
  .t-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-right: 18px;
  }
  .t-freq {
    font-variant-numeric: tabular-nums;
  }
  .t-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    font-size: 11px;
    color: var(--muted);
  }
  .mine {
    font-style: italic;
  }
  .fbadge {
    padding: 0 4px;
    border-radius: 3px;
    background: var(--led, #f59e0b);
    color: #111;
    font-weight: 700;
  }
  .t-tools {
    position: absolute;
    top: 3px;
    right: 3px;
    display: flex;
    gap: 2px;
  }
  .tool {
    padding: 0 5px;
    min-width: 20px;
    font-size: 12px;
    line-height: 18px;
    background: var(--surface);
  }
  .fpick {
    position: absolute;
    z-index: 2;
    top: 24px;
    right: 0;
    display: grid;
    grid-template-columns: repeat(4, auto);
    gap: 3px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 8px 20px rgba(0, 0, 0, 0.35);
  }
  .fpick button {
    padding: 3px 7px;
  }
  .bulk {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    margin: 10px 0 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .hint,
  .note {
    margin: 10px 0 4px;
  }
  .error-text {
    color: var(--bad);
  }
  .io {
    margin-top: 18px;
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .links {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  a.button {
    display: inline-block;
    padding: 4px 10px;
    font-size: 13px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text);
    text-decoration: none;
  }
  a.button:hover {
    border-color: var(--accent);
  }
  .io input[type='file'] {
    max-width: 100%;
  }
  fieldset {
    border: none;
    padding: 0;
    margin: 0 0 8px;
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    font-size: 14px;
  }
  fieldset label {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  legend {
    padding: 0;
    margin-bottom: 3px;
  }
  @media (max-width: 560px) {
    .tiles {
      grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    }
    .head {
      flex-wrap: wrap;
    }
  }
</style>
