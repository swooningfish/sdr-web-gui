<script lang="ts">
  import { api, type RadioState } from '../lib/api';
  import type { WaterfallFrame } from '../lib/stream';
  import { formatMHz } from '../lib/format';
  import { MODES, type Listen } from '../lib/audio';
  import {
    GOOD_MARGIN_DB,
    dbStr,
    fccDbc,
    fccRule,
    findCarrier,
    findSpurs,
    inRange,
    judge,
    measure,
    ofcomBand,
    ofcomDbc,
    ofcomNearDbc,
    ordinal,
    overall,
    type Captured,
    type HarmonicResult,
    type NearbyScan,
    type Rated,
    type Rating,
    type Regime,
    type Spur,
    type Verdict,
  } from '../lib/harmonics';

  let { radio, listen, isAdmin }: { radio: RadioState | null; listen: Listen; isAdmin: boolean } = $props();

  // Per-browser preferences.
  const PREFS = 'sdr.harmonics';
  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem(PREFS) ?? '{}');
    } catch {
      return {};
    }
  })();
  let ofcom = $state<boolean>(saved.ofcom ?? true);
  let fcc = $state<boolean>(saved.fcc ?? true);
  let backoff = $state<number>(saved.backoff ?? 0);
  let dwell = $state<number>(saved.dwell ?? 4);
  let maxN = $state<number>(saved.maxN ?? 5);
  $effect(() => {
    try {
      localStorage.setItem(PREFS, JSON.stringify({ ofcom, fcc, backoff, dwell, maxN }));
    } catch {
      /* not available */
    }
  });

  interface Report {
    fundHz: number;
    fundPeakDb: number;
    duty: number;
    regimes: Regime[];
    harmonics: HarmonicResult[];
    near: { scan: NearbyScan; spurs: (Spur & Rated)[] } | null;
    warnings: string[];
    at: number;
  }

  const SOURCE_NOTE: Record<Spur['source'], string | null> = {
    device: null,
    other: 'Not steady with the device: there while it was off, or came and went on its own. Another signal, or the receiver.',
    image: "Probably the receiver's mirror image of the carrier, not a real emission.",
    unknown: null,
  };

  /** Rate the spurs near the carrier (only the ones that could be the device's). */
  function rateNear(scan: NearbyScan, fundHz: number, regimes: Regime[]): (Spur & Rated)[] {
    return scan.spurs.map((s) => {
      const skipped = SOURCE_NOTE[s.source];
      const verdicts: Partial<Record<Regime, Verdict>> = {};
      for (const g of regimes) {
        const limit = g === 'ofcom' ? ofcomNearDbc(fundHz, s.hz, scan.ocwHz, backoff) : fccDbc(fundHz, s.hz, backoff);
        verdicts[g] = skipped ? { rating: 'unknown', marginDb: null, limit, text: skipped } : judge(s.dbc, null, limit);
      }
      return { ...s, skipped, verdicts };
    });
  }

  let running = $state(false);
  let progress = $state<{ label: string; step: number; steps: number; frac: number } | null>(null);
  let report = $state<Report | null>(null);
  let error = $state<string | null>(null);

  const canRun = $derived(isAdmin && !!radio?.connected && !!radio?.can_tune && (ofcom || fcc));
  const band = $derived(ofcomBand(listen.hz));
  const rule = $derived(fccRule(listen.hz));
  const harmonicsShown = $derived(
    Array.from({ length: maxN - 1 }, (_, i) => i + 2).map((n) => ({
      n,
      hz: listen.hz * n,
      ok: inRange(radio?.ranges ?? [], listen.hz * n),
    })),
  );

  // ------------------------------------------------------- frame capture

  /** Frames to drop after a retune while the FFT settles. */
  const SETTLE_FRAMES = 3;
  class Cancelled extends Error {}

  let waiter: {
    centre: number;
    arrived: boolean;
    skip: number;
    frames: Captured[];
    until: number | null;
    resolve: (f: Captured[]) => void;
    reject: (e: Error) => void;
  } | null = null;
  let waitTimer: ReturnType<typeof setTimeout> | null = null;

  function finish(fn: () => void) {
    if (waitTimer) clearTimeout(waitTimer);
    waitTimer = null;
    waiter = null;
    fn();
  }

  /** Called by the parent with every waterfall frame. */
  export function push(f: WaterfallFrame) {
    const w = waiter;
    if (!w) return;
    if (f.centerHz !== w.centre) {
      if (w.arrived) {
        const e = new Error(
          'Something else retuned the radio during the check (profile rotation, or another admin). Try again.',
        );
        finish(() => w.reject(e));
      }
      return;
    }
    w.arrived = true;
    if (w.skip > 0) {
      w.skip--;
      return;
    }
    w.frames.push({
      bins: f.bins.slice(),
      sampleRate: f.sampleRate,
      centerHz: f.centerHz,
      minDb: f.minDb,
      maxDb: f.maxDb,
    });
    const now = performance.now();
    if (w.until === null) w.until = now + dwell * 1000;
    if (progress) progress.frac = Math.min(1, 1 - (w.until - now) / (dwell * 1000));
    if (now >= w.until) {
      const frames = w.frames;
      finish(() => w.resolve(frames));
    }
  }

  /** Retune to `centre` and collect `dwell` seconds of frames there. */
  async function collect(centre: number): Promise<Captured[]> {
    if (!running) throw new Cancelled();
    await api.configure({ center_hz: centre });
    if (!running) throw new Cancelled();
    return new Promise((resolve, reject) => {
      waiter = { centre, arrived: false, skip: SETTLE_FRAMES, frames: [], until: null, resolve, reject };
      waitTimer = setTimeout(
        () => finish(() => reject(new Error(`No spectrum from ${formatMHz(centre, 3)}: the radio didn't retune there.`))),
        8000 + dwell * 1000,
      );
    });
  }

  function cancel() {
    running = false;
    const w = waiter;
    if (w) finish(() => w.reject(new Cancelled()));
  }

  // ------------------------------------------------------------ the check

  async function run() {
    const r = radio;
    if (!r || !canRun) return;
    error = null;
    report = null;
    try {
      const sched = await api.schedule();
      if (sched.config.rotation) {
        error = 'Profile rotation is on and would retune the radio mid-check. Switch it off under Centre & presets on the receiver first.';
        return;
      }
    } catch {
      /* can't tell: carry on */
    }

    const regimes: Regime[] = [...(ofcom ? ['ofcom' as const] : []), ...(fcc ? ['fcc' as const] : [])];
    const sr = r.settings.sample_rate;
    const original = r.settings.center_hz;
    const ranges = r.ranges;
    // Keep the signal off the dongle's DC spike in the middle.
    const offset = Math.round((sr * 0.2) / 1000) * 1000;
    const centreFor = (hz: number) => {
      const up = Math.round((hz + offset) / 1000) * 1000;
      return inRange(ranges, up + sr / 2) ? up : Math.round((hz - offset) / 1000) * 1000;
    };
    const target = listen.hz;
    const modeBw = MODES.find((m) => m.value === listen.mode)?.bw ?? 12_500;
    const ns = Array.from({ length: maxN - 1 }, (_, i) => i + 2);
    const steps = 1 + ns.filter((n) => inRange(ranges, target * n)).length;
    const warnings: string[] = [];
    if (r.settings.gain_db === null) {
      warnings.push(
        'Auto gain is on, so the gain may differ from one frequency to the next. Set a fixed gain with the Gain knob on the receiver for a fair comparison.',
      );
    }

    running = true;
    try {
      progress = { label: `Finding the carrier near ${formatMHz(target, 4)}`, step: 1, steps, frac: 0 };
      const fundFrames = await collect(centreFor(target));
      const search = Math.max(25_000, modeBw);
      const car = findCarrier(fundFrames, target, search);
      const fund = car ? measure(fundFrames, car.hz - car.halfWidth, car.hz + car.halfWidth) : null;
      if (!car || !fund || fund.powerDb === null) {
        throw new Error(
          `No signal within ±${Math.round(search / 1000)} kHz of ${formatMHz(target, 4)}. Make sure the device is transmitting, and click its signal on the spectrum first.`,
        );
      }
      const fundDb = fund.powerDb;
      if (car.peakDb > -6) {
        warnings.push(
          `The carrier peaks at ${car.peakDb.toFixed(1)} dBFS, close to full scale: the dongle may be making harmonics of its own. Lower the gain or move the antenna away and check again; real harmonics keep the same dBc.`,
        );
      }
      if (fund.duty < 0.5) {
        warnings.push(
          `The carrier was on only ${Math.round(fund.duty * 100)}% of the time, so short bursts at a harmonic may be missed. Keep it transmitting (hold the button), or use a longer time per step.`,
        );
      }

      // Spurs near the carrier, from the same capture.
      const scan = findSpurs(fundFrames, car);
      const near = scan ? { scan, spurs: rateNear(scan, car.hz, regimes) } : null;
      if (scan) {
        const far = car.hz + 3 * scan.ocwHz;
        const tightest = Math.min(
          ...regimes.map((g) => (g === 'ofcom' ? ofcomNearDbc(car.hz, far, scan.ocwHz, backoff) : fccDbc(car.hz, far, backoff))?.dbc ?? Infinity),
        );
        if (scan.ceilingDbc > tightest) {
          warnings.push(
            `Near the carrier, only spurs above ${dbStr(scan.ceilingDbc)} dBc show, but the limit there is ${dbStr(tightest)} dBc: weaker ones could still fail. Move closer or raise the gain (without overloading).`,
          );
        }
        if (scan.spurs.some((s) => s.source === 'unknown') && !scan.sawOff) {
          warnings.push(
            "The carrier never switched off, so it can't be told whether the nearby spurs are the device's or another signal's. Stop it transmitting and see which go.",
          );
        }
        const b = ofcomBand(car.hz);
        if (ofcom && b && (car.hz - scan.ocwHz / 2 < b.lo || car.hz + scan.ocwHz / 2 > b.hi)) {
          warnings.push(`The signal spills over the edge of the ${b.label} band.`);
        }
      }

      const harmonics: HarmonicResult[] = [];
      let step = 1;
      for (const n of ns) {
        const hz = car.hz * n;
        let res: HarmonicResult;
        if (!inRange(ranges, hz)) {
          res = { n, hz, dbc: null, ceilingDbc: null, skipped: "Outside this receiver's range.", duty: 0, verdicts: {} };
        } else {
          step++;
          progress = { label: `Measuring the ${ordinal(n)} harmonic at ${formatMHz(hz, 4)}`, step, steps, frac: 0 };
          const frames = await collect(centreFor(hz));
          // FSK deviation (and drift) scale with n, so the window does too.
          const half = Math.min(car.halfWidth * n, sr * 0.2);
          const m = measure(frames, hz - half, hz + half);
          const dbc = m.powerDb === null ? null : m.powerDb - fundDb;
          const ceilingDbc = m.ceilingDb - fundDb;
          res = { n, hz, dbc, ceilingDbc, skipped: null, duty: m.duty, verdicts: {} };
        }
        for (const g of regimes) {
          const limit = g === 'ofcom' ? ofcomDbc(car.hz, hz, backoff) : fccDbc(car.hz, hz, backoff);
          res.verdicts[g] = res.skipped
            ? { rating: 'unknown', marginDb: null, limit, text: res.skipped }
            : judge(res.dbc, res.ceilingDbc, limit);
        }
        harmonics.push(res);
      }
      report = { fundHz: car.hz, fundPeakDb: car.peakDb, duty: fund.duty, regimes, harmonics, near, warnings, at: Date.now() };
    } catch (e) {
      if (!(e instanceof Cancelled)) error = e instanceof Error ? e.message : String(e);
    } finally {
      running = false;
      progress = null;
      try {
        await api.configure({ center_hz: original });
      } catch {
        /* the user can retune */
      }
    }
  }

  const LABEL: Record<Rating, string> = { good: 'Good', medium: 'Medium', bad: 'Bad', unknown: "Can't tell" };
  const REGIME: Record<Regime, string> = { ofcom: 'Ofcom (UK)', fcc: 'FCC (US)' };

