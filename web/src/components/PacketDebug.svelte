<script lang="ts">
  import { api, type EventRow, type Json, type RawSignal } from '../lib/api';
  import { formatMHz, formatRate } from '../lib/format';
  import { pulseText } from '../lib/sensors';
  import { confirmAsk } from '../lib/confirm.svelte';
  import SignalView from './SignalView.svelte';

  /** The packet to show (newest unless one was picked from the history). */
  let {
    event,
    latest,
    deviceId,
    isAdmin,
    armed,
    onArm,
    onBurstDeleted,
  }: {
    event: EventRow | null;
    latest: boolean;
    deviceId: number;
    isAdmin: boolean;
    /** The device's next burst will be saved with its event. */
    armed: boolean;
    onArm: (on: boolean) => void;
    onBurstDeleted: (eventId: number) => void;
  } = $props();

  const OPEN_PREF = 'sdr.sensors.debug';
  let open = $state(
    (() => {
      try {
        return localStorage.getItem(OPEN_PREF) === '1';
      } catch {
        return false;
      }
    })(),
  );
  function toggled(e: Event) {
    open = (e.currentTarget as HTMLDetailsElement).open;
    try {
      localStorage.setItem(OPEN_PREF, open ? '1' : '0');
    } catch {
      /* per-browser convenience only */
    }
  }

  // A burst saved with the event (logged on request) comes from the database;
  // otherwise only each device's latest packet has one, in memory.
  let sig = $state<RawSignal | null>(null);
  let sigKey = $state('');
  let sigState = $state<'none' | 'loading' | 'ok' | 'gone'>('none');
  const saved = $derived(!!event?.burst);
  const iqHref = $derived(
    event?.burst ? `/api/events/${event.id}/burst.cs8` : `/api/devices/${deviceId}/raw.cs8`,
  );
  $effect(() => {
    const ev = event;
    const id = deviceId;
    if (!open || !ev || (!ev.burst && ev.rx?.signal !== true)) {
      sigState = 'none';
      return;
    }
    const key = ev.burst ? `saved:${ev.id}` : `mem:${id}:${ev.ts}`;
    if (sig && sigKey === key) {
      sigState = 'ok';
      return;
    }
    sigState = 'loading';
    (ev.burst ? api.eventBurst(ev.id) : api.deviceRaw(id))
      .then((s) => {
        if (event?.id !== ev.id) return;
        sig = s;
        sigKey = key;
        sigState = ev.burst || s.ts === ev.ts ? 'ok' : 'gone';
      })
      .catch(() => {
        if (event?.id === ev.id) sigState = 'gone';
      });
  });

  async function deleteSaved() {
    const ev = event;
    if (!ev?.burst) return;
    if (!(await confirmAsk('Delete the radio burst saved with this packet?', { ok: 'Delete', danger: true })))
      return;
    try {
      await api.deleteEventBurst(ev.id);
      onBurstDeleted(ev.id);
    } catch {
      /* stays listed; nothing else to do */
    }
  }

  const canCopy = typeof navigator !== 'undefined' && !!navigator.clipboard;
  let copied = $state(false);

  const n = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
  const signed = (hz: number) => {
    const s = hz < 0 ? '−' : '+';
    const a = Math.abs(hz);
    return a >= 1e6 ? `${s}${(a / 1e6).toFixed(4)} MHz` : `${s}${(a / 1e3).toFixed(1)} kHz`;
  };
  const DEMOD: Record<string, string> = { ASK: 'OOK / ASK (amplitude)', FSK: 'FSK (frequency)' };

  const rx = $derived<Json>((event?.rx as Json) ?? {});
  const data = $derived<Json>(event?.data ?? {});
  /** Older events, stored before debug capture, have only the decoded fields. */
  const raw = $derived(typeof rx.raw === 'string' ? rx.raw : event ? JSON.stringify(event.data) : '');

  const rows = $derived.by(() => {
    if (!event) return [];
    const out: [string, string][] = [];
    const add = (k: string, v: string | null | undefined) => {
      if (v) out.push([k, v]);
    };
    add('Received', new Date(event.ts).toLocaleString());
    const proto = n(rx.protocol ?? data.protocol);
    add('Decoder', `${data.model ?? 'unknown'}${proto !== null ? ` (rtl_433 protocol ${proto})` : ''}`);
    const mod = (rx.demod ?? data.mod) as string | undefined;
    add('Demod', mod ? (DEMOD[mod] ?? mod) : null);
    const sig = n(rx.signal_hz);
    const nominal = n(rx.nominal_hz) ?? event.freq_hz;
    if (sig !== null) {
      const off = n(rx.offset_hz);
      add('Frequency', `${formatMHz(sig, 4)}${off !== null ? ` (${signed(off)} from ${formatMHz(nominal ?? 0, 3)})` : ''}`);
    } else if (nominal) {
      add('Frequency', `${formatMHz(nominal, 3)} (listening; no measured frequency)`);
    }
    const shift = n(rx.fsk_shift_hz);
    add('FSK shift', shift !== null ? `${(shift / 1e3).toFixed(1)} kHz` : null);
    const center = n(rx.center_hz);
    const ifHz = n(rx.if_hz);
    add('IF', ifHz !== null ? `${signed(ifHz)} from radio centre${center ? ` ${formatMHz(center, 4)}` : ''}` : null);
    const ddc = n(rx.ddc_offset_hz);
    add('DDC offset', ddc !== null ? `${signed(ddc)} (decoder band centre vs radio centre)` : null);
    const level = [
      n(data.rssi) !== null ? `RSSI ${n(data.rssi)!.toFixed(1)} dB` : null,
      n(data.snr) !== null ? `SNR ${n(data.snr)!.toFixed(1)} dB` : null,
      n(data.noise) !== null ? `noise ${n(data.noise)!.toFixed(1)} dB` : null,
    ].filter(Boolean);
    add('Level', level.join(' · '));
    const sdr = n(rx.sdr_rate);
    const dec = n(rx.decimation);
    const stream = n(rx.stream_rate);
    if (sdr !== null && stream !== null) {
      add('Sample rate', `${formatRate(sdr)}${dec ? ` ÷ ${dec}` : ''} → ${formatRate(stream)} to rtl_433`);
    }
    if ('gain_db' in rx) {
      const g = n(rx.gain_db);
      const ppm = n(rx.ppm);
      add('Gain', `${g === null ? 'AGC' : `${g.toFixed(1)} dB`}${ppm ? ` · ${ppm} ppm` : ''}`);
    }
    add('Receiver', [rx.device, rx.tuner].filter((s) => typeof s === 'string' && s).join(' · '));
    add('Stream time', typeof rx.stream_time === 'string' ? rx.stream_time : null);
    return out;
  });

  async function copy() {
    try {
      const signal =
        sigState === 'ok' && sig
          ? { ...sig, envelope: undefined, freq_khz: undefined, pulses: pulseText(sig.pulses) }
          : undefined;
      await navigator.clipboard.writeText(JSON.stringify({ ts: event?.ts, rx, data, signal }, null, 2));
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      /* clipboard blocked; the text is still selectable */
    }
  }
