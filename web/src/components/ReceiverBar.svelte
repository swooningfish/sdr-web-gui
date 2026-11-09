<script lang="ts">
  import { onMount } from 'svelte';
  import RotationToggle from './RotationToggle.svelte';
  import BookmarksPanel from './BookmarksPanel.svelte';
  import SMeter from './SMeter.svelte';
  import { api, type Profile, type RadioPatch, type RadioState, type SdrDevice } from '../lib/api';
  import { channelEdges, type Listen } from '../lib/audio';
  import type { Bookmark } from '../lib/bookmarks.svelte';
  import { describeRanges, unsupported } from '../lib/coverage';
  import { formatMHz, formatRate, parseFrequency, PRESETS } from '../lib/format';
  import { schedule } from '../lib/schedule.svelte';
  import { channelDbfs, s9Dbm, sReading, toDbm } from '../lib/smeter';
  import type { WaterfallFrame } from '../lib/stream';

  /** The receiver, at the top of every page: tuning, S-meter, presets,
   * bookmarks and the dongle's settings (the last three fold away). */
  let {
    radio,
    isAdmin,
    listen,
    onbookmark,
    onmanage,
    onsettings,
  }: {
    radio: RadioState | null;
    isAdmin: boolean;
    listen: Listen;
    onbookmark: (b: Bookmark) => void;
    onmanage: () => void;
    onsettings: () => void;
  } = $props();

  // ---- remembered layout (per browser)
  const LAYOUT = 'sdr.rxbar';
  type Drawer = 'presets' | 'bookmarks' | 'settings';
  const stored = (() => {
    try {
      return JSON.parse(localStorage.getItem(LAYOUT) ?? '{}');
    } catch {
      return {};
    }
  })();
  // Phones start with the bar folded to one line so the page itself is in view.
  const narrow = typeof matchMedia === 'function' && matchMedia('(max-width: 640px)').matches;
  let collapsed = $state<boolean>(stored.collapsed ?? narrow);
  let open = $state<Record<Drawer, boolean>>({
    presets: stored.presets ?? false,
    bookmarks: stored.bookmarks ?? false,
    settings: stored.settings ?? false,
  });
  // S-meter calibration: trim in dB, and the IARU VHF scale (S9 = −93 dBm above 30 MHz).
  let trim = $state<number>(stored.trim ?? 0);
  let vhfScale = $state<boolean>(stored.vhfScale ?? false);
  $effect(() => {
    const v = { collapsed, ...open, trim, vhfScale };
    try {
      localStorage.setItem(LAYOUT, JSON.stringify(v));
    } catch {
      /* private mode */
    }
  });

  // ---- S-meter, fed from the spectrum frames
  let dbfs = $state<number | null>(null);
  let lastFrame = 0;
  /** Spectrum frame from App. */
  export function push(f: WaterfallFrame) {
    const now = performance.now();
    if (now - lastFrame < 60) return;
    lastFrame = now;
    const [lo, hi] = channelEdges(listen.hz, listen.mode);
    dbfs = channelDbfs(f, lo, hi);
  }
  const dbm = $derived(dbfs === null || !radio ? null : toDbm(dbfs, radio.settings.gain_db, trim));
  const s9 = $derived(s9Dbm(vhfScale, listen.hz));

  // ---- schedule (rotation profiles)
  onMount(() => schedule.use());
  const profiles = $derived(schedule.info?.config.profiles ?? []);
  const rotating = $derived(schedule.info?.config.rotation ?? false);
  const passOn = $derived(schedule.info?.status.mode === 'pass');

  // ---- device picker
  let busy = $state(false);
  let err = $state<string | null>(null);
  let devices = $state<SdrDevice[]>([]);
  const GROUPS = [
    { source: 'rtlsdr', label: 'USB' },
    { source: 'rtl_tcp', label: 'Network (rtl_tcp)' },
    { source: 'replay', label: 'Recording' },
    { source: 'synthetic', label: 'Simulated' },
  ] as const;
  $effect(() => {
    void radio?.device_id;
    void radio?.connected;
    if (!radio?.can_select) return;
    api.sdrDevices().then(
      (r) => (devices = r.devices),
      () => {},
    );
  });

  async function selectDevice(e: Event) {
    const id = (e.currentTarget as HTMLSelectElement).value;
    busy = true;
    err = null;
    try {
      await api.selectSdr(id);
    } catch (ex) {
      err = ex instanceof Error ? ex.message : String(ex);
    } finally {
      busy = false;
    }
  }

  // ---- centre frequency box: show the radio's frequency, but never over
  // something typed and not yet submitted (clicking Tune blurs the box first).
  let freqInput = $state('');
  let editing = $state(false);
  let dirty = $state(false);
  let shownHz: number | null = null;
  let blurTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    const hz = radio?.settings.center_hz;
    if (hz === undefined || editing || dirty || hz === shownHz) return;
    shownHz = hz;
    freqInput = (hz / 1e6).toFixed(4);
  });
  function leaveFreq() {
    editing = false;
    if (blurTimer) clearTimeout(blurTimer);
    blurTimer = setTimeout(() => {
      if (!editing && dirty) {
        dirty = false;
        shownHz = null;
      }
    }, 400);
  }

  const locked = $derived(!isAdmin || !radio?.can_tune || busy);
  const gainValue = $derived(radio ? (radio.settings.gain_db === null ? 'auto' : String(radio.settings.gain_db)) : 'auto');
  const gainOptions = $derived.by(() => {
    if (!radio) return [] as number[];
    const g = [...radio.gains_db];
    const cur = radio.settings.gain_db;
    if (cur !== null && !g.includes(cur)) g.push(cur);
    return g.sort((a, b) => a - b);
  });

  async function apply(patch: RadioPatch) {
    busy = true;
    err = null;
    try {
      await api.configure(patch);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function submitFreq(e: SubmitEvent) {
    e.preventDefault();
    const hz = parseFrequency(freqInput);
    if (hz === null) {
      err = 'Enter a frequency like 145.8, 433.92M or 437550k';
      return;
    }
    editing = false;
    dirty = false;
    shownHz = hz;
    freqInput = (hz / 1e6).toFixed(4);
    apply({ center_hz: hz });
  }

  function nudge(deltaHz: number) {
    if (radio) apply({ center_hz: radio.settings.center_hz + deltaHz });
  }

  function onGain(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    apply(v === 'auto' ? { auto_gain: true } : { gain_db: Number(v) });
  }

  function tuneProfile(p: Pick<Profile, 'center_hz' | 'sample_rate'>) {
    apply({ center_hz: p.center_hz, sample_rate: p.sample_rate });
  }

  const presetTitle = (hz: number, why: string | null) => (why ? `${formatMHz(hz)}: ${why}` : formatMHz(hz));
</script>

<section class="panel rx" class:collapsed aria-label="Receiver">
  <div class="top">
    <h2>Receiver</h2>
    {#if collapsed && radio}
      <span class="mini">
        <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
        <strong class="num">{formatMHz(radio.settings.center_hz, 4)}</strong>
        <span class="muted small-text hide-xs">{formatRate(radio.settings.sample_rate)}</span>
        {#if dbm !== null}<span class="s-mini small-text">{sReading(dbm, s9)}</span>{/if}
      </span>
    {/if}
    <button
      type="button"
      class="small fold"
      aria-expanded={!collapsed}
      aria-controls="rx-body"
      title={collapsed ? 'Show the receiver' : 'Fold the receiver away'}
      onclick={() => (collapsed = !collapsed)}>{collapsed ? 'Show' : 'Hide'}<span class="chev" aria-hidden="true"></span></button
    >
  </div>

  {#if !radio}
    {#if !collapsed}<p class="muted">Connecting…</p>{/if}
  {:else}
    <div id="rx-body" hidden={collapsed}>
      <div class="main">
        <div class="ident">
          <div class="status">
            <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
            <div class="ident-text">
              <div class="device-name">{radio.device ?? 'No device'}</div>
              <div class="muted small-text">
                {radio.source}{radio.tuner ? ` · ${radio.tuner}` : ''} · {formatRate(radio.settings.sample_rate)}
              </div>
              {#if radio.ranges.length}
                <div class="muted small-text">Covers {describeRanges(radio.ranges)}</div>
              {/if}
            </div>
          </div>
          {#if radio.can_select && devices.length > 1}
            <div class="device">
              <label for="sdr">Device</label>
              <select id="sdr" value={radio.device_id ?? ''} disabled={!isAdmin || busy} onchange={selectDevice}>
                {#if !devices.some((d) => d.id === radio.device_id)}
                  <option value={radio.device_id ?? ''}>{radio.device ?? radio.device_id ?? 'None'}</option>
                {/if}
                {#each GROUPS as g (g.source)}
                  {@const list = devices.filter((d) => d.source === g.source)}
                  {#if list.length}
                    <optgroup label={g.label}>
                      {#each list as d (d.id)}
                        <option value={d.id}>{d.name}{d.tuner ? ` (${d.tuner})` : ''}</option>
                      {/each}
                    </optgroup>
                  {/if}
                {/each}
              </select>
            </div>
          {/if}
          {#if radio.error}<p class="error">{radio.error}</p>{/if}
        </div>

        <form class="freq" onsubmit={submitFreq}>
          <label for="freq">Centre frequency (MHz)</label>
          <div class="row">
            <input
              id="freq"
              inputmode="decimal"
              autocomplete="off"
              bind:value={freqInput}
              oninput={() => (dirty = true)}
              onfocus={() => (editing = true)}
              onblur={leaveFreq}
              disabled={locked}
            />
            <button type="submit" disabled={locked}>Tune</button>
          </div>
          <div class="row nudges">
            <button type="button" class="small" disabled={locked} onclick={() => nudge(-1e6)}>−1M</button>
            <button type="button" class="small" disabled={locked} onclick={() => nudge(-100e3)}>−100k</button>
            <button type="button" class="small" disabled={locked} onclick={() => nudge(100e3)}>+100k</button>
            <button type="button" class="small" disabled={locked} onclick={() => nudge(1e6)}>+1M</button>
          </div>
        </form>

        <div class="meter">
          <SMeter {dbm} {s9} label={formatMHz(listen.hz, 4)} />
          <div class="meter-caption small-text muted" title="The S-meter reads the channel you're listening to">
            {formatMHz(listen.hz, 4)} {listen.mode.toUpperCase()}
          </div>
        </div>
      </div>

      <div class="drawers" role="group" aria-label="Receiver sections">
        <button
          type="button"
          class="small drawer-btn"
          aria-expanded={open.presets}
          aria-controls="rx-presets"
          onclick={() => (open.presets = !open.presets)}
          ><span class="chev" aria-hidden="true"></span>Presets{profiles.length ? ` & rotation (${profiles.length})` : ''}</button
        >
        <button
          type="button"
          class="small drawer-btn"
          aria-expanded={open.bookmarks}
          aria-controls="rx-bookmarks"
          onclick={() => (open.bookmarks = !open.bookmarks)}><span class="chev" aria-hidden="true"></span>Bookmarks</button
        >
        <button
          type="button"
          class="small drawer-btn"
          aria-expanded={open.settings}
          aria-controls="rx-settings"
          onclick={() => (open.settings = !open.settings)}><span class="chev" aria-hidden="true"></span>Gain &amp; settings</button
        >
        {#if rotating || passOn}
          <span class="pill small-text" title="What the scheduler is doing">
            <span class="dot ok"></span>
            {passOn ? `Tracking ${schedule.info?.status.profile ?? 'a pass'}` : `Rotating: ${schedule.info?.status.profile ?? '…'}`}
          </span>
        {/if}
      </div>

      {#if open.presets}
        <div class="drawer" id="rx-presets">
          <div class="chips">
            {#each PRESETS as p (p.label)}
              {@const why = unsupported(radio, p.hz)}
              <button
                type="button"
                class="small preset"
                class:active={radio.settings.center_hz === p.hz}
                class:na={!!why}
                disabled={locked || !!why}
                onclick={() => apply({ center_hz: p.hz, ...(p.rate ? { sample_rate: p.rate } : {}) })}
                title={presetTitle(p.hz, why)}>{p.label}</button
              >
            {/each}
          </div>
          <h3 class="sub-head">Rotation profiles</h3>
          {#if profiles.length}
            <div class="chips">
              {#each profiles as p (p.name + p.center_hz)}
                {@const why = unsupported(radio, p.center_hz)}
                {@const current = schedule.info?.status.mode === 'rotation' && schedule.info?.status.profile === p.name}
                <button
                  type="button"
                  class="small preset"
                  class:active={radio.settings.center_hz === p.center_hz}
                  class:rotating={current}
                  class:na={!!why}
                  disabled={locked || !!why}
                  onclick={() => tuneProfile(p)}
                  title="{presetTitle(p.center_hz, why)} · {formatRate(p.sample_rate)} · {p.dwell_s} s in rotation{current
                    ? ' (on now)'
                    : ''}">{p.name}</button
                >
              {/each}
            </div>
          {:else}
            <p class="muted small-text">No rotation profiles yet.</p>
          {/if}
          <div class="rotation-box">
            <RotationToggle {isAdmin} {onsettings} />
          </div>
          {#if rotating && isAdmin}
            <p class="muted small-text hint">Rotation is on, so it will move on at the next switch. Turn it off to stay put.</p>
          {/if}
        </div>
      {/if}

      {#if open.bookmarks}
        <div class="drawer" id="rx-bookmarks">
          <BookmarksPanel bare {listen} {isAdmin} onselect={onbookmark} {onmanage} />
        </div>
      {/if}

      {#if open.settings}
        <div class="drawer" id="rx-settings">
          <div class="grid">
            <label for="rate">Sample rate</label>
            <select
              id="rate"
              disabled={locked}
              value={radio.settings.sample_rate}
              onchange={(e) => apply({ sample_rate: Number((e.currentTarget as HTMLSelectElement).value) })}
            >
              {#each radio.sample_rates as r (r)}
                <option value={r}>{formatRate(r)}</option>
              {/each}
            </select>

            <label for="gain">Gain</label>
            <select id="gain" disabled={locked} value={gainValue} onchange={onGain}>
              <option value="auto">Auto (AGC)</option>
              {#each gainOptions as g (g)}
                <option value={String(g)}>{g.toFixed(1)} dB</option>
              {/each}
            </select>

            <label for="ppm">PPM correction</label>
            <input
              id="ppm"
              type="number"
              min="-200"
              max="200"
              step="1"
              disabled={locked}
              value={radio.settings.ppm}
              onchange={(e) => apply({ ppm: Number((e.currentTarget as HTMLInputElement).value) })}
            />

            <label for="ds">Direct sampling</label>
            <select
              id="ds"
              disabled={locked}
              value={radio.settings.direct_sampling}
              onchange={(e) => apply({ direct_sampling: Number((e.currentTarget as HTMLSelectElement).value) })}
            >
              <option value={0}>Off</option>
              <option value={1}>I branch</option>
              <option value={2}>Q branch (HF)</option>
            </select>

            <label for="bias">Bias-T (powers LNA)</label>
            <input
              id="bias"
              type="checkbox"
              disabled={locked}
              checked={radio.settings.bias_tee}
              onchange={(e) => apply({ bias_tee: (e.currentTarget as HTMLInputElement).checked })}
            />

            <label for="trim">S-meter trim</label>
            <span class="trim">
              <input id="trim" type="number" min="-40" max="40" step="1" bind:value={trim} />
              <span class="muted small-text">dB (this browser)</span>
            </span>

            <label for="vhf">IARU VHF scale</label>
            <span class="trim">
              <input id="vhf" type="checkbox" bind:checked={vhfScale} />
              <span class="muted small-text">S9 = −93 dBm above 30 MHz</span>
            </span>
          </div>
          <p class="muted small-text hint">
            dBm is an estimate from the dongle's gain; add a trim to match a known signal.
          </p>
          {#if !isAdmin}
            <p class="muted small-text hint">View only — an admin can change receiver settings.</p>
          {:else if !radio.can_tune}
            <p class="muted small-text hint">The {radio.source} source can't be retuned.</p>
          {:else}
            <p class="muted small-text hint">Tip: double-click the spectrum to centre on a signal.</p>
          {/if}
        </div>
      {/if}

      {#if err}<p class="error">{err}</p>{/if}
    </div>
  {/if}
</section>

<style>
  .rx {
    padding: 12px 14px;
    min-width: 0;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .top h2 {
    margin: 0;
  }
  .fold {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .s-mini {
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    font-variant-numeric: tabular-nums;
  }
  /* Chevron: points right when closed, down when open. */
  .chev {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-right: 2px solid currentColor;
    border-bottom: 2px solid currentColor;
    transform: rotate(-45deg);
    transition: transform 0.15s ease;
  }
  [aria-expanded='true'] .chev {
    transform: rotate(45deg) translate(-2px, -2px);
  }
  .fold .chev {
    transform: rotate(-135deg) translate(-2px, -2px);
  }
  .fold[aria-expanded='false'] .chev {
    transform: rotate(45deg) translate(-2px, -2px);
  }

  .main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(260px, 340px) 230px;
    gap: 12px 20px;
    align-items: center;
    margin-top: 10px;
  }
  .status {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .ident-text {
    min-width: 0;
  }
  .device-name {
    overflow-wrap: anywhere;
  }
  .device {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    margin-top: 10px;
    font-size: 14px;
  }
  .device label {
    color: var(--muted);
  }
  .device select {
    min-width: 0;
  }
  .freq {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .freq label {
    color: var(--muted);
    font-size: 13px;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
    min-width: 0;
    font-size: 18px;
    font-variant-numeric: tabular-nums;
  }
  .nudges button {
    flex: 1;
    min-width: 0;
  }
  .meter {
    display: grid;
    justify-items: center;
    gap: 2px;
  }
  .meter-caption {
    font-variant-numeric: tabular-nums;
  }

  .drawers {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .drawer-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .drawer-btn[aria-expanded='true'] {
    background: var(--surface-2);
    border-color: var(--accent);
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    margin-left: auto;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .drawer {
    margin-top: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--surface-2) 45%, transparent);
    min-width: 0;
  }
  .sub-head {
    margin: 12px 0 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .preset.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .preset.rotating {
    box-shadow: inset 0 -2px 0 var(--ok);
  }
  .preset.na {
    border-style: dashed;
    text-decoration: line-through;
  }
  .rotation-box {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .hint {
    margin: 8px 0 0;
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    font-size: 14px;
    max-width: 760px;
  }
  .grid label {
    color: var(--muted);
  }
  .grid > input[type='checkbox'],
  .trim input[type='checkbox'] {
    justify-self: start;
    width: 18px;
    height: 18px;
  }
  .trim {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .trim input[type='number'] {
    width: 5em;
  }

  @media (max-width: 1100px) {
    .main {
      grid-template-columns: minmax(0, 1fr) 230px;
    }
    .ident {
      grid-column: 1 / -1;
    }
  }
  @media (max-width: 640px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
    .meter :global(.smeter) {
      max-width: 220px;
    }
    .grid {
      grid-template-columns: auto minmax(0, 1fr);
    }
    .pill {
      margin-left: 0;
    }
    .hide-xs {
      display: none;
    }
  }
</style>