</script>

<section class="panel" aria-labelledby="harm-h">
  <div class="head">
    <h2 id="harm-h">Harmonics</h2>
    <span class="muted small-text">{formatMHz(listen.hz, 4)}</span>
  </div>

  <p class="muted small-text intro">
    Measures the signal you're listening to and any spurs close to it, then retunes to its harmonics and checks them
    all against SRD limits.
  </p>

  <div class="opts">
    <fieldset class="rules">
      <legend class="sr-only">Rules to check</legend>
      <label class="check"><input type="checkbox" bind:checked={ofcom} disabled={running} /> Ofcom</label>
      <label class="check"><input type="checkbox" bind:checked={fcc} disabled={running} /> FCC</label>
    </fieldset>
    <label title="How far below the legal maximum the transmitter runs. 0 assumes the worst case.">
      Below max
      <input type="number" min="0" max="40" step="1" bind:value={backoff} disabled={running} /> dB
    </label>
    <label title="How long to watch each frequency. Longer catches more bursts.">
      Each step
      <select bind:value={dwell} disabled={running}>
        {#each [2, 4, 8, 15] as s (s)}<option value={s}>{s} s</option>{/each}
      </select>
    </label>
    <label>
      Up to
      <select bind:value={maxN} disabled={running}>
        {#each [2, 3, 4, 5] as n (n)}<option value={n}>{ordinal(n)}</option>{/each}
      </select>
    </label>
  </div>

  <ul class="ctx small-text">
    {#if ofcom}
      <li>
        <strong>Ofcom:</strong>
        {#if band}{band.label}, {band.erpMw} mW ERP max{:else}not a UK SRD band here; assuming 10 mW ERP{/if}
      </li>
    {/if}
    {#if fcc}
      <li>
        <strong>FCC:</strong>
        {#if rule.kind === 'rule'}{rule.section}, {rule.label}{:else}{rule.why}{/if}
      </li>
    {/if}
    <li class="muted">
      Will measure spurs near the carrier, then the {harmonicsShown.map((h) => `${ordinal(h.n)} ${(h.hz / 1e6).toFixed(1)}${h.ok ? '' : ' (out of range)'}`).join(', ')} MHz.
    </li>
  </ul>

  <div class="actions">
    {#if running}
      <button type="button" class="small" onclick={cancel}>Cancel</button>
    {:else}
      <button type="button" class="primary small" disabled={!canRun} onclick={run}>Check harmonics</button>
    {/if}
    {#if !isAdmin}
      <span class="muted small-text">Admins only: it retunes the radio.</span>
    {:else if radio && !radio.can_tune}
      <span class="muted small-text">This source can't retune.</span>
    {:else if !ofcom && !fcc}
      <span class="muted small-text">Pick Ofcom, FCC or both.</span>
    {/if}
  </div>

  {#if progress}
    <div class="progress" role="status">
      <span class="small-text">Step {progress.step} of {progress.steps}: {progress.label}…</span>
      <progress max="1" value={(progress.step - 1 + progress.frac) / progress.steps}></progress>
    </div>
  {/if}
  {#if error}<p class="error">{error}</p>{/if}

  {#if report}
    {@const r = report}
    <div class="summary" aria-label="Overall result">
      {#each r.regimes as g (g)}
        {@const nearSpurs = r.near?.spurs ?? []}
        {@const o = overall([...r.harmonics, ...nearSpurs], g)}
        {@const oh = overall(r.harmonics, g)}
        {@const on = overall(nearSpurs, g)}
        <div class="overall {o.rating}">
          <span class="who">{REGIME[g]}</span>
          <span class="badge {o.rating}">{LABEL[o.rating]}</span>
          <span class="muted small-text">
            {oh.checked} of {oh.total} harmonics{#if r.near}, {on.checked} nearby spur{on.checked === 1 ? '' : 's'}{/if}
          </span>
        </div>
      {/each}
    </div>
    <p class="muted small-text">
      Carrier {formatMHz(r.fundHz, 4)}, peak {r.fundPeakDb.toFixed(1)} dBFS, on {Math.round(r.duty * 100)}% of the time.
    </p>
    {#snippet verdictRows(item: Rated)}
      {#each r.regimes as g (g)}
        {@const v = item.verdicts[g]}
        {#if v}
          <div class="verdict" title={v.limit?.why ?? ''}>
            <span class="badge {v.rating}">{REGIME[g].split(' ')[0]}: {LABEL[v.rating]}</span>
            <span class="small-text">
              {v.text}
              {#if v.limit && (v.rating === 'bad' || v.rating === 'medium')}<span class="muted"> {v.limit.why}.</span>{/if}
            </span>
          </div>
        {/if}
      {/each}
    {/snippet}

    <h3 class="sub">Harmonics</h3>
    <ul class="harmonics">
      {#each r.harmonics as h (h.n)}
        <li>
          <div class="row">
            <span class="n">{ordinal(h.n)}</span>
            <span class="hz">{formatMHz(h.hz, 3)}</span>
            <span class="level">
              {#if h.skipped}–{:else if h.dbc !== null}{dbStr(h.dbc)} dBc{:else}&lt; {dbStr(h.ceilingDbc ?? 0)} dBc{/if}
            </span>
          </div>
          {@render verdictRows(h)}
        </li>
      {/each}
    </ul>

    {#if r.near}
      {@const nr = r.near}
      <h3 class="sub">Near the carrier</h3>
      <p class="muted small-text" data-testid="near-summary">
        Spurs within {formatMHz(nr.scan.lo, 3)}–{formatMHz(nr.scan.hi, 3)} from the same capture, outside a
        {(nr.scan.ocwHz / 1000).toFixed(1)} kHz channel (99% of the carrier's power, as measured; a wider declared
        channel loosens the limit close in).
        {#if nr.spurs.length}Nothing else above {dbStr(nr.scan.ceilingDbc)} dBc.{:else}None above
          {dbStr(nr.scan.ceilingDbc)} dBc.{/if}
        {#if nr.scan.combHz}
          <strong>Evenly spaced every {(nr.scan.combHz / 1000).toFixed(1)} kHz</strong>: usually a clock, switching
          regulator or the data rate leaking onto the carrier.
        {/if}
      </p>
      {#if nr.spurs.length}
        <ul class="harmonics near">
          {#each nr.spurs as s (s.hz)}
            <li>
              <div class="row">
                <span class="n off">{s.offsetHz > 0 ? '+' : '−'}{(Math.abs(s.offsetHz) / 1000).toFixed(1)} kHz</span>
                <span class="hz">{formatMHz(s.hz, 4)}</span>
                <span class="level">{dbStr(s.dbc)} dBc</span>
              </div>
              {@render verdictRows(s)}
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
    {#each r.warnings as w (w)}<p class="warn small-text">{w}</p>{/each}
    <p class="muted small-text note">
      Indicative only. An RTL-SDR isn't calibrated and its response, like your antenna's, varies by several dB across
      frequency, so "Good" needs a {GOOD_MARGIN_DB} dB margin and "Medium" means test it on proper equipment. Limits
      assume the carrier is {backoff ? `${backoff} dB below` : 'at'} the legal maximum.
    </p>
  {/if}
</section>

<style>
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .intro {
    margin: -6px 0 10px;
  }
  .opts {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 14px;
    align-items: center;
    font-size: 13px;
  }
  .opts label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .opts input[type='number'] {
    width: 4em;
    padding: 4px 6px;
  }
  .opts select {
    padding: 4px 6px;
  }
  .rules {
    display: flex;
    gap: 12px;
    border: none;
    margin: 0;
    padding: 0;
  }
  .rules label {
    color: var(--text);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .ctx {
    margin: 10px 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 3px;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
  }
  .progress {
    display: grid;
    gap: 4px;
    margin-top: 10px;
  }
  progress {
    width: 100%;
    height: 6px;
    accent-color: var(--accent);
  }
  .summary {
    display: grid;
    gap: 6px;
    margin: 12px 0 6px;
  }
  .overall {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .overall.good {
    border-color: color-mix(in srgb, var(--ok) 55%, var(--border));
  }
  .overall.medium {
    border-color: color-mix(in srgb, var(--warn) 60%, var(--border));
  }
  .overall.bad {
    border-color: color-mix(in srgb, var(--bad) 60%, var(--border));
  }
  .who {
    font-weight: 600;
    flex: 1 1 auto;
  }
  .badge {
    display: inline-block;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    border: 1px solid currentColor;
  }
  .badge.good {
    color: var(--ok);
  }
  .badge.medium {
    color: var(--warn);
  }
  .badge.bad {
    color: var(--bad);
  }
  .badge.unknown {
    color: var(--muted);
  }
  .harmonics {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .harmonics li {
    display: grid;
    gap: 4px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }
  .sub {
    margin: 12px 0 0;
    font-size: 13px;
    font-weight: 600;
  }
  .n.off {
    min-width: 5.5em;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: baseline;
    font-variant-numeric: tabular-nums;
  }
  .n {
    font-weight: 600;
    min-width: 2.2em;
  }
  .hz {
    flex: 1;
    color: var(--muted);
  }
  .verdict {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .verdict .badge {
    flex: none;
  }
  .warn {
    color: var(--warn);
    margin: 8px 0 0;
  }
  .note {
    margin: 10px 0 0;
  }
</style>
