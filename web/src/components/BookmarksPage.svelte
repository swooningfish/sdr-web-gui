<script lang="ts">
  import { formatMHz } from '../lib/format';
  import { Selection } from '../lib/selection.svelte';
  import { indeterminate } from '../lib/actions';
  import {
    bookmarks,
    colourOf,
    matches,
    type Bookmark,
    type BookmarkInput,
    type ImportResult,
    type Scope,
  } from '../lib/bookmarks.svelte';
  import BookmarkEditor from './BookmarkEditor.svelte';

  /** Manage bookmarks in detail: edit, bulk delete, import and export. */
  let {
    isAdmin,
    onlisten,
    onback,
    current,
  }: {
    isAdmin: boolean;
    onlisten: (b: Bookmark) => void;
    onback: () => void;
    /** What the Listen panel is tuned to, for "Add bookmark". */
    current: { hz: number; mode: Bookmark['mode'] };
  } = $props();

  type Key = 'name' | 'freq' | 'mode' | 'group';
  let q = $state('');
  let scope = $state<Scope>('all');
  let group = $state('');
  let sortKey = $state<Key>('freq');
  let asc = $state(true);
  let editing = $state<{ id: number | null; input: BookmarkInput } | null>(null);
  let msg = $state<string | null>(null);
  let err = $state<string | null>(null);
  const sel = new Selection();

  // svelte-ignore state_referenced_locally
  let importShared = $state(isAdmin);
  let importing = $state(false);
  let importResult = $state<ImportResult | null>(null);
  let fileInput = $state<HTMLInputElement | undefined>();

  const shown = $derived.by(() => {
    const list = bookmarks.list.filter((b) => matches(b, q, scope, group));
    const cmp: Record<Key, (a: Bookmark, b: Bookmark) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      freq: (a, b) => a.freq_hz - b.freq_hz,
      mode: (a, b) => a.mode.localeCompare(b.mode) || a.freq_hz - b.freq_hz,
      group: (a, b) => (a.group ?? '￿').localeCompare(b.group ?? '￿') || a.freq_hz - b.freq_hz,
    };
    const sorted = [...list].sort(cmp[sortKey]);
    return asc ? sorted : sorted.reverse();
  });
  const editable = $derived(shown.filter((b) => b.can_edit).map((b) => b.id));

  $effect(() => sel.keepOnly(bookmarks.list.filter((b) => b.can_edit).map((b) => b.id)));

  function sortBy(k: Key) {
    if (sortKey === k) asc = !asc;
    else {
      sortKey = k;
      asc = true;
    }
  }
  const ariaSort = (k: Key) => (sortKey === k ? (asc ? 'ascending' : 'descending') : 'none');
  const arrow = (k: Key) => (sortKey === k ? (asc ? '▲' : '▼') : '');

  function add() {
    msg = null;
    editing = {
      id: null,
      input: { name: '', freq_hz: current.hz, mode: current.mode, group: null, colour: null, notes: null, shared: isAdmin },
    };
  }

  function edit(b: Bookmark) {
    msg = null;
    editing = { id: b.id, input: { ...b } };
  }

  async function save(input: BookmarkInput) {
    if (!editing) return;
    if (editing.id === null) await bookmarks.add(input);
    else await bookmarks.update(editing.id, input);
    msg = `Saved ${input.name}.`;
    editing = null;
  }

  async function remove(ids: number[], what: string) {
    if (!confirm(`Delete ${what}? This can't be undone.`)) return;
    err = null;
    try {
      const r = await bookmarks.remove(ids);
      msg = `Deleted ${r.deleted} bookmark${r.deleted === 1 ? '' : 's'}.`;
      sel.clear();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
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
</script>

<div class="page">
  <section class="panel">
    <div class="head">
      <h2>Bookmarks</h2>
      <button type="button" class="small" onclick={onback}>← Spectrum</button>
    </div>
    <p class="muted small-text intro">
      The <strong>station list</strong> is shared with everyone who signs in{isAdmin ? ' (admins edit it)' : ''};
      <strong>mine</strong> are yours alone. Both show as flags on the spectrum.
    </p>

    <div class="toolbar">
      <input type="search" bind:value={q} placeholder="Search name, group, notes or MHz…" aria-label="Search bookmarks" />
      <select bind:value={scope} aria-label="Which bookmarks">
        <option value="all">All</option>
        <option value="shared">Station list</option>
        <option value="mine">Mine</option>
      </select>
      <select bind:value={group} aria-label="Group">
        <option value="">All groups</option>
        {#each bookmarks.groups as g (g)}<option value={g}>{g}</option>{/each}
      </select>
      <button type="button" class="small primary" onclick={add} disabled={!!editing}>Add bookmark</button>
    </div>

    {#if editing}
      <BookmarkEditor
        initial={editing.input}
        {isAdmin}
        title={editing.id === null ? 'New bookmark' : `Edit ${editing.input.name}`}
        onsave={save}
        oncancel={() => (editing = null)}
      />
    {/if}
    {#if msg}<p class="success small-text">{msg}</p>{/if}
    {#if err}<p class="error">{err}</p>{/if}

    {#if sel.size}
      <div class="bulk small-text" role="toolbar" aria-label="Selected bookmarks">
        <strong>{sel.size} selected</strong>
        <button type="button" class="small danger" onclick={() => remove(sel.list(), `${sel.size} bookmark${sel.size === 1 ? '' : 's'}`)}
          >Delete</button
        >
        <button type="button" class="small" onclick={() => sel.clear()}>Clear</button>
      </div>
    {/if}

    {#if bookmarks.list.length}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th class="pick">
                <input
                  type="checkbox"
                  aria-label="Select all shown bookmarks you can change"
                  checked={sel.allOf(editable)}
                  use:indeterminate={!sel.allOf(editable) && sel.someOf(editable)}
                  disabled={!editable.length}
                  onchange={() => sel.toggleAll(editable)}
                />
              </th>
              <th aria-sort={ariaSort('name')}><button type="button" class="sort" onclick={() => sortBy('name')}>Name {arrow('name')}</button></th>
              <th class="num" aria-sort={ariaSort('freq')}
                ><button type="button" class="sort" onclick={() => sortBy('freq')}>MHz {arrow('freq')}</button></th
              >
              <th aria-sort={ariaSort('mode')}><button type="button" class="sort" onclick={() => sortBy('mode')}>Mode {arrow('mode')}</button></th>
              <th class="hide-sm" aria-sort={ariaSort('group')}
                ><button type="button" class="sort" onclick={() => sortBy('group')}>Group {arrow('group')}</button></th
              >
              <th class="hide-md">Notes</th>
              <th><span class="sr-only">Actions</span></th>
            </tr>
          </thead>
          <tbody>
            {#each shown as b (b.id)}
              <tr class:picked={sel.has(b.id)}>
                <td class="pick">
                  {#if b.can_edit}
                    <input
                      type="checkbox"
                      aria-label="Select {b.name}"
                      checked={sel.has(b.id)}
                      onclick={(e) => {
                        sel.toggle(b.id, e.shiftKey, editable);
                      }}
                    />
                  {/if}
                </td>
                <td>
                  <div class="name">
                    <span class="swatch" style:background={colourOf(b)}></span>
                    <span class="label">{b.name}</span>
                    <span class="tag" title={b.shared ? 'In the station list' : 'Only you see this'}
                      >{b.shared ? 'station' : 'mine'}</span
                    >
                  </div>
                </td>
                <td class="num">{(b.freq_hz / 1e6).toFixed(4)}</td>
                <td>{b.mode.toUpperCase()}</td>
                <td class="hide-sm">{b.group ?? ''}</td>
                <td class="hide-md notes" title={b.notes ?? ''}>{b.notes ?? ''}</td>
                <td class="actions">
                  <button type="button" class="small" onclick={() => onlisten(b)} title="Listen on {formatMHz(b.freq_hz, 4)}"
                    >Listen</button
                  >
                  {#if b.can_edit}
                    <button type="button" class="small" onclick={() => edit(b)} aria-label="Edit {b.name}">Edit</button>
                  {/if}
                </td>
              </tr>
            {:else}
              <tr><td colspan="7" class="muted small-text">Nothing matches.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if bookmarks.loaded}
      <p class="muted">No bookmarks yet. Add one here, use <strong>★ Save</strong> beside the spectrum, or import a CSV below.</p>
    {/if}
  </section>

  <section class="panel">
    <h2>Import and export</h2>
    <div class="io">
      <div>
        <h3 class="small-text">Export as CSV</h3>
        <div class="links">
          <a class="button small" href="/api/bookmarks/export.csv?scope=all" download>All</a>
          <a class="button small" href="/api/bookmarks/export.csv?scope=shared" download>Station list</a>
          <a class="button small" href="/api/bookmarks/export.csv?scope=mine" download>Mine</a>
        </div>
        <p class="muted small-text">Columns: name, frequency_mhz, mode, group, colour, notes.</p>
      </div>
      <div>
        <h3 class="small-text">Import a CSV</h3>
        <p class="muted small-text">
          Our own export, a CHIRP CSV or a gqrx <code>bookmarks.csv</code>. Bookmarks already there (same name and
          frequency) are skipped.
        </p>
        {#if isAdmin}
          <fieldset>
            <legend class="small-text muted">Into</legend>
            <label><input type="radio" bind:group={importShared} value={true} /> Station list</label>
            <label><input type="radio" bind:group={importShared} value={false} /> My bookmarks</label>
          </fieldset>
        {/if}
        <div class="links">
          <input
            type="file"
            accept=".csv,.txt,text/csv"
            bind:this={fileInput}
            aria-label="CSV file to import"
            onchange={importFile}
            disabled={importing}
          />
        </div>
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
      </div>
    </div>
  </section>
</div>

<style>
  .page {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 16px;
    max-width: 1100px;
    margin: 0 auto;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .intro {
    margin: -4px 0 12px;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 10px;
  }
  .toolbar input {
    flex: 1 1 220px;
    min-width: 0;
  }
  .toolbar select {
    min-width: 0;
  }
  .bulk {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    margin: 10px 0 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .table-wrap {
    overflow-x: auto;
    margin-top: 8px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
  }
  th,
  td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    text-align: left;
    vertical-align: middle;
  }
  th {
    color: var(--muted);
    font-weight: 500;
    font-size: 13px;
    white-space: nowrap;
  }
  .sort {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  th.pick,
  td.pick {
    width: 28px;
    padding-right: 0;
  }
  tr.picked td {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .label {
    overflow-wrap: anywhere;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    flex: none;
  }
  .tag {
    font-size: 11px;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 6px;
    white-space: nowrap;
  }
  .notes {
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
  }
  .actions {
    white-space: nowrap;
    text-align: right;
  }
  .actions button + button {
    margin-left: 4px;
  }
  .io {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 16px 24px;
  }
  h3 {
    margin: 0 0 6px;
    font-weight: 600;
  }
  .links {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .links input[type='file'] {
    max-width: 100%;
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
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  @media (max-width: 900px) {
    .hide-md {
      display: none;
    }
  }
  @media (max-width: 560px) {
    .hide-sm {
      display: none;
    }
  }
</style>
