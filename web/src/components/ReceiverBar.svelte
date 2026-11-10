<script lang="ts">
  import { onMount } from 'svelte';
  import RotationToggle from './RotationToggle.svelte';
  import SMeter from './SMeter.svelte';
  import LcdDigits from './LcdDigits.svelte';
  import TuningKnob from './TuningKnob.svelte';
  import SelectorKnob from './SelectorKnob.svelte';
  import AudioMeter from './AudioMeter.svelte';
  import { api, type Profile, type RadioPatch, type RadioState, type SdrDevice } from '../lib/api';
  import { CHANNEL_STEP, DAB_CHANNELS, MODES, channelEdges, dabChannel, dabIndex, tunedHz, type AudioMode } from '../lib/audio';
  import { describeRanges, unsupported } from '../lib/coverage';
  import { fkeys, SLOTS } from '../lib/fkeys.svelte';
  import { formatMHz, formatRate, parseFrequency, PRESETS } from '../lib/format';
  import { PTY, type Listener, type VfoName } from '../lib/listener.svelte';
  import { schedule } from '../lib/schedule.svelte';
  import { channelDbfs, s9Dbm, sReading, toDbm } from '../lib/smeter';
  import type { WaterfallFrame } from '../lib/stream';

  /** The receiver's front panel, at the top of every page: the display with
   * VFO A and B, the S-meter and tuning knob, mode and audio keys, the
   * programmable function keys, and folding drawers for the centre
   * frequency, presets and the dongle's settings. */
  let {
    radio,
    isAdmin,
    rx,
    ontune,
    onbookmarks,
    onsavebookmark,
    onsettings,
  }: {
    radio: RadioState | null;
    isAdmin: boolean;
    rx: Listener;
    /** Listen at `hz` (in `mode`, if given); the app brings it into the sampled band. */
    ontune: (hz: number, mode?: AudioMode, sampleRate?: number | null, service?: number | null) => void;
    /** Open the bookmark manager; with a slot, to pick a bookmark for that F-key. */
    onbookmarks: (slot?: number) => void;
    /** Open the bookmark manager to save what's being heard. */
    onsavebookmark: () => void;
    onsettings: () => void;
  } = $props();

  const listen = $derived(rx.listen);

  // ---- remembered layout (per browser)
  const LAYOUT = 'sdr.rxbar';
  type Drawer = 'presets' | 'settings';
  const stored = (() => {
    try {
      return JSON.parse(localStorage.getItem(LAYOUT) ?? '{}');
    } catch {
      return {};
    }
  })();
  // Phones start with the bar folded to one line so the page itself is in view.
  const narrow = typeof matchMedia === 'function' && matchMedia('(max-width: 640px)').matches;
  type Size = 'compact' | 'medium' | 'large';
  const SIZES: { id: Size; label: string; title: string }[] = [
    { id: 'compact', label: 'Compact', title: 'One line: frequency, Play and a bar meter' },
    { id: 'medium', label: 'Medium', title: 'Display, meters, tuning knob and function keys' },
    { id: 'large', label: 'Large', title: 'The whole front panel' },
  ];
  // Older browsers stored `collapsed` (Hide/Show).
  let size = $state<Size>(
    ['compact', 'medium', 'large'].includes(stored.size)
      ? stored.size
      : (stored.collapsed ?? narrow)
        ? 'compact'
        : 'large',
  );
  const collapsed = $derived(size === 'compact');
  const large = $derived(size === 'large');
  /** Manual gain to go back to when the gain knob is pulled out of AGC. */
  let lastGain = $state<number | null>(typeof stored.lastGain === 'number' ? stored.lastGain : null);
  let open = $state<Record<Drawer, boolean>>({
    presets: stored.presets ?? false,
    settings: stored.settings ?? false,
  });
  // S-meter calibration: trim in dB, and the IARU VHF scale (S9 = −93 dBm above 30 MHz).
  let trim = $state<number>(stored.trim ?? 0);
  let vhfScale = $state<boolean>(stored.vhfScale ?? false);
  // Meter faces: a needle (analogue) or LED segments (digital), each meter on its own.
  type MeterStyle = 'analogue' | 'digital';
  let sMeter = $state<MeterStyle>(stored.sMeter === 'digital' ? 'digital' : 'analogue');
  let afMeter = $state<MeterStyle>(stored.afMeter === 'analogue' ? 'analogue' : 'digital');
  // svelte-ignore state_referenced_locally
  let step = $state<number>(typeof stored.step === 'number' ? stored.step : CHANNEL_STEP[listen.mode]);
  $effect(() => {
    const v = { size, ...open, trim, vhfScale, sMeter, afMeter, step, lastGain };
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
    const [lo, hi] = channelEdges(tunedHz(listen), listen.mode);
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

  // ---- RF gain knob: pushed in = AGC; out, it steps through the dongle's gains.
  $effect(() => {
    const g = radio?.settings.gain_db;
    if (g !== null && g !== undefined) lastGain = g;
  });
  const agc = $derived(!!radio && radio.settings.gain_db === null);
  const gainLabels = $derived(gainOptions.map((g) => `${g.toFixed(1)} dB`));
  /** Where the gain knob points: the current gain, or the one it returns to from AGC. */
  const gainIndex = $derived.by(() => {
    if (!gainOptions.length) return -1;
    const g = radio?.settings.gain_db ?? lastGain;
    if (g === null || g === undefined) return Math.floor(gainOptions.length / 2);
    let best = 0;
    gainOptions.forEach((x, i) => {
      if (Math.abs(x - g) < Math.abs(gainOptions[best] - g)) best = i;
    });
    return best;
  });
  function pushGain() {
    if (agc) {
      const g = gainIndex >= 0 ? gainOptions[gainIndex] : null;
      if (g !== null) apply({ gain_db: g });
    } else {
      apply({ auto_gain: true });
    }
  }
  const rateLabels = $derived((radio?.sample_rates ?? []).map(formatRate));
  const rateIndex = $derived(radio ? radio.sample_rates.indexOf(radio.settings.sample_rate) : -1);

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

  function submitCentre(e: SubmitEvent) {
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

  // ---- the display: VFO frequency entry (click the big digits to type one)
  const MODE_KEYS: { value: AudioMode; label: string }[] = [
    { value: 'lsb', label: 'LSB' },
    { value: 'usb', label: 'USB' },
    { value: 'cw', label: 'CW' },
    { value: 'am', label: 'AM' },
    { value: 'nfm', label: 'NFM' },
    { value: 'wfm', label: 'WFM' },
  ];
  const STEPS = [10, 50, 100, 500, 1_000, 2_500, 5_000, 6_250, 10_000, 12_500, 25_000, 100_000, 1_000_000];
  const stepLabel = (s: number) => (s >= 1e6 ? `${s / 1e6} MHz` : s >= 1000 ? `${s / 1000} kHz` : `${s} Hz`);

  let typing = $state(false);
  let typed = $state('');
  let typeBox = $state<HTMLInputElement | undefined>();
  function startTyping() {
    typed = (listen.hz / 1e6).toFixed(4);
    typing = true;
    queueMicrotask(() => {
      typeBox?.focus();
      typeBox?.select();
    });
  }
  function submitTyped(e: SubmitEvent) {
    e.preventDefault();
    const hz = parseFrequency(typed);
    if (hz === null) {
      rx.err = 'Enter a frequency like 145.8 or 433.92M';
      return;
    }
    rx.err = null;
    typing = false;
    ontune(hz);
  }

  /** "145.800" (MHz, kHz) and "000" (Hz), MHz padded to four cells. */
  function digits(hz: number): [string, string] {
    const h = Math.max(0, Math.round(hz));
    const mhz = String(Math.floor(h / 1e6)).padStart(4, ' ');
    const khz = String(Math.floor(h / 1e3) % 1000).padStart(3, '0');
    const rest = String(h % 1000).padStart(3, '0');
    return [`${mhz}.${khz}`, `.${rest}`];
  }
  const mainDigits = $derived(digits(listen.hz));
  const otherName = $derived<VfoName>(rx.active === 'A' ? 'B' : 'A');
  const subDigits = $derived(digits(rx.other.hz));
  const modeLabel = (m: AudioMode) => MODES.find((x) => x.value === m)?.label ?? m.toUpperCase();

  function knobStep(n: number) {
    if (listen.mode === 'dab') {
      // Channel by channel through Band III.
      const i = Math.min(DAB_CHANNELS.length - 1, Math.max(0, dabIndex(listen.hz) + Math.sign(n)));
      ontune(DAB_CHANNELS[i][1]);
      return;
    }
    const hz = Math.round((listen.hz + n * step) / step) * step;
    ontune(hz);
  }

  /** A/B: hear the other VFO, retuning the receiver when it's out of band. */
  function swapVfo() {
    const was = listen.mode;
    const v = rx.swap();
    if (v.mode !== was) step = CHANNEL_STEP[v.mode];
    typing = false;
    ontune(v.hz, v.mode, undefined, v.service ?? null);
  }

  function setMode(m: AudioMode) {
    listen.mode = m;
    step = CHANNEL_STEP[m];
    // DAB: onto the nearest multiplex channel, with the receiver centred on it.
    if (m === 'dab') ontune(listen.hz, m);
  }

  // ---- audio
  const meter = $derived(rx.level === null ? 0 : Math.min(1, Math.max(0, (rx.level + 110) / 100)));
  const sqPos = $derived(Math.min(1, Math.max(0, (rx.squelch + 110) / 100)));
  const outside = $derived(
    listen.on &&
      (!rx.inBand || (!!radio && Math.abs(listen.hz - radio.settings.center_hz) > radio.settings.sample_rate * 0.45)),
  );
  const busySig = $derived(listen.on && rx.open && rx.level !== null);

  // ---- RDS (broadcast FM): station name, programme type and radiotext on the LCD
  const showRds = $derived(listen.mode === 'wfm' && listen.on);
  const rdsPs = $derived(rx.rds?.ps?.trim() ?? '');
  const rdsPty = $derived(rx.rds?.pty ? (PTY[rx.rds.pty] ?? `PTY ${rx.rds.pty}`) : '');
  const rdsRt = $derived(rx.rds?.rt?.trim() ?? '');
  // ---- DAB: the station playing and its multiplex, in the same row
  const showDab = $derived(listen.mode === 'dab' && listen.on);
  const dabName = $derived(rx.dab?.label ?? '');
  const dabMux = $derived(
    [dabChannel(listen.hz), rx.dab?.ensemble].filter(Boolean).join(' · ') ||
      (rx.dab?.locked ? 'Reading the station list…' : 'Looking for the multiplex…'),
  );
  // Radiotext wider than the display scrolls across it, like a car radio.
  // Each copy carries RT_GAP px of trailing space so the loop joins up.
  const RT_GAP = 48;
  let rtBoxW = $state(0);
  let rtTextW = $state(0);
  const rtScroll = $derived(!!rdsRt && rtTextW - RT_GAP > rtBoxW + 1);
  const rtSeconds = $derived(Math.max(6, rtTextW / 45));

  // ---- function keys
  let fset = $state(false);
  let menu = $state<number | null>(null);
  let pressTimer: ReturnType<typeof setTimeout> | null = null;
  let longPressed = false;

  function recall(slot: number) {
    const k = fkeys.recall(slot);
    if (k) ontune(k.freq_hz, k.mode, k.sample_rate, k.service_id);
  }
  function fkeyClick(slot: number) {
    if (longPressed) {
      longPressed = false;
      return;
    }
    if (fset) {
      menu = menu === slot ? null : slot;
      return;
    }
    if (fkeys.keys[slot - 1]) recall(slot);
    else onbookmarks(slot);
  }
  function fkeyDown(e: PointerEvent, slot: number) {
    if (e.pointerType === 'mouse') return;
    longPressed = false;
    pressTimer = setTimeout(() => {
      longPressed = true;
      menu = slot;
    }, 550);
  }
  function fkeyUp() {
    if (pressTimer) clearTimeout(pressTimer);
    pressTimer = null;
  }
  async function storeHere(slot: number) {
    menu = null;
    try {
      await fkeys.assign(slot, { id: 0, name: `${(listen.hz / 1e6).toFixed(4)} ${modeLabel(listen.mode)}`, freq_hz: listen.hz, mode: listen.mode });
    } catch (e) {
      fkeys.error = e instanceof Error ? e.message : String(e);
    }
  }
  async function clearKey(slot: number) {
    menu = null;
    try {
      await fkeys.clear(slot);
    } catch (e) {
      fkeys.error = e instanceof Error ? e.message : String(e);
    }
  }
  const isCurrent = (slot: number) => {
    const k = fkeys.recall(slot);
    return !!k && k.freq_hz === listen.hz && k.mode === listen.mode;
  };
  function closeMenu(e: MouseEvent) {
    if (menu !== null && !(e.target as HTMLElement).closest('.fkeys')) menu = null;
  }
</script>

<svelte:window onclick={closeMenu} onkeydown={(e) => e.key === 'Escape' && (menu = null)} />

<section class="panel rx" class:collapsed class:medium={size === 'medium'} aria-label="Receiver">
  <div class="top">
    <h2>Receiver</h2>
    {#if collapsed && radio}
      <span class="mini">
        <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
        <button
          type="button"
          class="key mini-play"
          aria-pressed={listen.on}
          title={listen.on ? 'Stop listening' : 'Listen'}
          onclick={() => rx.toggle()}><span class="led" class:on={listen.on}></span>{listen.on ? 'Stop' : 'Play'}</button
        >
        <strong class="num"
          ><span class="hide-xs">{rx.active} </span>{(listen.hz / 1e6).toFixed(4)}<span class="hide-xs"> MHz</span></strong
        >
        <span class="muted small-text hide-xs">{modeLabel(listen.mode)}</span>
        {#if showRds && rdsPs}<strong class="rds-mini hide-xs" title={rdsRt || rdsPty || 'RDS'}>{rdsPs}</strong>{/if}
        {#if showDab && dabName}<strong class="rds-mini hide-xs" title={dabMux}>{dabName}</strong>{/if}
        <span class="bar-meter"><SMeter bar digital={sMeter === 'digital'} {dbm} {s9} label={formatMHz(listen.hz, 4)} /></span>
        {#if dbm !== null}<span class="s-mini small-text hide-xs">{sReading(dbm, s9)}</span>{/if}
      </span>
    {/if}
    <div class="sizes" role="group" aria-label="Receiver size">
      {#each SIZES as z (z.id)}
        <button
          type="button"
          class="small"
          aria-pressed={size === z.id}
          aria-label={z.label}
          title={z.title}
          onclick={() => (size = z.id)}
          ><span class="hide-xs">{z.label}</span><span class="show-xs" aria-hidden="true">{z.label[0]}</span></button
        >
      {/each}
    </div>
  </div>

  {#if !radio}
    {#if !collapsed}<p class="muted">Connecting…</p>{/if}
  {:else}
    <div id="rx-body" hidden={collapsed}>
      <div class="face">
        <!-- ---- display row: S-meter, LCD, knob -->
        <div class="meter">
          <SMeter digital={sMeter === 'digital'} {dbm} {s9} label={formatMHz(listen.hz, 4)} />
          <AudioMeter digital={afMeter === 'digital'} levels={() => rx.audioLevels()} />
        </div>

        <div class="lcd" aria-label="Display">
          <div class="annun" aria-hidden="true">
            <span class:lit={busySig} class="ok">BUSY</span>
            <span class:lit={rx.squelchOn && listen.mode !== 'wfm'}>SQL</span>
            <span class:lit={rx.nr}>NR</span>
            <span class:lit={rx.afcActive} class:ok={rx.afcActive && rx.afcLocked}>AFC</span>
            <span class:lit={rx.cwActive}>CW-DEC</span>
            <span class:lit={rx.digitalActive} class:ok={rx.syncFresh}>DIG{rx.syncFresh && rx.sync ? ` ${rx.sync.mode}` : ''}</span>
            <span class:lit={showRds} class:ok={showRds && !!rdsPs}>RDS</span>
            <span class:lit={showDab} class:ok={showDab && rx.open}>DAB</span>
            <span class:lit={rx.muted} class="warn">MUTE</span>
            <span class:lit={outside} class="bad">OUT</span>
            <span class="spacer"></span>
            <span class="lit">{rx.autoMode ? 'AUTO' : ''}</span>
          </div>

          <div class="vfo-row main-row">
            <span class="vfo-tag rx-tag" title="VFO {rx.active}: the one you hear">
              <span>VFO</span><strong>{rx.active}</strong>
            </span>
            {#if typing}
              <form class="type-freq" onsubmit={submitTyped}>
                <input
                  id="listen-freq"
                  bind:this={typeBox}
                  bind:value={typed}
                  inputmode="decimal"
                  autocomplete="off"
                  aria-label="Frequency (MHz)"
                  onkeydown={(e) => e.key === 'Escape' && (typing = false)}
                  onblur={() => setTimeout(() => (typing = false), 150)}
                />
                <button type="submit" class="key">Set</button>
              </form>
            {:else}
              <button
                type="button"
                class="digits main-digits"
                title="Click to type a frequency"
                aria-label="VFO {rx.active} {formatMHz(listen.hz, 4)}. Click to type a frequency"
                onclick={startTyping}
              >
                <LcdDigits text={mainDigits[0]} /><span class="hz"><LcdDigits text={mainDigits[1]} /></span>
              </button>
            {/if}
            <span class="mode-tag">{modeLabel(listen.mode)}</span>
          </div>

          <div class="vfo-row sub-row">
            <span class="vfo-tag" title="VFO {otherName}: press A/B to hear it">
              <span>VFO</span><strong>{otherName}</strong>
            </span>
            <span class="digits sub-digits" aria-label="VFO {otherName} {formatMHz(rx.other.hz, 4)}">
              <LcdDigits text={subDigits[0]} /><span class="hz"><LcdDigits text={subDigits[1]} /></span>
            </span>
            <span class="mode-tag">{modeLabel(rx.other.mode)}</span>
            {#if rx.afcActive && listen.on && listen.afc != null}
              <span class="afc-read" title="AFC correction">AFC {listen.afc >= 0 ? '+' : '−'}{(Math.abs(listen.afc) / 1000).toFixed(2)}k</span>
            {/if}
          </div>

          {#if showDab}
            <div class="rds-row" aria-label="DAB station">
              <span class="rds-ps" class:waiting={!dabName}>{dabName || '········'}</span>
              {#if rx.dab?.codec}<span class="rds-pty">{rx.dab.codec}</span>{/if}
              <div class="rds-rt" title={dabMux}><span class="rt-wait">{dabMux}</span></div>
            </div>
          {/if}

          {#if showRds}
            <div class="rds-row" aria-label="RDS">
              <span class="rds-ps" class:waiting={!rdsPs}>{rdsPs || '········'}</span>
              {#if rdsPty}<span class="rds-pty">{rdsPty}</span>{/if}
              {#if rx.rds?.pi}<span class="rds-pi" title="Programme identification">PI {rx.rds.pi}</span>{/if}
              <div class="rds-rt" bind:clientWidth={rtBoxW} title={rdsRt || undefined}>
                {#if rdsRt}
                  <span class="rt-track" class:scroll={rtScroll} style:--rt-dur="{rtSeconds}s" style:--rt-gap="{RT_GAP}px">
                    <span class="rt-copy" bind:clientWidth={rtTextW}>{rdsRt}</span>{#if rtScroll}<span
                        class="rt-copy"
                        aria-hidden="true">{rdsRt}</span
                      >{/if}
                  </span>
                {:else}
                  <span class="rt-wait">{rx.rds?.groups ? 'Decoding…' : 'Looking for station data…'}</span>
                {/if}
              </div>
            </div>
          {/if}

          <div class="lcd-foot">
            <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
            <span>CTR {formatMHz(radio.settings.center_hz, 3)}</span>
            <span>{formatRate(radio.settings.sample_rate)}</span>
            <span>{radio.settings.gain_db === null ? 'AGC' : `${radio.settings.gain_db.toFixed(1)} dB`}</span>
            <span class="dev" title={radio.device ?? ''}>{radio.device ?? 'No device'}{radio.tuner ? ` · ${radio.tuner}` : ''}</span>
            {#if listen.on && rx.stats}
              <span class="stats">{rx.stats.playing ? `${Math.round(rx.stats.buffered * 1000)} ms` : 'buffering'} · {rx.kbps} kb/s</span>
            {/if}
          </div>
        </div>

        <div class="knob-col">
          <TuningKnob
            onstep={knobStep}
            label="Tuning, VFO {rx.active}"
            value={listen.hz}
            valuetext="{formatMHz(listen.hz, 4)} {modeLabel(listen.mode)}"
          />
          <label class="step">
            <span class="lbl">Step</span>
            <select bind:value={step} aria-label="Tuning step">
              {#each STEPS as s (s)}<option value={s}>{stepLabel(s)}</option>{/each}
              {#if !STEPS.includes(step)}<option value={step}>{stepLabel(step)}</option>{/if}
            </select>
          </label>
        </div>
      </div>

      <!-- ---- keys -->
      {#if large}
      <div class="controls">
        <div class="group rf" role="group" aria-label="RF">
          <span class="lbl">RF</span>
          <div class="knobs">
            <SelectorKnob
              label="Gain"
              options={gainLabels}
              index={gainIndex}
              pushable
              pushed={agc}
              pushedText="AGC"
              disabled={locked || !gainOptions.length}
              title={locked
                ? 'Gain (an admin can change it)'
                : agc
                  ? 'AGC on: click to pull the knob out and set the gain by hand'
                  : 'Turn to set the gain; click to push in for AGC'}
              onchange={(i) => apply({ gain_db: gainOptions[i] })}
              onpush={pushGain}
            />
            <SelectorKnob
              label="Rate"
              options={rateLabels}
              index={rateIndex}
              disabled={locked || rateLabels.length < 2}
              title={locked ? 'Sample rate (an admin can change it)' : 'Turn to change the sample rate'}
              onchange={(i) => apply({ sample_rate: radio.sample_rates[i] })}
            />
          </div>
        </div>

        <div class="group full-sm" role="group" aria-label="Audio">
          <span class="lbl">Audio</span>
          <div class="keys">
            <button
              type="button"
              class="key wide play"
              aria-pressed={listen.on}
              disabled={rx.starting}
              onclick={() => rx.toggle()}
              ><span class="led" class:on={listen.on}></span>{rx.starting ? 'Starting…' : listen.on ? 'Stop' : 'Play'}</button
            >
            <button type="button" class="key" aria-pressed={rx.muted} onclick={() => (rx.muted = !rx.muted)}
              ><span class="led" class:on={rx.muted}></span>Mute</button
            >
          </div>
          <label class="slider">
            <span class="lbl">AF</span>
            <input type="range" min="0" max="2" step="0.05" bind:value={rx.volume} aria-label="Volume" />
            <span class="num">{Math.round(rx.volume * 100)}%</span>
          </label>
          <div class="slider">
            <button
              type="button"
              class="key tiny"
              aria-pressed={rx.squelchOn && listen.mode !== 'wfm'}
              disabled={listen.mode === 'wfm'}
              title="Squelch: mute until a signal is above the level"
              onclick={() => (rx.squelchOn = !rx.squelchOn)}
              ><span class="led" class:on={rx.squelchOn && listen.mode !== 'wfm'}></span>SQL</button
            >
            <input
              type="range"
              min="-110"
              max="-20"
              bind:value={rx.squelch}
              aria-label="Squelch level"
              disabled={!rx.squelchOn || listen.mode === 'wfm'}
            />
            <span class="num">{rx.squelch} dB</span>
          </div>
          <div class="level" aria-label="Audio level" title={rx.level === null ? 'No audio' : `${rx.level.toFixed(1)} dBFS`}>
            <div class="fill" class:closed={!rx.open} style:width="{meter * 100}%"></div>
            {#if rx.squelchOn && listen.mode !== 'wfm'}<div class="sq" style:left="{sqPos * 100}%"></div>{/if}
          </div>
        </div>

        <div class="group full-sm" role="group" aria-label="Mode">
          <span class="lbl">Mode</span>
          <div class="keys mode-keys">
            {#each MODE_KEYS as m (m.value)}
              <button type="button" class="key" aria-pressed={listen.mode === m.value} onclick={() => setMode(m.value)}
                ><span class="led" class:on={listen.mode === m.value}></span>{m.label}</button
              >
            {/each}
          </div>
          <button
            type="button"
            class="key auto"
            aria-pressed={rx.autoMode}
            title="Clicking the spectrum picks the mode the band plan suggests"
            onclick={() => (rx.autoMode = !rx.autoMode)}><span class="led" class:on={rx.autoMode}></span>Auto mode</button
          >
        </div>

        <div class="group" role="group" aria-label="VFO">
          <span class="lbl">VFO</span>
          <div class="keys vfo-keys">
            <button
              type="button"
              class="key"
              title="Swap: listen on VFO {otherName} ({formatMHz(rx.other.hz, 4)} {modeLabel(rx.other.mode)})"
              onclick={swapVfo}>A / B</button
            >
            <button
              type="button"
              class="key"
              title="Copy VFO {rx.active} to VFO {otherName}"
              onclick={() => rx.copy()}>{rx.active}={otherName}</button
            >
          </div>
        </div>

        <div class="group" role="group" aria-label="Decode and filters">
          <span class="lbl">DSP</span>
          <div class="keys dsp-keys">
            <button
              type="button"
              class="key"
              aria-pressed={rx.nr}
              title="Spectral noise reduction: removes steady background hiss"
              onclick={() => (rx.nr = !rx.nr)}><span class="led" class:on={rx.nr}></span>NR</button
            >
            <button
              type="button"
              class="key"
              aria-pressed={rx.afcActive}
              disabled={!rx.afcWorks}
              title={rx.afcWorks
                ? 'Automatic frequency control: follow a signal that drifts (up to ±50 kHz in NFM, ±10 kHz in AM)'
                : 'AFC works in NFM and AM'}
              onclick={() => (rx.afcOn = !rx.afcOn)}><span class="led" class:on={rx.afcActive}></span>AFC</button
            >
            <button
              type="button"
              class="key"
              aria-pressed={rx.cwActive}
              disabled={listen.mode === 'wfm'}
              title="Decode Morse: CW signals, beacons and FM repeater idents"
              onclick={() => (rx.cwOn = !rx.cwOn)}><span class="led" class:on={rx.cwActive}></span>CW dec</button
            >
            <button
              type="button"
              class="key"
              aria-pressed={rx.digitalActive}
              disabled={listen.mode !== 'nfm'}
              title={listen.mode === 'nfm'
                ? 'Decode digital voice: DMR, D-STAR, System Fusion, NXDN, P25, dPMR and M17'
                : 'Digital decoding works in NFM'}
              onclick={() => (rx.digitalOn = !rx.digitalOn)}><span class="led" class:on={rx.digitalActive}></span>Digital</button
            >
          </div>
        </div>
      </div>

      {/if}

      <!-- ---- function keys -->
      <div class="fkeys" role="group" aria-label="Function keys">
        <div class="fk-row">
          {#each Array.from({ length: SLOTS }, (_, i) => i + 1) as slot (slot)}
            {@const k = fkeys.keys[slot - 1]}
            {@const shown = fkeys.recall(slot)}
            <div class="fk-cell">
              <button
                type="button"
                class="key fkey"
                class:empty={!k}
                class:setting={fset}
                aria-haspopup={fset ? 'menu' : undefined}
                aria-expanded={menu === slot ? true : undefined}
                aria-label={shown
                  ? `F${slot}: ${shown.name}, ${formatMHz(shown.freq_hz, 4)} ${modeLabel(shown.mode)}`
                  : `F${slot}: not set. Pick a bookmark for it`}
                title={shown
                  ? `${shown.name} · ${formatMHz(shown.freq_hz, 4)} ${modeLabel(shown.mode)}\nRight-click (or long-press) to change`
                  : 'Not set: click to pick a bookmark'}
                onclick={() => fkeyClick(slot)}
                oncontextmenu={(e) => {
                  e.preventDefault();
                  menu = slot;
                }}
                onpointerdown={(e) => fkeyDown(e, slot)}
                onpointerup={fkeyUp}
                onpointerleave={fkeyUp}
              >
                <span class="led" class:on={isCurrent(slot)}></span>
                <span class="fnum">F{slot}</span>
                <span class="fname">{shown ? shown.name : '—'}</span>
              </button>
              {#if menu === slot}
                <div class="fmenu" role="menu" aria-label="F{slot}">
                  <button type="button" role="menuitem" onclick={() => ((menu = null), onbookmarks(slot))}
                    >Pick a bookmark…</button
                  >
                  <button type="button" role="menuitem" onclick={() => storeHere(slot)}
                    >Store {formatMHz(listen.hz, 4)} {modeLabel(listen.mode)}</button
                  >
                  {#if k}
                    <button type="button" role="menuitem" class="danger" onclick={() => clearKey(slot)}>Clear F{slot}</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
        <div class="fk-side">
          {#if !large}
            <button
              type="button"
              class="key wide play"
              aria-pressed={listen.on}
              disabled={rx.starting}
              onclick={() => rx.toggle()}
              ><span class="led" class:on={listen.on}></span>{rx.starting ? 'Starting…' : listen.on ? 'Stop' : 'Play'}</button
            >
          {/if}
          <button
            type="button"
            class="key"
            aria-pressed={fset}
            title="Program the F-keys: click a key to change it"
            onclick={() => {
              fset = !fset;
              menu = null;
            }}><span class="led" class:on={fset}></span>F-Set</button
          >
          <button
            type="button"
            class="key"
            title="Bookmark {formatMHz(listen.hz, 4)} ({modeLabel(listen.mode)})"
            onclick={onsavebookmark}>★ Save</button
          >
          <button type="button" class="key wide accent" onclick={() => onbookmarks()}>Bookmarks</button>
        </div>
      </div>

      {#if outside}
        <p class="oob small-text">
          {formatMHz(listen.hz, 4)} is outside the tuned band ({formatMHz(radio.settings.center_hz, 3)} ± {(
            radio.settings.sample_rate / 2e6
          ).toFixed(2)} MHz).
        </p>
      {/if}
      {#if rx.err}<p class="error">{rx.err}</p>{/if}
      {#if fkeys.error}<p class="error">{fkeys.error}</p>{/if}

      <!-- ---- drawers -->
      {#if large}
      <div class="drawers" role="group" aria-label="Receiver sections">
        <button
          type="button"
          class="small drawer-btn"
          aria-expanded={open.presets}
          aria-controls="rx-presets"
          onclick={() => (open.presets = !open.presets)}
          ><span class="chev" aria-hidden="true"></span>Centre &amp; presets{profiles.length ? ` (${profiles.length} profiles)` : ''}</button
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
          <form class="freq" onsubmit={submitCentre}>
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
              <button type="button" class="small" disabled={locked} onclick={() => nudge(-1e6)}>−1M</button>
              <button type="button" class="small" disabled={locked} onclick={() => nudge(-100e3)}>−100k</button>
              <button type="button" class="small" disabled={locked} onclick={() => nudge(100e3)}>+100k</button>
              <button type="button" class="small" disabled={locked} onclick={() => nudge(1e6)}>+1M</button>
            </div>
          </form>
          <h3 class="sub-head">Presets</h3>
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

      {#if open.settings}
        <div class="drawer" id="rx-settings">
          <div class="ident small-text">
            <strong>{radio.device ?? 'No device'}</strong>
            <span class="muted">{radio.source}{radio.tuner ? ` · ${radio.tuner}` : ''} · {formatRate(radio.settings.sample_rate)}</span>
            {#if radio.ranges.length}<span class="muted">Covers {describeRanges(radio.ranges)}</span>{/if}
          </div>
          {#if radio.error}<p class="error">{radio.error}</p>{/if}
          <div class="grid">
            {#if radio.can_select && devices.length > 1}
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
            {/if}

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

            <label for="smeter-style">S-meter</label>
            <span class="trim">
              <select id="smeter-style" bind:value={sMeter}>
                <option value="analogue">Analogue (needle)</option>
                <option value="digital">Digital (LED bar)</option>
              </select>
              <span class="muted small-text">this browser</span>
            </span>

            <label for="afmeter-style">AF peak meter</label>
            <span class="trim">
              <select id="afmeter-style" bind:value={afMeter}>
                <option value="analogue">Analogue (needle)</option>
                <option value="digital">Digital (LED bar)</option>
              </select>
              <span class="muted small-text">this browser</span>
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

      {/if}

      {#if err}<p class="error">{err}</p>{/if}
    </div>
  {/if}
  <!-- The radio's feet. -->
  <span class="foot left" aria-hidden="true"></span>
  <span class="foot right" aria-hidden="true"></span>
</section>

<style>
  .rx {
    position: relative;
    padding: 12px 14px;
    margin-bottom: 16px;
    min-width: 0;
    background: var(--face);
    border-color: var(--face-edge);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.06),
      0 2px 10px rgba(0, 0, 0, 0.25);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .top h2 {
    margin: 0;
    font-size: 13px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--face-label);
  }
  .sizes {
    margin-left: auto;
    display: inline-flex;
    flex: none;
  }
  .sizes button {
    border-radius: 0;
    padding: 4px 9px;
  }
  .sizes button:first-child {
    border-radius: 6px 0 0 6px;
  }
  .sizes button:last-child {
    border-radius: 0 6px 6px 0;
  }
  .sizes button + button {
    margin-left: -1px;
  }
  .sizes button[aria-pressed='true'] {
    position: relative;
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .show-xs {
    display: none;
  }
  /* Feet under the chassis, like the radio's: rubber with a metal band. */
  .foot {
    position: absolute;
    bottom: -18px;
    width: clamp(80px, 13%, 170px);
    height: 18px;
    border-radius: 0 0 10px 10px;
    border: 1px solid #000;
    border-top: none;
    background: linear-gradient(
      180deg,
      #3a3e46 0 2px,
      #b4bac4 2px 5px,
      #5b6069 5px 6px,
      #2c3036 6px,
      #121417 100%
    );
    box-shadow:
      inset 0 -2px 3px rgba(255, 255, 255, 0.06),
      0 4px 6px rgba(0, 0, 0, 0.35);
  }
  .foot.left {
    left: 9%;
  }
  .foot.right {
    right: 9%;
  }
  .mini {
    flex: 1 1 auto;
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
  .bar-meter {
    flex: 0 1 150px;
    min-width: 80px;
    display: flex;
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

  /* ---- hardware keys */
  .key {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 32px;
    padding: 9px 10px 5px;
    border-radius: 4px;
    border: 1px solid var(--key-edge);
    background: var(--key);
    color: var(--key-text);
    font: 600 11.5px/1 ui-sans-serif, system-ui, sans-serif;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    white-space: nowrap;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.12),
      0 2px 0 var(--key-edge),
      0 3px 4px rgba(0, 0, 0, 0.25);
    transition: transform 0.05s ease, box-shadow 0.05s ease;
  }
  .key:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .key:active:not(:disabled) {
    transform: translateY(2px);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.08),
      0 0 0 var(--key-edge);
  }
  .key.wide {
    min-width: 84px;
  }
  .key.tiny {
    min-height: 26px;
    padding: 7px 7px 4px;
    font-size: 10.5px;
  }
  .key.accent {
    color: var(--accent);
  }
  /* LED strip on top of a key, like the radio's PROC/MONI keys. */
  .led {
    position: absolute;
    top: 3px;
    left: 50%;
    width: 16px;
    height: 3px;
    margin-left: -8px;
    border-radius: 2px;
    background: var(--led-off);
  }
  .led.on {
    background: var(--led);
    box-shadow: 0 0 6px var(--led);
  }
  .play .led.on {
    background: var(--ok);
    box-shadow: 0 0 6px var(--ok);
  }
  .mini-play {
    min-height: 26px;
    padding: 7px 8px 4px;
    font-size: 10.5px;
  }
  .lbl {
    font: 600 10px/1 ui-sans-serif, system-ui, sans-serif;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--face-label);
  }

  /* ---- display row */
  .face {
    display: grid;
    grid-template-columns: 236px minmax(0, 1fr) auto;
    grid-template-areas: 'meter lcd knob';
    gap: 14px;
    align-items: center;
    margin-top: 10px;
  }
  .meter {
    grid-area: meter;
    display: grid;
    justify-items: center;
    gap: 8px;
  }
  .knob-col {
    grid-area: knob;
    display: grid;
    justify-items: center;
    gap: 8px;
  }
  .step {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .step select {
    padding: 3px 6px;
    font-size: 12px;
  }

  .lcd {
    grid-area: lcd;
    min-width: 0;
    display: grid;
    gap: 4px;
    padding: 8px 14px 8px;
    border-radius: 8px;
    border: 2px solid #05090b;
    background: var(--lcd);
    color: var(--lcd-text);
    box-shadow:
      inset 0 2px 10px rgba(0, 0, 0, 0.7),
      0 0 0 3px var(--face-edge);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color-scheme: dark;
  }
  .annun {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
    font-size: 10.5px;
    letter-spacing: 0.08em;
    color: var(--lcd-text);
  }
  .annun span {
    opacity: 0.12;
  }
  .annun .lit {
    opacity: 1;
  }
  .annun .ok.lit {
    color: #4ade80;
  }
  .annun .warn.lit {
    color: #fbbf24;
  }
  .annun .bad.lit {
    color: #f87171;
  }
  .annun .spacer {
    flex: 1;
  }
  .vfo-row {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    min-width: 0;
  }
  .vfo-tag {
    display: grid;
    justify-items: center;
    align-self: center;
    padding: 2px 5px;
    border: 1px solid currentColor;
    border-radius: 3px;
    font-size: 9px;
    line-height: 1.1;
    opacity: 0.85;
  }
  .vfo-tag strong {
    font-size: 15px;
  }
  .rx-tag {
    background: var(--lcd-text);
    color: #071419;
    border-color: var(--lcd-text);
    opacity: 1;
  }
  .digits {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    min-width: 0;
    color: inherit;
  }
  button.digits {
    padding: 0;
    background: none;
    border: none;
    border-radius: 4px;
    cursor: text;
  }
  button.digits:hover {
    background: rgba(200, 243, 255, 0.05);
  }
  .main-digits {
    font-size: clamp(30px, 4.4vw, 52px);
  }
  .main-digits .hz {
    font-size: 0.55em;
  }
  .sub-row {
    color: var(--lcd-sub);
  }
  .sub-digits {
    font-size: clamp(18px, 2vw, 24px);
  }
  .sub-digits .hz {
    font-size: 0.7em;
  }
  .mode-tag {
    align-self: center;
    font-size: 15px;
    font-weight: 700;
    letter-spacing: 0.06em;
  }
  .main-row .mode-tag {
    font-size: 18px;
  }
  .afc-read {
    align-self: center;
    margin-left: auto;
    font-size: 12px;
    color: #4ade80;
  }
  .type-freq {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }
  .type-freq input {
    width: 9em;
    min-width: 0;
    font: 600 26px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    background: rgba(0, 0, 0, 0.35);
    color: var(--lcd-text);
    border-color: var(--lcd-dim);
  }
  /* RDS: the station name big, like a car radio, radiotext under it */
  .rds-row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 12px;
    min-width: 0;
    padding-top: 4px;
    border-top: 1px solid rgba(200, 243, 255, 0.12);
  }
  .rds-ps {
    font-size: 20px;
    font-weight: 700;
    letter-spacing: 0.14em;
    white-space: pre;
    text-shadow: 0 0 6px rgba(200, 243, 255, 0.35);
  }
  .rds-ps.waiting {
    opacity: 0.2;
    text-shadow: none;
  }
  .rds-pty {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--lcd-sub);
  }
  .rds-pi {
    margin-left: auto;
    font-size: 11px;
    color: var(--lcd-dim);
  }
  .rds-rt {
    flex: 1 1 100%;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    font-size: 13px;
  }
  .rt-track {
    display: inline-flex;
  }
  .rt-copy {
    display: inline-block;
    padding-right: var(--rt-gap);
  }
  .rt-track.scroll {
    animation: rt-scroll var(--rt-dur) linear infinite;
  }
  .rds-rt:hover .rt-track.scroll {
    animation-play-state: paused;
  }
  @keyframes rt-scroll {
    to {
      transform: translateX(-50%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .rt-track.scroll {
      animation: none;
    }
    .rds-rt {
      text-overflow: ellipsis;
    }
  }
  .rt-wait {
    color: var(--lcd-dim);
  }
  .rds-mini {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    letter-spacing: 0.08em;
  }
  .lcd-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 12px;
    padding-top: 4px;
    border-top: 1px solid rgba(200, 243, 255, 0.12);
    font-size: 11px;
    color: var(--lcd-dim);
    min-width: 0;
  }
  .lcd-foot .dev {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    max-width: 22em;
  }
  .lcd-foot .stats {
    margin-left: auto;
  }

  /* ---- key groups */
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 12px 22px;
    margin-top: 14px;
    align-items: flex-start;
  }
  .group {
    display: grid;
    gap: 7px;
    align-content: start;
    min-width: 0;
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .rf .knobs {
    display: flex;
    gap: 14px;
  }
  .key.auto {
    justify-self: start;
    font-size: 10.5px;
    min-height: 26px;
    padding: 7px 8px 4px;
  }
  .slider {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .slider .lbl {
    width: 2.6em;
  }
  .slider input[type='range'] {
    flex: 1;
    min-width: 90px;
    width: 140px;
  }
  .slider .num {
    width: 4.3em;
    text-align: right;
    font-size: 12px;
    color: var(--face-label);
  }
  .level {
    position: relative;
    height: 6px;
    border-radius: 3px;
    background: var(--led-off);
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
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--warn);
  }

  /* ---- function keys */
  .fkeys {
    display: flex;
    flex-wrap: wrap;
    align-items: stretch;
    gap: 8px 16px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--face-edge);
  }
  .fk-row {
    flex: 1 1 520px;
    display: grid;
    grid-template-columns: repeat(8, minmax(0, 1fr));
    gap: 6px;
    min-width: 0;
  }
  .fk-cell {
    position: relative;
    min-width: 0;
  }
  .fkey {
    width: 100%;
    min-width: 0;
    height: 46px;
    flex-direction: column;
    align-items: stretch;
    gap: 3px;
    padding: 9px 6px 5px;
    text-transform: none;
    letter-spacing: 0;
  }
  .fnum {
    font-size: 10px;
    color: var(--face-label);
    text-align: left;
    letter-spacing: 0.08em;
  }
  .fname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 600;
    text-align: center;
  }
  .fkey.empty .fname {
    color: var(--face-label);
    font-weight: 400;
  }
  .fkey.setting {
    border-style: dashed;
    border-color: var(--led);
  }
  .fmenu {
    position: absolute;
    z-index: 20;
    top: calc(100% + 6px);
    left: 0;
    display: grid;
    min-width: 210px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  }
  .fk-cell:nth-child(n + 6) .fmenu {
    left: auto;
    right: 0;
  }
  .fmenu button {
    text-align: left;
    background: transparent;
    border-color: transparent;
    font-size: 13px;
    padding: 6px 10px;
  }
  .fmenu button:hover {
    background: var(--surface-2);
  }
  .fk-side {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .fk-side .key {
    height: 46px;
  }

  .oob {
    color: var(--warn);
    margin: 8px 0 0;
  }

  /* ---- drawers */
  .drawers {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--face-edge);
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
    background: var(--surface);
    min-width: 0;
  }
  .freq {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .freq label {
    color: var(--muted);
    font-size: 13px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .row input {
    flex: 1 1 140px;
    min-width: 0;
    max-width: 220px;
    font-size: 17px;
    font-variant-numeric: tabular-nums;
  }
  .ident {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 12px;
    margin-bottom: 10px;
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
    .face {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        'lcd lcd'
        'meter knob';
    }
  }
  @media (max-width: 640px) {
    .face {
      gap: 10px;
    }
    .meter :global(.smeter),
    .meter :global(.ameter) {
      max-width: 200px;
    }
    .knob-col :global(.knob) {
      width: 104px !important;
      height: 104px !important;
    }
    .lcd {
      padding: 6px 10px;
    }
    .annun {
      font-size: 9.5px;
      gap: 2px 7px;
    }
    .main-digits {
      font-size: clamp(26px, 9vw, 40px);
    }
    .main-row .mode-tag {
      font-size: 14px;
    }
    .vfo-row {
      gap: 7px;
    }
    .controls {
      gap: 12px;
    }
    .group.full-sm {
      flex: 1 1 100%;
    }
    .slider input[type='range'] {
      width: auto;
    }
    .fk-row {
      grid-template-columns: repeat(4, minmax(0, 1fr));
      flex-basis: 100%;
    }
    .fk-cell:nth-child(n + 6) .fmenu {
      left: 0;
      right: auto;
    }
    .fk-cell:nth-child(4n) .fmenu,
    .fk-cell:nth-child(4n - 1) .fmenu {
      left: auto;
      right: 0;
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
    .show-xs {
      display: inline;
    }
    .sizes button {
      padding: 4px 7px;
    }
    .bar-meter {
      min-width: 64px;
    }
    /* Folded on a phone: give the heading's room to the bar meter (still read out). */
    .collapsed .top h2 {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
  }
</style>
