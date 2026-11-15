<script lang="ts">
  import { api, type MeshInfo, type MeshMessage, type MeshNode, type MeshPacket, type RadioState } from '../lib/api';
  import { formatMHz, relTime } from '../lib/format';
  import { PROTO_NAMES, battery, clock, nodeName, uptime } from '../lib/mesh';
  import { confirmAsk } from '../lib/confirm.svelte';
  import MeshMap from './MeshMap.svelte';

  let {
    isAdmin,
    radio,
    active,
    onsettings,
  }: {
    isAdmin: boolean;
    radio: RadioState | null;
    /** The tab is showing: poll only then. */
    active: boolean;
    onsettings: () => void;
  } = $props();

  type View = 'nodes' | 'messages' | 'packets';
  let view = $state<View>('nodes');
  let info = $state<MeshInfo | null>(null);
  let nodes = $state<MeshNode[]>([]);
  let messages = $state<MeshMessage[]>([]);
  let packets = $state<MeshPacket[]>([]);
  let selected = $state<string | null>(null);
  let proto = $state<'all' | 'meshtastic' | 'meshcore'>('all');
  let channel = $state<string>('all');
  let showRepeats = $state(false);
  let openPacket = $state<number | null>(null);
  let now = $state(Date.now());
  let err = $state<string | null>(null);
  let busy = $state(false);

  const sel = $derived(nodes.find((n) => n.key === selected) ?? null);
  const shownNodes = $derived(nodes.filter((n) => proto === 'all' || n.proto === proto));
  /** Channels heard, as "proto|name". */
  const channels = $derived([...new Set(messages.map((m) => `${m.proto}|${m.channel ?? ''}`))].sort());
  const shownMessages = $derived(
    messages
      .filter((m) => (proto === 'all' || m.proto === proto) && (channel === 'all' || `${m.proto}|${m.channel ?? ''}` === channel))
      .slice()
      .reverse(),
  );
  const shownPackets = $derived(packets.filter((p) => (proto === 'all' || p.proto === proto) && (showRepeats || !p.repeat)));
  const presetName = (id: string) => info?.presets.find((p) => p.id === id)?.name ?? id;

  async function poll() {
    try {
      const [i, n] = await Promise.all([api.mesh(), api.meshNodes()]);
      info = i;
      nodes = n.nodes;
      now = n.now;
      if (view === 'messages') messages = (await api.meshMessages()).messages;
      if (view === 'packets') packets = (await api.meshPackets()).packets;
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  $effect(() => {
    if (!active) return;
    void view;
    poll();
    const t = setInterval(poll, 3000);
    return () => clearInterval(t);
  });

  /** Tune the main receiver so all the presets fit (admins). */
  async function tune() {
    if (!info) return;
    busy = true;
    try {
      const rate = radio && radio.settings.sample_rate >= 1_024_000 ? radio.settings.sample_rate : 1_024_000;
      await api.configure({ center_hz: info.status.tune_hz, sample_rate: rate });
      await poll();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function forget(n: MeshNode) {
    if (!(await confirmAsk(`Forget ${nodeName(n)}? It comes back if it's heard again.`, { ok: 'Forget', danger: true }))) return;
    try {
      await api.deleteMeshNodes([n.key]);
      selected = null;
      await poll();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  const fmtSnr = (v: number | null) => (v === null ? '' : `${v > 0 ? '+' : ''}${v.toFixed(1)}`);
</script>

<div class="mesh">
  <section class="panel">
    <div class="head">
      <h2>Mesh: Meshtastic and MeshCore</h2>
      {#if info}
        <div class="pills small-text">
          <span class="pill"><span class="dot" class:ok={info.status.listening}></span>{info.status.listening ? 'Listening' : 'Not listening'}</span>
          <span class="pill">{info.status.nodes_24h.meshtastic} Meshtastic · {info.status.nodes_24h.meshcore} MeshCore nodes (24 h)</span>
          {#if info.status.crc_errors}<span class="pill" title="Packets heard but damaged">{info.status.crc_errors} CRC errors</span>{/if}
        </div>
      {/if}
    </div>
    {#if info}
      {#if !info.status.enabled}
        <p class="hint small-text">
          Mesh decoding is off.
          {#if isAdmin}<button type="button" class="link" onclick={onsettings}>Settings → Decoders</button>{/if}
        </p>
      {:else if !info.main_tuned}
        <p class="hint small-text">
          <span>
            Packets are decoded while the receiver is tuned near {formatMHz(info.status.tune_hz, 3)} MHz (UK/EU 868 MHz
            mesh channels). Use a rotation profile there to check in now and then.
          </span>
          {#if isAdmin}
            <button type="button" class="small primary" disabled={busy} onclick={tune}>Tune to {formatMHz(info.status.tune_hz, 3)} MHz</button>
          {/if}
        </p>
      {/if}
      <div class="presets small-text">
        {#each info.status.presets.filter((p) => p.enabled) as p (p.preset.id)}
          <span class="pill" class:dim={!p.in_band} title="SF{p.preset.sf}, {p.preset.bw / 1000} kHz, CR 4/{4 + p.preset.cr}">
            <span class="dot" class:ok={p.in_band}></span>
            {p.preset.name} · {formatMHz(p.preset.freq_hz, 3)} · {p.packets} packets{p.last_ms ? `, last ${relTime(p.last_ms, now)}` : ''}
          </span>
        {/each}
      </div>
    {/if}
    <div class="filters">
      <div class="seg" role="group" aria-label="View">
        {#each [['nodes', 'Nodes'], ['messages', 'Messages'], ['packets', 'Packet log']] as [v, l] (v)}
          <button type="button" class="small" class:active={view === v} aria-pressed={view === v} onclick={() => (view = v as View)}>{l}</button>
        {/each}
      </div>
      <div class="seg" role="group" aria-label="Network">
        {#each [['all', 'Both'], ['meshtastic', 'Meshtastic'], ['meshcore', 'MeshCore']] as [v, l] (v)}
          <button type="button" class="small" class:active={proto === v} aria-pressed={proto === v} onclick={() => (proto = v as typeof proto)}>{l}</button>
        {/each}
      </div>
    </div>
    {#if err}<p class="error">{err}</p>{/if}
  </section>

  {#if view === 'nodes'}
    <div class="main">
      <section class="panel map-panel">
        {#if info}
          <MeshMap nodes={shownNodes} {selected} tileUrl={info.tile_url} onselect={(k) => (selected = k)} />
        {/if}
        <p class="legend small-text">
          <span class="key meshtastic"></span> Meshtastic <span class="key meshcore"></span> MeshCore · nodes that sent a position
        </p>
      </section>
      <aside class="panel side">
        {#if sel}
          <div class="head">
            <h2>{nodeName(sel)}</h2>
            <button type="button" class="small" onclick={() => (selected = null)}>Back to list</button>
          </div>
          <dl class="small-text">
            <dt>Network</dt><dd>{PROTO_NAMES[sel.proto]} ({presetName(sel.preset)})</dd>
            <dt>ID</dt><dd class="mono">{sel.id}</dd>
            {#if sel.short_name}<dt>Short name</dt><dd>{sel.short_name}</dd>{/if}
            {#if sel.hw}<dt>Hardware</dt><dd>{sel.hw}</dd>{/if}
            {#if sel.role}<dt>Role</dt><dd>{sel.role}</dd>{/if}
            {#if sel.lat !== null}
              <dt>Position</dt>
              <dd>{sel.lat.toFixed(5)}, {sel.lon?.toFixed(5)}{sel.alt !== null ? `, ${sel.alt} m` : ''}{sel.pos_ms ? ` (${relTime(sel.pos_ms, now)})` : ''}</dd>
            {/if}
            {#if sel.battery !== null}<dt>Battery</dt><dd>{battery(sel)}{sel.voltage !== null ? ` · ${sel.voltage.toFixed(2)} V` : ''}</dd>{/if}
            {#if sel.channel_util !== null}<dt>Channel use</dt><dd>{sel.channel_util.toFixed(1)}% (own transmit {sel.air_util_tx?.toFixed(1) ?? '–'}%)</dd>{/if}
            {#if sel.uptime_s !== null}<dt>Up for</dt><dd>{uptime(sel.uptime_s)}</dd>{/if}
            {#if sel.temperature !== null}<dt>Temperature</dt><dd>{sel.temperature.toFixed(1)} °C</dd>{/if}
            {#if sel.humidity !== null}<dt>Humidity</dt><dd>{sel.humidity.toFixed(0)}%</dd>{/if}
            {#if sel.pressure !== null}<dt>Pressure</dt><dd>{sel.pressure.toFixed(1)} hPa</dd>{/if}
            <dt>Signal</dt><dd>{sel.snr === null ? '–' : `${fmtSnr(sel.snr)} dB SNR`}{sel.hops !== null ? ` · ${sel.hops === 0 ? 'direct' : `${sel.hops} hop${sel.hops === 1 ? '' : 's'}`}` : ''}</dd>
            <dt>Packets</dt><dd>{sel.packets.toLocaleString('en-GB')}</dd>
            <dt>First heard</dt><dd>{relTime(sel.first_ms, now)}</dd>
            <dt>Last heard</dt><dd>{relTime(sel.last_ms, now)}</dd>
          </dl>
          {#if isAdmin}<button type="button" class="small danger" onclick={() => forget(sel)}>Forget this node</button>{/if}
        {:else}
          <h2>Nodes</h2>
          {#if shownNodes.length === 0}
            <p class="muted small-text">No nodes heard yet.</p>
          {:else}
            <div class="table-wrap">
              <table class="small-text">
                <thead>
                  <tr><th>Name</th><th>Kind</th><th class="num">SNR</th><th class="num">Hops</th><th class="num">Battery</th><th class="num">Heard</th></tr>
                </thead>
                <tbody>
                  {#each shownNodes as n (n.key)}
                    <tr
                      tabindex="0"
                      class:nopos={n.lat === null}
                      onclick={() => (selected = n.key)}
                      onkeydown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          selected = n.key;
                        }
                      }}
                    >
                      <td><span class="key {n.proto}" title={PROTO_NAMES[n.proto]}></span><strong>{nodeName(n)}</strong>{#if n.short_name && n.long_name}<span class="muted"> {n.short_name}</span>{/if}</td>
                      <td>{n.role ?? ''}</td>
                      <td class="num">{fmtSnr(n.snr)}</td>
                      <td class="num">{n.hops ?? ''}</td>
                      <td class="num">{battery(n)}</td>
                      <td class="num">{relTime(n.last_ms, now)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
            <p class="muted small-text">SNR in dB of the last direct copy. Greyed: no position.</p>
          {/if}
        {/if}
      </aside>
    </div>
  {:else if view === 'messages'}
    <section class="panel">
      <div class="seg wrap" role="group" aria-label="Channel">
        <button type="button" class="small" class:active={channel === 'all'} onclick={() => (channel = 'all')}>All channels</button>
        {#each channels as c (c)}
          {@const [p, name] = c.split('|')}
          <button type="button" class="small" class:active={channel === c} onclick={() => (channel = c)}>
            <span class="key {p}"></span>{name || 'unknown'}
          </button>
        {/each}
      </div>
      {#if shownMessages.length === 0}
        <p class="muted small-text">No channel messages yet. Direct messages are end-to-end encrypted: they show in the packet log.</p>
      {:else}
        <ol class="messages">
          {#each shownMessages as m (m.id)}
            <li>
              <div class="meta small-text muted">
                <span class="key {m.proto}" title={PROTO_NAMES[m.proto]}></span>
                <strong class="from">{m.from_name ?? 'unknown'}</strong>
                in {m.channel ?? '?'}{#if m.private}<span class="tag" title="Private channel: admins only">private</span>{/if}
                · {clock(m.ts_ms)}{m.hops !== null ? ` · ${m.hops === 0 ? 'direct' : `${m.hops} hop${m.hops === 1 ? '' : 's'}`}` : ''}{m.snr !== null ? ` · ${fmtSnr(m.snr)} dB` : ''}
              </div>
              <div class="text">{m.text}</div>
            </li>
          {/each}
        </ol>
      {/if}
    </section>
  {:else}
    <section class="panel">
      <label class="check small-text"><input type="checkbox" bind:checked={showRepeats} /> Show repeats (the same packet relayed again)</label>
      {#if shownPackets.length === 0}
        <p class="muted small-text">No packets yet.</p>
      {:else}
        <div class="table-wrap">
          <table class="small-text packets">
            <thead>
              <tr><th>Time</th><th>Network</th><th>Type</th><th>From → to</th><th>Content</th><th class="num">SNR</th><th class="num">Hops</th></tr>
            </thead>
            <tbody>
              {#each shownPackets as p, i (`${p.ts_ms}-${i}`)}
                <tr class:bad={!p.crc_ok} class:repeat={p.repeat} tabindex="0" onclick={() => (openPacket = openPacket === i ? null : i)}
                  onkeydown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      openPacket = openPacket === i ? null : i;
                    }
                  }}>
                  <td>{clock(p.ts_ms)}</td>
                  <td><span class="key {p.proto}"></span>{presetName(p.preset)}</td>
                  <td>{p.kind}{p.repeat ? ' (repeat)' : ''}</td>
                  <td>{p.from_name ?? p.from ?? ''}{p.to ? ` → ${p.to}` : ''}</td>
                  <td class="content">{#if p.channel}<span class="tag">{p.channel}</span>{/if}{p.summary}</td>
                  <td class="num">{fmtSnr(p.snr)}</td>
                  <td class="num">{p.hops ?? ''}</td>
                </tr>
                {#if openPacket === i}
                  <tr class="detail">
                    <td colspan="7">
                      {p.len} bytes · offset {p.offset_hz} Hz{p.from ? ` · from ${p.from}` : ''}
                      {#if p.raw}<pre class="mono">{p.raw.match(/.{1,64}/g)?.join('\n')}</pre>{/if}
                    </td>
                  </tr>
                {/if}
              {/each}
            </tbody>
          </table>
        </div>
        <p class="muted small-text">Newest first; click a row for the raw bytes. Red: damaged (CRC error).</p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .mesh {
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
  .pills,
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .presets {
    margin-top: 10px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .pill.dim {
    opacity: 0.6;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted);
    flex: none;
  }
  .dot.ok {
    background: var(--ok, #2e9d5b);
  }
  .hint {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 12px;
    padding: 8px 10px;
    margin: 10px 0 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 14px;
    margin-top: 10px;
  }
  .seg {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .seg.wrap {
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .seg .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
  }
  .main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 440px;
    gap: 16px;
    align-items: stretch;
  }
  .map-panel {
    padding: 6px;
    min-height: 520px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .legend {
    margin: 0;
    padding: 0 4px 2px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .key {
    display: inline-block;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    margin-right: 5px;
    vertical-align: baseline;
    flex: none;
  }
  .key.meshtastic {
    background: #2e9d5b;
  }
  .key.meshcore {
    background: #7a4fd0;
  }
  .side {
    min-width: 0;
    max-height: 720px;
    overflow-y: auto;
  }
  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .mono,
  pre {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  .table-wrap {
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  th {
    text-align: left;
    padding: 6px 4px;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
    white-space: nowrap;
  }
  th.num,
  td.num {
    text-align: right;
  }
  td {
    padding: 6px 4px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  td.content {
    white-space: normal;
    min-width: 16em;
    overflow-wrap: anywhere;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover {
    background: var(--surface-2);
  }
  tr.nopos td,
  tr.repeat td {
    color: var(--muted);
  }
  tr.bad td {
    color: var(--danger, #c0392b);
  }
  tr.detail {
    cursor: default;
  }
  tr.detail td {
    white-space: normal;
  }
  tr.detail pre {
    margin: 6px 0 0;
    padding: 8px;
    background: var(--surface-2);
    border-radius: 6px;
    font-size: 12px;
    overflow-x: auto;
  }
  .tag {
    display: inline-block;
    padding: 0 6px;
    margin: 0 6px 0 4px;
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 11px;
  }
  .messages {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .messages li {
    padding: 8px 4px;
    border-bottom: 1px solid var(--border);
  }
  .messages .from {
    color: var(--text);
  }
  .messages .text {
    margin-top: 2px;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
  @media (max-width: 1000px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
    .map-panel {
      min-height: 380px;
    }
    .side {
      max-height: none;
    }
  }
</style>
