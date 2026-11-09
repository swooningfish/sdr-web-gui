<script lang="ts">
  import { formatMHz } from '../lib/format';
  import type { Listen } from '../lib/audio';
  import { bookmarks, colourOf, matches, type Bookmark, type BookmarkInput, type Scope } from '../lib/bookmarks.svelte';
  import BookmarkEditor from './BookmarkEditor.svelte';

  /** Bookmarks beside the spectrum: find one and click to listen. */
  let {
    listen,
    isAdmin,
    onselect,
    onmanage,
  }: {
    listen: Listen;
    isAdmin: boolean;
    onselect: (b: Bookmark) => void;
    onmanage: () => void;
  } = $props();

  let q = $state('');
  let scope = $state<Scope>('all');
  let group = $state('');
  let adding = $state<BookmarkInput | null>(null);
  let saved = $state<string | null>(null);

  const shown = $derived(bookmarks.list.filter((b) => matches(b, q, scope, group)));
  const hasShared = $derived(bookmarks.list.some((b) => b.shared));
  const hasMine = $derived(bookmarks.list.some((b) => !b.shared));

  function startAdd() {
    saved = null;
    adding = { name: '', freq_hz: listen.hz, mode: listen.mode, group: null, colour: null, notes: null, shared: isAdmin };
  }

  async function save(b: BookmarkInput) {
    await bookmarks.add(b);
    adding = null;
    saved = `Saved ${b.name}`;
  }
</script>

<section class="panel">
  <div class="head">
    <h2>Bookmarks</h2>
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

  {#if bookmarks.list.length}
    <div class="filters">
      <input type="search" bind:value={q} placeholder="Search…" aria-label="Search bookmarks" />
      {#if hasShared && hasMine}
        <select bind:value={scope} aria-label="Which bookmarks">
          <option value="all">All</option>
          <option value="shared">Station</option>
          <option value="mine">Mine</option>
        </select>
      {/if}
      {#if bookmarks.groups.length}
        <select bind:value={group} aria-label="Group">
          <option value="">All groups</option>
          {#each bookmarks.groups as g (g)}<option value={g}>{g}</option>{/each}
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
            <span class="name">{b.name}{#if !b.shared}<span class="mine" title="Only you see this"> · mine</span>{/if}</span>
            <span class="freq">{(b.freq_hz / 1e6).toFixed(4)}</span>
            <span class="mode">{b.mode.toUpperCase()}</span>
          </button>
        </li>
      {:else}
        <li class="muted small-text empty">Nothing matches.</li>
      {/each}
    </ul>
  {:else if bookmarks.loaded && !adding}
    <p class="muted small-text">No bookmarks yet. <strong>★ Save</strong> keeps the frequency you're tuned to.</p>
  {/if}
  {#if bookmarks.error}<p class="error">{bookmarks.error}</p>{/if}
</section>

<style>
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .tools {
    display: flex;
    gap: 6px;
  }
  .icon {
    display: inline-grid;
    place-items: center;
    padding: 4px 7px;
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
  .item {
    width: 100%;
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
  .mine {
    color: var(--muted);
    font-size: 12px;
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
