<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { RadioState } from '../lib/api';
  import { AFC_MODES, AudioPlayer, MODES, type AudioFrame, type Listen, type PlayerStats } from '../lib/audio';
  import { formatMHz, parseFrequency } from '../lib/format';
  import type { DecodedMsg, DigitalCall, DigitalEvent, RdsMsg } from '../lib/stream';

  let {
    radio,
    listen = $bindable(),
    send,
    autoMode = $bindable(true),
  }: {
    radio: RadioState | null;
    listen: Listen;
    send: (msg: unknown) => void;
    /** Clicking the spectrum picks the mode the band plan suggests. */
    autoMode?: boolean;
  } = $props();

  /** RDS programme types (European list). */
  const PTY = [
    'No programme type', 'News', 'Current affairs', 'Information', 'Sport', 'Education', 'Drama', 'Culture',
    'Science', 'Varied', 'Pop music', 'Rock music', 'Easy listening', 'Light classical', 'Serious classical',
    'Other music', 'Weather', 'Finance', "Children's", 'Social affairs', 'Religion', 'Phone-in', 'Travel',
    'Leisure', 'Jazz', 'Country', 'National music', 'Oldies', 'Folk', 'Documentary', 'Alarm test', 'Alarm',
  ];

  // Per-browser preferences (kept in localStorage; harmless if unavailable).
  const PREFS = 'sdr.listen';
  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem(PREFS) ?? '{}');
    } catch {
      return {};
    }
  })();
  let squelchOn = $state<boolean>(saved.squelchOn ?? false);
  let squelch = $state<number>(saved.squelch ?? -60);
  let volume = $state<number>(saved.volume ?? 0.8);
  let muted = $state(false);
  let nr = $state<boolean>(saved.nr ?? false);
  let cwOn = $state<boolean>(saved.cw ?? false);
  let afcOn = $state<boolean>(saved.afc ?? false);
  let digitalOn = $state<boolean>(saved.digital ?? false);
  let consoleOn = $state<boolean>(saved.dsdConsole ?? false);
  if (typeof saved.hz === 'number') listen.hz = saved.hz;
  if (typeof saved.mode === 'string') listen.mode = saved.mode;
  if (typeof saved.autoMode === 'boolean') autoMode = saved.autoMode;

  let cwText = $state('');
  let cwWpm = $state<number | null>(null);
  let cwBox = $state<HTMLElement | undefined>();
  let rds = $state<RdsMsg | null>(null);
  let afcLocked = $state(false);
  const afcWorks = $derived(AFC_MODES.includes(listen.mode));
  const afcActive = $derived(afcOn && afcWorks);
  const digitalActive = $derived(digitalOn && listen.mode === 'nfm');

  /** Digital voice: calls on air, recent calls (newest first) and dsd-neo's console. */
  let onAir = $state<DigitalCall[]>([]);
  let heard = $state<(DigitalEvent & { hz: number })[]>([]);
  let dsdLog = $state<string[]>([]);
  let dsdErr = $state<string | null>(null);
  /** Mode dsd-neo keeps finding frame sync for (e.g. "DMR", "M17") and when it
   * last did. Noise gives the odd false sync, so a mode counts only after
   * SYNC_RUN syncs in a row within SYNC_GAP_MS of each other. */
  let sync = $state<{ mode: string; at: number } | null>(null);
  let syncRun = { mode: '', n: 0, at: 0 };
  const SYNC_RUN = 3;
  const SYNC_GAP_MS = 500;
  function sawSync(mode: string) {
    const t = Date.now();
    syncRun = mode === syncRun.mode && t - syncRun.at < SYNC_GAP_MS ? { mode, n: syncRun.n + 1, at: t } : { mode, n: 1, at: t };
    if (syncRun.n >= SYNC_RUN) sync = { mode, at: t };
  }
  let now = $state(Date.now());
  let logBox = $state<HTMLElement | undefined>();
  const syncFresh = $derived(!!sync && now - sync.at < 1500);
  $effect(() => {
    if (!digitalActive || !listen.on) return;
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  $effect(() => {
    const prefs = {
      squelchOn,
      squelch,
      volume,
      nr,
      cw: cwOn,
      afc: afcOn,
      digital: digitalOn,
      dsdConsole: consoleOn,
      autoMode,
      hz: listen.hz,
      mode: listen.mode,
    };
    try {
      localStorage.setItem(PREFS, JSON.stringify(prefs));
    } catch {
      /* private mode etc. */
    }
  });
  let freqInput = $state('');
  let editing = $state(false);
  let level = $state<number | null>(null);
  let inBand = $state(true);
  let open = $state(true);
  let stats = $state<PlayerStats | null>(null);
  let err = $state<string | null>(null);
  let starting = $state(false);
  let bytes = 0;
  let kbps = $state(0);
  let t0 = performance.now();

  const player = new AudioPlayer();
  player.onstats = (s) => (stats = s);

  // Follow the listening frequency when it changes, but never over
  // something typed and not yet submitted (clicking Set blurs the box before
  // the click lands). Typed text is dropped once the box has been left for a
  // moment without submitting.
  let dirty = $state(false);
  let shownHz: number | null = null;
  let blurTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    const hz = listen.hz;
    if (editing || dirty || hz === shownHz) return;
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

  // Keep the server's listener in step with the controls.
  $effect(() => {
    if (!listen.on) return;
    send(params());
  });

  $effect(() => player.setVolume(muted ? 0 : volume));

  function params() {
    return {
      type: 'audio',
      freq_hz: Math.round(listen.hz),
      mode: listen.mode,
      squelch_db: squelchOn ? squelch : null,
      nr,
      cw: cwOn && listen.mode !== 'wfm',
      afc: afcActive,
      digital: digitalActive,
    };
  }

  // The server starts AFC afresh on a new station, or when it is turned on
  // again; drop the old correction until it reports.
  $effect(() => {
    void listen.hz;
    void listen.mode;
    void afcActive;
    listen.afc = null;
    afcLocked = false;
  });

  /** Morse and RDS decoded by the server for this listener. */
  export function decoded(m: DecodedMsg) {
    if (m.type === 'afc') {
      if (!afcActive || !listen.on) return;
      listen.afc = m.offset_hz;
      afcLocked = m.locked;
    } else if (m.type === 'digital') {
      if (!digitalActive) return;
      if (m.error !== undefined) dsdErr = m.error;
      if (m.calls) onAir = m.calls;
      if (m.event) {
        const e = { ...m.event, hz: listen.hz };
        const i = heard.findIndex((h) => h.id === e.id);
        if (i >= 0) heard[i] = { ...e, hz: heard[i].hz };
        else heard = [e, ...heard].slice(0, 50);
      }
      if (m.log) {
        dsdErr = null;
        dsdLog = [...dsdLog, ...m.log].slice(-300);
        for (const l of m.log) {
          const s = /Sync:\s*[+-]?([A-Za-z0-9-]+)/.exec(l);
          if (s) sawSync(s[1]);
        }
      }
    } else if (m.type === 'cw') {
      cwText = (cwText + m.text).slice(-800);
      cwWpm = m.wpm;
    } else {
      rds = m;
    }
  }

  // A new station: forget the old one's RDS and what was on air.
  $effect(() => {
    void listen.hz;
    void listen.mode;
    rds = null;
    onAir = [];
    sync = null;
  });

  // Digital decoding turned off or on: start afresh.
  $effect(() => {
    void digitalActive;
    onAir = [];
    sync = null;
    dsdErr = null;
  });

  // Keep the newest console line in view.
  $effect(() => {
    void dsdLog;
    void consoleOn;
    if (logBox) logBox.scrollTop = logBox.scrollHeight;
  });

  function party(p: string | null): string {
    return p ?? '?';
  }

  function clock(unix: number): string {
    return new Date(unix * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  // Keep the newest Morse in view.
  $effect(() => {
    void cwText;
    if (cwBox) cwBox.scrollTop = cwBox.scrollHeight;
  });

  /** Audio frames from the WebSocket. */
  export function feed(f: AudioFrame) {
    if (!listen.on) return;
    level = Number.isFinite(f.level) ? f.level : null;
    inBand = f.inBand;
    open = f.open;
    player.feed(f);
    bytes += 24 + f.data.length;
    const now = performance.now();
    if (now - t0 > 2000) {
      kbps = Math.round((bytes * 8) / (now - t0));
      bytes = 0;
      t0 = now;
    }
  }

  /** Replies from the server. */
  export function status(kind: string, message: string | null) {
    if (kind === 'audio_error') {
      err = message;
      stop();
    }
  }

  /** The WebSocket reconnected: the server forgot our listener. */
  export function reconnected() {
    if (listen.on) send(params());
  }

  async function start() {
    err = null;
    starting = true;
    try {
      await player.start(muted ? 0 : volume);
      listen.on = true;
    } catch (e) {
      err = `Couldn't start audio: ${e instanceof Error ? e.message : e}`;
    } finally {
      starting = false;
    }
  }

  function stop() {
    if (listen.on) send({ type: 'audio_stop' });
    listen.on = false;
    listen.afc = null;
    level = null;
    stats = null;
    player.stop();
  }

  onDestroy(() => {
    if (listen.on) send({ type: 'audio_stop' });
    player.stop();
  });

  function submitFreq(e: SubmitEvent) {
    e.preventDefault();
    const hz = parseFrequency(freqInput);
    editing = false;
    if (hz === null) {
      err = 'Enter a frequency like 145.8 or 433.92M';
      return;
    }
    err = null;
    dirty = false;
    shownHz = hz;
    listen.hz = hz;
    freqInput = (hz / 1e6).toFixed(4);
  }

  // Meter from -110 to -10 dBFS.
  const meter = $derived(level === null ? 0 : Math.min(1, Math.max(0, (level + 110) / 100)));
  const sqPos = $derived(Math.min(1, Math.max(0, (squelch + 110) / 100)));
  const outside = $derived(
    !!radio &&
      Math.abs(listen.hz - radio.settings.center_hz) > radio.settings.sample_rate * 0.45,
  );
</script>

<section class="panel">
  <div class="head">
    <h2>Listen</h2>
    {#if listen.on}
      <button type="button" class="small" onclick={stop}>Stop</button>
    {:else}
      <button type="button" class="small primary" onclick={start} disabled={starting}>{starting ? 'Starting…' : 'Play'}</button>
    {/if}
  </div>

  <form class="freq" onsubmit={submitFreq}>
    <label for="listen-freq" class="small-text">Frequency (MHz)</label>
    <div class="row">
      <input
        id="listen-freq"
        bind:value={freqInput}
        inputmode="decimal"
        oninput={() => (dirty = true)}
        onfocus={() => (editing = true)}
        onblur={leaveFreq}
      />
      <button type="submit" class="small">Set</button>
    </div>
  </form>
  <p class="muted small-text hint">Or click the waterfall to listen there.</p>

  <div class="row modes" role="group" aria-label="Demodulation">
    {#each MODES as m (m.value)}
      <button type="button" class="small" class:active={listen.mode === m.value} onclick={() => (listen.mode = m.value)}
        >{m.label}</button
      >
    {/each}
  </div>
  <label class="auto small-text muted"
    ><input type="checkbox" bind:checked={autoMode} /> Pick the mode from the band plan when I click the spectrum</label
  >

  <div class="meter" aria-label="Signal level" title={level === null ? '' : `${level.toFixed(1)} dBFS`}>
    <div class="fill" class:closed={!open} style:width="{meter * 100}%"></div>
    {#if squelchOn && listen.mode !== 'wfm'}<div class="sq" style:left="{sqPos * 100}%"></div>{/if}
  </div>
  <div class="small-text muted meter-legend">
    <span>{level === null ? 'level –' : `${level.toFixed(0)} dBFS`}</span>
    {#if listen.on && stats}
      <span>{stats.playing ? `buffer ${Math.round(stats.buffered * 1000)} ms` : 'buffering…'} · {kbps} kb/s</span>
    {/if}
  </div>

  <label class="slider small-text">
    <input type="checkbox" bind:checked={squelchOn} disabled={listen.mode === 'wfm'} /> Squelch
    <input type="range" min="-110" max="-20" bind:value={squelch} disabled={!squelchOn || listen.mode === 'wfm'} />
    <span class="num">{squelch} dB</span>
  </label>
  <label class="slider small-text">
    Volume <input type="range" min="0" max="2" step="0.05" bind:value={volume} aria-label="Volume" />
    <span class="num">{Math.round(volume * 100)}%</span>
  </label>
  <div class="row toggles">
    <button type="button" class="small" class:active={muted} aria-pressed={muted} onclick={() => (muted = !muted)}
      >{muted ? 'Unmute' : 'Mute'}</button
    >
    <button
      type="button"
      class="small"
      class:active={nr}
      aria-pressed={nr}
      title="Spectral noise reduction: removes steady background hiss"
      onclick={() => (nr = !nr)}>Noise reduction</button
    >
    <button
      type="button"
      class="small"
      class:active={cwOn && listen.mode !== 'wfm'}
      aria-pressed={cwOn && listen.mode !== 'wfm'}
      disabled={listen.mode === 'wfm'}
      title="Decode Morse: CW signals, beacons and FM repeater idents"
      onclick={() => (cwOn = !cwOn)}>Decode CW</button
    >
    <button
      type="button"
      class="small"
      class:active={afcActive}
      aria-pressed={afcActive}
      disabled={!afcWorks}
      title={afcWorks
        ? 'Automatic frequency control: follow a signal that drifts or wanders (up to ±50 kHz in NFM, ±10 kHz in AM)'
        : 'AFC works in NFM and AM'}
      onclick={() => (afcOn = !afcOn)}>AFC</button
    >
    <button
      type="button"
      class="small"
      class:active={digitalActive}
      aria-pressed={digitalActive}
      disabled={listen.mode !== 'nfm'}
      title={listen.mode === 'nfm'
        ? 'Decode digital voice: DMR, D-STAR, System Fusion, NXDN, P25, dPMR and M17 (callsigns, IDs, talkgroups)'
        : 'Digital decoding works in NFM'}
      onclick={() => (digitalOn = !digitalOn)}>Decode digital</button
    >
  </div>
  {#if afcActive && listen.on}
    <p class="afc small-text" class:muted={!afcLocked}>
      {#if listen.afc == null}
        AFC: waiting for a signal…
      {:else}
        AFC {listen.afc >= 0 ? '+' : '−'}{(Math.abs(listen.afc) / 1000).toFixed(2)} kHz · {formatMHz(listen.hz + listen.afc, 4)}
        · {afcLocked ? 'following' : 'holding'}
      {/if}
    </p>
  {/if}

  {#if cwOn && listen.mode !== 'wfm'}
    <div class="decoder">
      <div class="dec-head small-text">
        <span class="muted">Morse{cwWpm ? ` · ${Math.round(cwWpm)} WPM` : ''}</span>
        {#if cwText}<button type="button" class="small" onclick={() => (cwText = '')}>Clear</button>{/if}
      </div>
      <div class="cw-text" bind:this={cwBox} role="log" aria-label="Decoded Morse">
        {#if cwText}{cwText}{:else}<span class="muted"
            >{listen.on ? 'Listening for Morse…' : 'Press Play to decode Morse.'}</span
          >{/if}
      </div>
    </div>
  {/if}

  {#if digitalActive}
    <div class="decoder digital">
      <div class="dec-head small-text">
        <span class="muted">
          Digital voice{#if listen.on && syncFresh && sync}
            · <span class="sync">{sync.mode} sync</span>{/if}
        </span>
        <span class="row">
          <button
            type="button"
            class="small"
            class:active={consoleOn}
            aria-pressed={consoleOn}
            title="Show what the decoder prints: every frame it syncs to"
            onclick={() => (consoleOn = !consoleOn)}>Console</button
          >
          {#if heard.length || dsdLog.length}
            <button
              type="button"
              class="small"
              onclick={() => {
                heard = [];
                dsdLog = [];
              }}>Clear</button
            >
          {/if}
        </span>
      </div>
      {#if dsdErr}
        <p class="warn small-text">{dsdErr}</p>
      {:else if !listen.on}
        <p class="small-text muted">Press Play to decode digital voice.</p>
      {:else if onAir.length}
        {#each onAir as c (c.slot)}
          <div class="onair" class:emergency={c.emergency}>
            {#if c.protocol}<span class="badge">{c.protocol}</span>{/if}
            {#if onAir.length > 1}<span class="small-text muted">TS{c.slot + 1}</span>{/if}
            <strong>{party(c.src)}</strong>
            <span class="muted">→</span>
            <span>{party(c.dst)}</span>
            {#if c.name && c.name !== c.dst && c.name !== c.src}<span class="small-text muted">{c.name}</span>{/if}
            {#if c.enc}<span class="tag">encrypted</span>{/if}
            {#if c.emergency}<span class="tag warn">emergency</span>{/if}
          </div>
        {/each}
      {:else}
        <p class="small-text muted">
          {syncFresh ? 'Receiving…' : 'Listening for DMR, D-STAR, YSF, NXDN, P25, dPMR and M17…'}
        </p>
      {/if}
      {#if heard.length}
        <ul class="heard small-text" aria-label="Recent digital calls">
          {#each heard as e (e.id)}
            <li title={e.summary}>
              <span class="muted num-t">{clock(e.time)}</span>
              {#if e.protocol}<span class="badge">{e.protocol}</span>{/if}
              <strong>{party(e.src)}</strong>
              <span class="muted">→ {party(e.dst)}</span>
              {#if e.code}<span class="muted">{e.code}</span>{/if}
              {#if e.enc}<span class="tag">encrypted</span>{/if}
              {#if e.emergency}<span class="tag warn">emergency</span>{/if}
              {#if e.hz !== listen.hz}<span class="muted">{formatMHz(e.hz, 4)}</span>{/if}
              {#if e.alias}<span class="extra">“{e.alias}”</span>{/if}
              {#if e.text}<span class="extra">{e.text}</span>{/if}
              {#if e.gps}<span class="extra muted">{e.gps}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
      {#if consoleOn}
        <pre class="console" bind:this={logBox} aria-label="Decoder console">{#if dsdLog.length}{dsdLog.join(
              '\n',
            )}{:else}<span class="muted">Nothing decoded yet.</span>{/if}</pre>
      {/if}
    </div>
  {/if}

  {#if listen.mode === 'wfm' && listen.on}
    <div class="rds" aria-label="RDS">
      {#if rds?.ps || rds?.rt}
        <div class="ps-row">
          <strong class="ps">{rds.ps?.trim() || '········'}</strong>
          {#if rds.pty}<span class="pty small-text">{PTY[rds.pty] ?? `PTY ${rds.pty}`}</span>{/if}
        </div>
        {#if rds.rt}<p class="rt small-text">{rds.rt}</p>{/if}
        {#if rds.pi}<p class="small-text muted pi">RDS · PI {rds.pi}</p>{/if}
      {:else}
        <p class="small-text muted pi">RDS: {rds?.groups ? 'decoding…' : 'looking for station data…'}</p>
      {/if}
    </div>
  {/if}

  {#if listen.on && (!inBand || outside)}
    <p class="warn small-text">
      {formatMHz(listen.hz, 4)} is outside the tuned band
      {radio ? `(${formatMHz(radio.settings.center_hz, 3)} ± ${(radio.settings.sample_rate / 2e6).toFixed(2)} MHz)` : ''}.
    </p>
  {/if}
  {#if err}<p class="error">{err}</p>{/if}
</section>

<style>
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  .freq {
    display: grid;
    gap: 4px;
  }
  .row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .row input {
    flex: 1;
    min-width: 0;
    font-variant-numeric: tabular-nums;
  }
  .hint {
    margin: 4px 0 8px;
  }
  .modes {
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .toggles {
    margin-top: 8px;
    flex-wrap: wrap;
  }
  .toggles .active,
  .modes .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .meter {
    position: relative;
    height: 10px;
    border-radius: 5px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--ok);
    transition: width 0.1s linear;
  }
  .fill.closed {
    background: var(--muted);
  }
  .sq {
    position: absolute;
    top: -1px;
    bottom: -1px;
    width: 2px;
    background: var(--warn);
  }
  .meter-legend {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    margin: 4px 0 10px;
    font-variant-numeric: tabular-nums;
  }
  .slider {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0;
  }
  .slider input[type='range'] {
    flex: 1;
    min-width: 0;
  }
  .num {
    width: 4.5em;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .warn {
    color: var(--warn);
  }
  .afc {
    margin: 6px 0 0;
    font-variant-numeric: tabular-nums;
    color: var(--ok);
  }
  .afc.muted {
    color: var(--muted);
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -4px 0 10px;
  }
  .decoder,
  .rds {
    margin-top: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .dec-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    min-height: 26px;
  }
  .cw-text {
    max-height: 5.6em;
    overflow-y: auto;
    font: 14px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    letter-spacing: 0.04em;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
  .ps-row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
  }
  .ps {
    font: 600 18px/1.3 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    white-space: pre;
  }
  .pty {
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
  }
  .rt {
    margin: 4px 0 0;
    overflow-wrap: anywhere;
  }
  .pi {
    margin: 4px 0 0;
  }
  .digital .row {
    gap: 4px;
  }
  .digital .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .sync {
    color: var(--ok);
  }
  .onair {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 8px;
    margin: 4px 0;
    font-size: 16px;
    overflow-wrap: anywhere;
  }
  .onair.emergency strong {
    color: var(--warn);
  }
  .badge {
    padding: 0 6px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    background: var(--accent);
    color: var(--accent-text);
  }
  .tag {
    padding: 0 6px;
    border-radius: 999px;
    border: 1px solid var(--border);
    font-size: 11px;
  }
  .tag.warn {
    border-color: var(--warn);
  }
  .heard {
    list-style: none;
    margin: 8px 0 0;
    padding: 6px 0 0;
    border-top: 1px solid var(--border);
    max-height: 10em;
    overflow-y: auto;
  }
  .heard li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 6px;
    padding: 2px 0;
    overflow-wrap: anywhere;
  }
  .num-t {
    font-variant-numeric: tabular-nums;
  }
  .extra {
    flex-basis: 100%;
    padding-left: 1em;
  }
  .console {
    margin: 8px 0 0;
    max-height: 12em;
    overflow: auto;
    padding: 6px;
    border-radius: 6px;
    background: var(--surface);
    font: 11px/1.35 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    white-space: pre;
  }
</style>