</script>

<details class="debug" {open} ontoggle={toggled}>
  <summary>
    <h3>Packet debug</h3>
    <span class="muted small-text">{latest ? 'last packet received' : 'packet picked from history'}</span>
  </summary>
  {#if !event}
    <p class="muted small-text">No packets in this period.</p>
  {:else}
    {#if !event.rx}
      <p class="muted small-text">Received before debug capture was added: only the decoded fields are known.</p>
    {/if}
    <dl class="small-text">
      {#each rows as [k, v] (k)}
        <dt>{k}</dt>
        <dd>{v}</dd>
      {/each}
    </dl>
    <div class="sig-head">
      <h4>Radio signal</h4>
      {#if saved}<span class="badge">saved</span>{/if}
      <span class="grow"></span>
      {#if isAdmin}
        {#if armed}
          <span class="small-text waiting">Waiting for the next burst…</span>
          <button type="button" class="small" onclick={() => onArm(false)}>Cancel</button>
        {:else}
          <button
            type="button"
            class="small"
            title="Save the next packet's burst (IQ and pulses) with it, kept like the rest of its history"
            onclick={() => onArm(true)}>Log next burst</button
          >
        {/if}
        {#if saved}
          <button type="button" class="small danger" onclick={deleteSaved}>Delete saved</button>
        {/if}
      {/if}
    </div>
    {#if sigState === 'ok' && sig}
      <SignalView {sig} href={iqHref} />
    {:else if sigState === 'loading'}
      <p class="muted small-text">Loading the captured signal…</p>
    {:else if sigState === 'gone'}
      <p class="muted small-text">
        Only each device's latest packet keeps its signal (until sdrd restarts), unless it was saved with
        "Log next burst".
      </p>
    {:else}
      <p class="muted small-text">No radio signal was captured for this packet.</p>
    {/if}
    <div class="raw-head">
      <span class="small-text muted">Raw rtl_433 output</span>
      {#if canCopy}
        <button type="button" class="small" onclick={copy}>{copied ? 'Copied' : 'Copy all'}</button>
      {/if}
    </div>
    <pre class="raw">{raw}</pre>
    {#if !latest}
      <p class="muted small-text">Click the packet again in the history to go back to the latest.</p>
    {/if}
  {/if}
</details>

<style>
  .debug {
    margin-top: 6px;
  }
  summary {
    cursor: pointer;
    display: flex;
    align-items: baseline;
    gap: 8px;
    list-style: none;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary::before {
    content: '▸';
    color: var(--muted);
    font-size: 12px;
  }
  .debug[open] summary::before {
    content: '▾';
  }
  h3 {
    margin: 16px 0 6px;
    font-size: 15px;
  }
  h4 {
    margin: 0;
    font-size: 14px;
  }
  .sig-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 12px 0 4px;
  }
  .grow {
    flex: 1;
  }
  .badge {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--ok) 25%, transparent);
    color: var(--ok);
  }
  .waiting {
    color: var(--warn);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 12px;
    margin: 0 0 10px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .raw-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .raw {
    margin: 0;
    padding: 8px;
    max-height: 200px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-all;
    font-size: 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
</style>
