<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Device, type EventRow, type RadioState, type SstvInfo } from '../lib/api';
  import type { SstvRows } from '../lib/stream';
  import { unsupported } from '../lib/coverage';
  import { formatMHz, formatRate, relTime } from '../lib/format';
  import { imageUrl, SSTV_MODES, zipUrl } from '../lib/sstv';
  import { Selection } from '../lib/selection.svelte';

  let { isAdmin, radio }: { isAdmin: boolean; radio: RadioState | null } = $props();

  /**
   * Blocks the SSTV tab offers to tune to. UHF is centred 550 kHz below ARISS
   * 437.550 so the dongle's DC spike stays off the channel, and spans most of
   * the 435–438 MHz satellite downlinks.
   */
  const BLOCKS = [
    { name: 'VHF', hz: 145_300_000, rate: 2_048_000 },
    { name: 'UHF', hz: 437_000_000, rate: 2_048_000 },
  ];

  interface Live {
    channel: number;
    session: number;
    vis: number;
    width: number;
    height: number;
    lines: number;
    done: boolean;
    updated: number;
  }

  let info = $state<SstvInfo | null>(null);
  let images = $state<EventRow[]>([]);
  let live = $state<Live[]>([]);
  let open = $state<EventRow | null>(null);
  let err = $state<string | null>(null);
  let now = $state(Date.now());
  let dialog = $state<HTMLDialogElement | undefined>();

  // Pixel buffers and canvases for the live view, outside Svelte's reactivity.
  const buffers = new Map<number, ImageData>();
  const canvases = new Map<number, HTMLCanvasElement>();

  /** Same margin as `dsp::ddc::fits` (FM channel ±10 kHz). */
  const inBlock = (b: (typeof BLOCKS)[number], hz: number) => Math.abs(hz - b.hz) + 10_000 <= b.rate * 0.45;

  /** Blocks holding an enabled channel that isn't being listened to right now. */
  const tunable = $derived.by(() => {
    if (!info?.status.enabled || !isAdmin || !radio?.can_tune) return [];
    const chans = info.status.channels.filter((c) => c.enabled);
    return BLOCKS.filter((b) => {
      const mine = chans.filter((c) => inBlock(b, c.frequency_hz));
      return !unsupported(radio, b.hz) && mine.length > 0 && !mine.some((c) => c.in_band);
    });
  });
  const channelName = (i: number) => info?.status.channels[i]?.label ?? `Channel ${i + 1}`;
  const data = (e: EventRow) =>
    e.data as { id: string; mode: string; width: number; height: number; lines: number; complete: boolean };

  /** Live rows from the WebSocket. */
  export function rows(f: SstvRows) {
    let l = live.find((x) => x.channel === f.channel);
    if (!l || l.session !== f.session) {
      const img = new ImageData(f.width, f.height);
      for (let i = 3; i < img.data.length; i += 4) img.data[i] = 255;
      buffers.set(f.channel, img);
      const entry: Live = {
        channel: f.channel,
        session: f.session,
        vis: f.vis,
        width: f.width,
        height: f.height,
        lines: 0,
        done: false,
        updated: Date.now(),
      };
      if (l) Object.assign(l, entry);
      else live.push(entry);
      l = live.find((x) => x.channel === f.channel)!;
    }
    const img = buffers.get(f.channel)!;
    const d = img.data;
    let o = f.firstLine * f.width * 4;
    for (let i = 0; i < f.rgb.length; i += 3, o += 4) {
      d[o] = f.rgb[i];
      d[o + 1] = f.rgb[i + 1];
      d[o + 2] = f.rgb[i + 2];
    }
    l.lines = Math.max(l.lines, f.firstLine + f.lines);
    l.updated = Date.now();
    canvases.get(f.channel)?.getContext('2d')?.putImageData(img, 0, 0, 0, f.firstLine, f.width, f.lines);
  }

  /** A finished image (event from the decoder). */
  export function push(_device: Device, event: EventRow) {
    images = [event, ...images];
    const idx = info?.status.channels.findIndex((c) => c.label === data(event).id) ?? -1;
    const l = live.find((x) => x.channel === idx);
    if (l) l.done = true;
  }

  function paint(node: HTMLCanvasElement, ch: number) {
    canvases.set(ch, node);
    const img = buffers.get(ch);
    if (img) node.getContext('2d')?.putImageData(img, 0, 0);
    return {
      destroy() {
        if (canvases.get(ch) === node) canvases.delete(ch);
      },
    };
  }

  onMount(() => {
    Promise.all([api.sstv(), api.events(200, 'sstv')]).then(
      ([i, ev]) => {
        info = i;
        images = ev;
      },
      (e) => (err = e instanceof Error ? e.message : String(e)),
    );
    const t = setInterval(async () => {
      now = Date.now();
      try {
        info = await api.sstv();
      } catch {
        /* keep last */
      }
    }, 5000);
    return () => clearInterval(t);
  });

  // Gallery multi-select: download several as a ZIP, or (admins) delete them.
  let selecting = $state(false);
  const sel = new Selection();
  const order = $derived(images.map((e) => e.id));
  $effect(() => sel.keepOnly(order));

  function stopSelecting() {
    selecting = false;
    sel.clear();
  }

  function onCard(e: EventRow, ev: MouseEvent) {
    if (selecting) sel.toggle(e.id, ev.shiftKey, order);
    else show(e);
  }

  async function removeSelected() {
    const ids = sel.list();
    if (!ids.length) return;
    if (!confirm(`Delete ${ids.length} image${ids.length === 1 ? '' : 's'}? This cannot be undone.`)) return;
    try {
      const r = await api.deleteSstvImages(ids);
      const gone = new Set(ids);
      images = images.filter((x) => !gone.has(x.id));
      sel.clear();
      if (r.deleted < ids.length) err = `${ids.length - r.deleted} could not be deleted`;
    } catch (x) {
      err = x instanceof Error ? x.message : String(x);
    }
  }

  function show(e: EventRow) {
    open = e;
    dialog?.showModal();
  }

  async function remove(e: EventRow) {
    if (!confirm('Delete this image? This cannot be undone.')) return;
    try {
      await api.deleteSstvImage(e.id);
      images = images.filter((x) => x.id !== e.id);
      dialog?.close();
    } catch (x) {
      err = x instanceof Error ? x.message : String(x);
    }
  }

  async function tune(b: (typeof BLOCKS)[number]) {
    try {
      await api.configure({ center_hz: b.hz, sample_rate: b.rate });
      info = await api.sstv();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
</script>

<svelte:window onkeydown={(e) => selecting && e.key === 'Escape' && !dialog?.open && stopSelecting()} />

<div class="sstv">
  <section class="panel">
    <div class="head">
      <h2>SSTV</h2>
      {#if info}
        <div class="channels small-text">
          {#each info.status.channels as c, i (i)}
            {@const why = unsupported(radio, c.frequency_hz)}
            <span class="pill" class:na={!!why} title={why ?? (c.in_band ? `${c.images} images since start` : 'outside the tuned band')}>
              <span class="dot" class:ok={c.enabled && c.in_band && !why}></span>
              {c.label} {formatMHz(c.frequency_hz, 3)}
              {c.demod?.toUpperCase()}{why ? ' · not supported by this SDR' : c.enabled ? (c.in_band ? '' : ' · idle') : ' · off'}
            </span>
          {/each}
          {#each tunable as b (b.name)}
            <button type="button" class="small" title="{formatMHz(b.hz, 3)} at {formatRate(b.rate)}" onclick={() => tune(b)}
              >Tune to {b.name} block</button
            >
          {/each}
        </div>
      {/if}
    </div>
    {#if err}<p class="error">{err}</p>{/if}

    {#if live.length === 0}
      <p class="muted small-text">
        Images appear here line by line while they're being received. Each channel listens whenever its frequency is
        inside the tuned band.
      </p>
    {:else}
      <div class="live">
        {#each live as l (`${l.channel}:${l.session}`)}
          {@const stale = !l.done && now - l.updated > 20_000}
          <figure>
            <canvas width={l.width} height={l.height} use:paint={l.channel} aria-label="Image being received"></canvas>
            <figcaption class="small-text">
              <strong>{channelName(l.channel)}</strong> · {SSTV_MODES[l.vis] ?? `VIS ${l.vis}`}
              <span class="muted">
                {#if l.done}
                  · saved to the gallery
                {:else if stale}
                  · signal lost at line {l.lines} of {l.height}
                {:else}
                  · receiving line {l.lines} of {l.height}
                {/if}
              </span>
              {#if !l.done && !stale}
                <progress max={l.height} value={l.lines}></progress>
              {/if}
            </figcaption>
          </figure>
        {/each}
      </div>
    {/if}
  </section>

  <section class="panel">
    <div class="head">
      <h2>Gallery</h2>
      <span class="gallery-tools small-text">
        <span class="muted">{images.length} images</span>
        {#if images.length}
          <button type="button" class="small" aria-pressed={selecting} onclick={() => (selecting ? stopSelecting() : (selecting = true))}
            >{selecting ? 'Done' : 'Select'}</button
          >
        {/if}
      </span>
    </div>
    {#if selecting}
      <div class="bulk small-text" role="toolbar" aria-label="Selected images">
        <strong>{sel.size} selected</strong>
        <button type="button" class="small" onclick={() => sel.toggleAll(order)}>{sel.allOf(order) ? 'Select none' : 'Select all'}</button>
        <a
          class="button small"
          class:disabled={sel.size === 0}
          aria-disabled={sel.size === 0}
          href={sel.size ? zipUrl(sel.list()) : undefined}
          download="sstv-images.zip">Download ZIP</a
        >
        {#if isAdmin}
          <button type="button" class="small danger" disabled={sel.size === 0} onclick={removeSelected}>Delete</button>
        {/if}
        <span class="muted hint">Click images to select; Shift+click selects a range.</span>
      </div>
    {/if}
    {#if images.length === 0}
      <p class="muted small-text">No images yet.</p>
    {/if}
    <ul class="grid">
      {#each images as e (e.id)}
        {@const d = data(e)}
        <li>
          <button
            type="button"
            class="card"
            class:picked={selecting && sel.has(e.id)}
            aria-pressed={selecting ? sel.has(e.id) : undefined}
            onclick={(ev) => onCard(e, ev)}
          >
            {#if selecting}<span class="tick" aria-hidden="true">{sel.has(e.id) ? '✓' : ''}</span>{/if}
            <img src={imageUrl(e.id, true)} alt="{d.mode} image from {d.id}" loading="lazy" width="160" />
            <span class="meta small-text">
              <strong>{d.mode}</strong>
              <span class="muted" title={new Date(e.ts).toLocaleString()}>{relTime(e.ts, now)}</span>
            </span>
            <span class="muted small-text">
              {d.id}{d.complete ? '' : ` · partial (${d.lines}/${d.height})`}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
</div>

<dialog bind:this={dialog} onclose={() => (open = null)} aria-label="SSTV image">
  {#if open}
    {@const d = data(open)}
    <div class="viewer">
      <img src={imageUrl(open.id)} alt="{d.mode} image from {d.id}" />
      <div class="viewer-bar small-text">
        <span>
          <strong>{d.mode}</strong> · {d.id} · {when(open.ts)}{open.freq_hz ? ` · ${formatMHz(open.freq_hz, 3)}` : ''}
          {d.complete ? '' : ` · partial (${d.lines} of ${d.height} lines)`}
        </span>
        <span class="actions">
          <a class="button small" href={imageUrl(open.id)} download>Download</a>
          {#if isAdmin}
            <button type="button" class="small danger" onclick={() => remove(open!)}>Delete</button>
          {/if}
          <button type="button" class="small" onclick={() => dialog?.close()}>Close</button>
        </span>
      </div>
    </div>
  {/if}
</dialog>

<style>
  .sstv {
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
  .pill.na {
    border-style: dashed;
    color: var(--muted);
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .live {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }
  figure {
    margin: 0;
    flex: 0 1 640px;
    min-width: 0;
  }
  canvas {
    display: block;
    width: 100%;
    height: auto;
    border-radius: 6px;
    background: #000;
    image-rendering: pixelated;
  }
  figcaption {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
  }
  progress {
    flex: 1 1 120px;
    accent-color: var(--accent);
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 12px;
  }
  .gallery-tools {
    display: inline-flex;
    align-items: center;
    gap: 10px;
  }
  .bulk {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    margin: 0 0 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
    position: sticky;
    top: 60px;
    z-index: 2;
  }
  .bulk .hint {
    flex-basis: 100%;
  }
  a.button.disabled {
    opacity: 0.55;
    pointer-events: none;
  }
  .card {
    position: relative;
  }
  .card.picked {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .tick {
    position: absolute;
    top: 10px;
    left: 10px;
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    font-size: 14px;
    font-weight: 700;
    color: var(--accent-text);
    background: var(--surface);
    border: 2px solid var(--text);
    z-index: 1;
  }
  .card.picked .tick {
    background: var(--accent);
    border-color: var(--accent);
  }
  .card {
    display: grid;
    gap: 4px;
    width: 100%;
    padding: 6px;
    text-align: left;
    background: var(--surface-2);
  }
  .card img {
    width: 100%;
    height: auto;
    aspect-ratio: 4 / 3;
    object-fit: cover;
    border-radius: 4px;
    background: #000;
  }
  .meta {
    display: flex;
    justify-content: space-between;
    gap: 6px;
  }
  dialog {
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    color: var(--text);
    /* SSTV images are small (320–640 px); show them large, pixels crisp. */
    width: min(96vw, 960px);
    max-width: none;
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.7);
  }
  .viewer img {
    display: block;
    width: 100%;
    max-height: 78vh;
    object-fit: contain;
    background: #000;
    image-rendering: pixelated;
  }
  .viewer-bar {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
  }
  .actions {
    display: flex;
    gap: 6px;
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
  a.button:hover {
    border-color: var(--accent);
  }
</style>
