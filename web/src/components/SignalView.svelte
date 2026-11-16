<script lang="ts">
  import type { RawSignal } from '../lib/api';
  import { pulseGroups, pulseText } from '../lib/sensors';

  let { sig, deviceId }: { sig: RawSignal; deviceId: number } = $props();

  const W = 600;
  const H = 90;
  const FH = 60;

  const ms = (us: number) => (Math.abs(us) >= 1000 ? `${(us / 1000).toFixed(1)} ms` : `${Math.round(us)} µs`);

  function area(points: number[]): string {
    const n = points.length;
    if (!n) return '';
    const x = (i: number) => ((i + 0.5) / n) * W;
    const y = (v: number) => H - 2 - (v / 1000) * (H - 6);
    let d = `M0 ${H} `;
    points.forEach((v, i) => (d += `L${x(i).toFixed(1)} ${y(v).toFixed(1)} `));
    return d + `L${W} ${H} Z`;
  }
  const envPath = $derived(area(sig.envelope));
  /** Only worth a second plot when the burst is longer than the zoomed part. */
  const detailPath = $derived(sig.detail_us < sig.duration_us - 1 ? area(sig.detail) : null);

  const freq = $derived.by(() => {
    const vals = sig.freq_khz.filter((v): v is number => v !== null);
    if (vals.length < 2) return null;
    let lo = Math.min(...vals);
    let hi = Math.max(...vals);
    if (hi - lo < 2) {
      lo -= 1;
      hi += 1;
    }
    const n = sig.freq_khz.length;
    const y = (v: number) => FH - 4 - ((v - lo) / (hi - lo)) * (FH - 8);
    let d = '';
    let pen = false;
    sig.freq_khz.forEach((v, i) => {
      if (v === null) {
        pen = false;
        return;
      }
      d += `${pen ? 'L' : 'M'}${(((i + 0.5) / n) * W).toFixed(1)} ${y(v).toFixed(1)} `;
      pen = true;
    });
    return { d, lo, hi };
  });

  const groups = $derived(pulseGroups(sig.pulses));
  const text = $derived(pulseText(sig.pulses));
  const highName = $derived(sig.slicer === 'frequency' ? 'Mark' : 'Pulse');
  const lowName = $derived(sig.slicer === 'frequency' ? 'Space' : 'Gap');
</script>

<div class="signal">
  <div class="meta small-text muted">
    {ms(sig.duration_us)} burst · {Math.round(sig.snr_db)} dB above noise · {sig.iq_samples.toLocaleString()} samples at
    {sig.rate / 1000} kS/s
  </div>
  <figure>
    <figcaption class="small-text muted">Amplitude</figcaption>
    <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="Signal amplitude over the burst">
      <path d={envPath} class="env" />
    </svg>
  </figure>
  {#if freq}
    <figure>
      <figcaption class="small-text muted">
        Frequency offset ({freq.lo.toFixed(1)} to {freq.hi.toFixed(1)} kHz from {(sig.freq_hz / 1e6).toFixed(3)} MHz)
      </figcaption>
      <svg class="short" viewBox="0 0 {W} {FH}" preserveAspectRatio="none" role="img" aria-label="Signal frequency over the burst">
        <path d={freq.d} class="freq" />
      </svg>
    </figure>
  {/if}
  <div class="axis small-text muted"><span>0</span><span>{ms(sig.duration_us)}</span></div>
  {#if detailPath}
    <figure>
      <figcaption class="small-text muted">Amplitude, first {ms(sig.detail_us)}</figcaption>
      <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="Signal amplitude at the start of the burst">
        <path d={detailPath} class="env" />
      </svg>
    </figure>
    <div class="axis small-text muted"><span>0</span><span>{ms(sig.detail_us)}</span></div>
  {/if}

  {#if sig.pulses.length}
    <dl class="small-text">
      <dt>{highName}s</dt>
      <dd>{groups.high.map((g) => `${g.us} µs ×${g.count}`).join(' · ') || 'none'}</dd>
      <dt>{lowName}s</dt>
      <dd>{groups.low.map((g) => `${g.us} µs ×${g.count}`).join(' · ') || 'none'}</dd>
      <dt>Sliced on</dt>
      <dd>{sig.slicer === 'frequency' ? 'frequency (FSK mark / space)' : 'carrier level (on / off)'}</dd>
    </dl>
    <div class="small-text muted">
      Pulse timings, µs (+ {highName.toLowerCase()}, − {lowName.toLowerCase()}){sig.pulses_truncated
        ? `, first ${sig.pulses.length}`
        : ''}
    </div>
    <pre class="raw">{text}</pre>
  {:else}
    <p class="small-text muted">No clear pulses in this burst.</p>
  {/if}
  <p class="small-text">
    <a href={`/api/devices/${deviceId}/raw.cs8`} download={sig.file_name}>Download IQ ({sig.file_name})</a>
    <span class="muted">· replay with <code>rtl_433 -r {sig.file_name} -A</code></span>
  </p>
</div>

<style>
  .signal {
    margin: 4px 0 10px;
  }
  figure {
    margin: 6px 0 0;
  }
  svg {
    display: block;
    width: 100%;
    height: 90px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  svg.short {
    height: 60px;
  }
  .env {
    fill: color-mix(in srgb, var(--accent) 45%, transparent);
    stroke: var(--accent);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .freq {
    fill: none;
    stroke: var(--warn);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
  }
  .axis {
    display: flex;
    justify-content: space-between;
    margin: 2px 0 8px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 12px;
    margin: 0 0 8px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .raw {
    margin: 4px 0 0;
    padding: 8px;
    max-height: 140px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  p {
    margin: 8px 0 0;
    overflow-wrap: anywhere;
  }
</style>
