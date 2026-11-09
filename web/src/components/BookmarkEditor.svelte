<script lang="ts">
  import { MODES, type AudioMode } from '../lib/audio';
  import { formatMHz, formatRate, parseFrequency } from '../lib/format';
  import { bookmarks, colourOf, type BookmarkInput } from '../lib/bookmarks.svelte';

  /** Add or edit one bookmark. */
  let {
    initial,
    isAdmin,
    title,
    sampleRates = [],
    current = null,
    onsave,
    oncancel,
  }: {
    initial: BookmarkInput;
    isAdmin: boolean;
    title: string;
    /** Rates the receiver offers. */
    sampleRates?: number[];
    /** What's tuned now, for "Use what's tuned now". */
    current?: { hz: number; mode: AudioMode; rate: number | null } | null;
    onsave: (b: BookmarkInput) => Promise<void>;
    oncancel: () => void;
  } = $props();

  // A form edits its own copy; the props only seed it.
  // svelte-ignore state_referenced_locally
  const seed = initial;
  // svelte-ignore state_referenced_locally
  const admin = isAdmin;
  let name = $state(seed.name);
  let freq = $state((seed.freq_hz / 1e6).toFixed(4));
  let mode = $state<AudioMode>(seed.mode);
  let group = $state(seed.group ?? '');
  let useColour = $state(!!seed.colour);
  let colour = $state(seed.colour ?? colourOf(seed));
  let notes = $state(seed.notes ?? '');
  /** 0 = leave the receiver's rate as it is. */
  let rate = $state(seed.sample_rate ?? 0);
  // A saved rate the receiver no longer lists still shows (and keeps).
  const rates = $derived([...new Set([...sampleRates, ...(seed.sample_rate ? [seed.sample_rate] : [])])].sort((a, b) => a - b));

  function useCurrent() {
    if (!current) return;
    freq = (current.hz / 1e6).toFixed(4);
    mode = current.mode;
    if (current.rate) rate = current.rate;
  }
  let shared = $state(seed.shared && admin);
  let busy = $state(false);
  let err = $state<string | null>(null);

  const uid = Math.random().toString(36).slice(2, 8);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    const hz = parseFrequency(freq);
    if (hz === null) {
      err = 'Enter a frequency like 145.7125 or 433.92M';
      return;
    }
    busy = true;
    err = null;
    try {
      await onsave({
        name: name.trim(),
        freq_hz: hz,
        mode,
        group: group.trim() || null,
        colour: useColour ? colour : null,
        notes: notes.trim() || null,
        sample_rate: rate || null,
        // A DAB bookmark keeps its station while it stays on its multiplex.
        service_id: mode === 'dab' && hz === seed.freq_hz ? (seed.service_id ?? null) : null,
        shared,
      });
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<form class="editor" onsubmit={submit} aria-label={title}>
  <p class="title small-text"><strong>{title}</strong></p>
  <div class="grid">
    <label class="wide"
      >Name
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={name} maxlength="64" required autofocus placeholder="e.g. GB3XX repeater" /></label
    >
    <label
      >Frequency (MHz)
      <input bind:value={freq} inputmode="decimal" required /></label
    >
    <label
      >Mode
      <select bind:value={mode}>
        {#each MODES as m (m.value)}<option value={m.value}>{m.label}</option>{/each}
      </select></label
    >
    <label
      >Sample rate
      <select bind:value={rate} title="Switched to when the bookmark is tuned (admins)">
        <option value={0}>Leave as is</option>
        {#each rates as r (r)}<option value={r}>{formatRate(r)}</option>{/each}
      </select></label
    >
    <label
      >Group
      <input bind:value={group} maxlength="32" list="bm-groups-{uid}" placeholder="optional" />
      <datalist id="bm-groups-{uid}">
        {#each bookmarks.groups as g (g)}<option value={g}></option>{/each}
      </datalist></label
    >
    <div class="colour">
      <span>Colour</span>
      <div class="row">
        <input type="checkbox" bind:checked={useColour} aria-label="Use a custom colour" />
        <input type="color" bind:value={colour} disabled={!useColour} aria-label="Colour" />
      </div>
    </div>
    <label class="wide"
      >Notes
      <input bind:value={notes} maxlength="500" placeholder="optional: CTCSS, hours, who…" /></label
    >
    {#if isAdmin}
      <fieldset class="wide">
        <legend class="small-text">Save in</legend>
        <label class="inline"><input type="radio" bind:group={shared} value={true} /> Station list (everyone sees it)</label>
        <label class="inline"><input type="radio" bind:group={shared} value={false} /> My bookmarks</label>
      </fieldset>
    {/if}
  </div>
  {#if err}<p class="error">{err}</p>{/if}
  <div class="actions">
    <button type="submit" class="small primary" disabled={busy}>{busy ? 'Saving…' : 'Save'}</button>
    <button type="button" class="small" onclick={oncancel}>Cancel</button>
    {#if current}
      <button
        type="button"
        class="small"
        title="Set frequency, mode and sample rate to what you're tuned to"
        onclick={useCurrent}>Use what's tuned now ({formatMHz(current.hz, 4)})</button
      >
    {/if}
  </div>
</form>

<style>
  .editor {
    display: grid;
    gap: 8px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .title {
    margin: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 8px;
  }
  .wide {
    grid-column: 1 / -1;
  }
  label,
  .colour {
    display: grid;
    gap: 3px;
    font-size: 13px;
    color: var(--muted);
  }
  label input,
  label select {
    min-width: 0;
    width: 100%;
    background: var(--surface);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 36px;
  }
  input[type='color'] {
    width: 48px;
    height: 32px;
    padding: 2px;
  }
  fieldset {
    border: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
  }
  legend {
    color: var(--muted);
    padding: 0;
    margin-bottom: 3px;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
  }
  .inline input {
    width: auto;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
