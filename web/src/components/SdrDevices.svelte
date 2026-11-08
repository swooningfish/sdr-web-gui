<script lang="ts">
  import { api, type RadioState, type SdrDevice, type SdrDevices, type SdrSource } from '../lib/api';
  import { describeRanges } from '../lib/coverage';
  import { formatMHz } from '../lib/format';

  let { radio, isAdmin }: { radio: RadioState | null; isAdmin: boolean } = $props();

  let info = $state<SdrDevices | null>(null);
  let loading = $state(false);
  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);
  /** Source tab being viewed; follows the receiver in use until changed. */
  let picked = $state<SdrSource | null>(null);
  let tcpHost = $state('host.docker.internal');
  let tcpPort = $state('1234');

  const SOURCES: { id: SdrSource; label: string }[] = [
    { id: 'rtlsdr', label: 'USB' },
    { id: 'rtl_tcp', label: 'Network (rtl_tcp)' },
    { id: 'synthetic', label: 'Simulated' },
    { id: 'replay', label: 'Recording' },
  ];
  const sources = $derived(SOURCES.filter((s) => s.id !== 'replay' || info?.replay_available));
  const source = $derived<SdrSource>(picked ?? ((radio?.source as SdrSource | undefined) || 'rtlsdr'));
  const shown = $derived(info ? info.devices.filter((d) => d.source === source) : []);
  const countOf = (s: SdrSource) => (info ? info.devices.filter((d) => d.source === s).length : 0);

  async function load() {
    loading = true;
    try {
      info = await api.sdrDevices();
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // Reload when the receiver changes (here, from the Receiver panel, or unplugged).
  $effect(() => {
    void radio?.device_id;
    void radio?.connected;
    void radio?.tuner;
    load();
  });

  async function selectId(id: string) {
    busy = id;
    err = null;
    try {
      await api.selectSdr(id);
      picked = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  function connectTcp(e: SubmitEvent) {
    e.preventDefault();
    let host = tcpHost.trim();
    const port = Number(tcpPort);
    if (!host || !Number.isInteger(port) || port < 1 || port > 65535) {
      err = 'Enter a host name or IP address and a port (rtl_tcp uses 1234 by default)';
      return;
    }
    // Bare IPv6 addresses need brackets before the port.
    if (host.includes(':') && !host.startsWith('[')) host = '[' + host + ']';
    selectId('tcp:' + host + ':' + port);
  }

  async function forget(d: SdrDevice) {
    busy = d.id;
    err = null;
    try {
      await api.forgetRtlTcp(d.id.slice('tcp:'.length));
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  const supportedCount = (d: SdrDevice) =>
    info ? info.features.filter((f, i) => f.enabled && d.supported[i]).length : 0;
  const enabledCount = $derived(info ? info.features.filter((f) => f.enabled).length : 0);
</script>

<section class="panel">
  <div class="head">
    <h2>SDR devices</h2>
    <button type="button" class="small" onclick={load} disabled={loading}>{loading ? 'Scanning…' : 'Rescan'}</button>
  </div>
  <p class="muted small-text hint">
    Features on frequencies the selected receiver can't tune are shown as disabled throughout the app, and the scheduler
    skips them.
  </p>

  <div class="source" role="radiogroup" aria-label="Source">
    <span class="source-label">Source</span>
    {#each sources as s (s.id)}
      <button
        type="button"
        role="radio"
        aria-checked={source === s.id}
        class:on={source === s.id}
        onclick={() => (picked = s.id)}
        title={radio?.source === s.id ? 'The receiver in use is from this source' : undefined}
      >
        {#if radio?.source === s.id}<span class="dot ok" aria-hidden="true"></span>{/if}
        {s.label}
        {#if s.id !== 'synthetic' && countOf(s.id)}<span class="n">{countOf(s.id)}</span>{/if}
      </button>
    {/each}
  </div>

  {#if source === 'rtl_tcp' && isAdmin}
    <form class="tcp" onsubmit={connectTcp}>
      <label>
        <span class="small-text muted">Host</span>
        <input bind:value={tcpHost} placeholder="192.168.0.20" autocomplete="off" spellcheck="false" />
      </label>
      <label class="port">
        <span class="small-text muted">Port</span>
        <input bind:value={tcpPort} inputmode="numeric" />
      </label>
      <button type="submit" class="small primary" disabled={busy !== null}>Connect</button>
    </form>
    <p class="muted small-text">
      Share a dongle from another machine with <code>rtl_tcp -a 0.0.0.0</code>. From inside Docker, the computer running
      Docker is <code>host.docker.internal</code>.
    </p>
  {/if}

  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else if shown.length === 0}
    {#if source === 'rtlsdr'}
      {#if !info.usb_supported}
        <p class="muted">This build of sdrd has no USB support.</p>
      {:else}
        <p class="muted">No USB RTL-SDR found. Plug one in and press Rescan.</p>
        <p class="muted small-text">
          In Docker the dongle has to be passed into the container. Docker Desktop on Windows and macOS can't do that by
          itself: share it with rtl_tcp and use the Network source, or on Windows attach it to WSL with usbipd-win and
          start with <code>docker-compose.usb.yml</code> (see the README).
        </p>
      {/if}
    {:else if source === 'rtl_tcp'}
      <p class="muted">No rtl_tcp servers yet.</p>
    {:else}
      <p class="muted">Nothing to select.</p>
    {/if}
  {:else}
    {#each shown as d (d.id)}
      <article class="dev" class:active={d.active}>
        <div class="dev-head">
          <div class="dev-title">
            <div class="name">
              <span class="dot" class:ok={d.active && !!radio?.connected} class:bad={d.active && !radio?.connected}></span>
              <strong>{d.name}</strong>
              {#if d.active}<span class="badge">In use</span>{/if}
            </div>
            <div class="muted small-text">
              {d.tuner ?? 'Tuner unknown'} · {describeRanges(d.ranges)}
            </div>
            {#if d.note}<div class="muted small-text">{d.note}</div>{/if}
          </div>
          {#if isAdmin}
            <div class="actions">
              {#if info.can_select && !d.active}
                <button type="button" class="small" disabled={busy !== null} onclick={() => selectId(d.id)}
                  >{busy === d.id ? 'Switching…' : 'Use this device'}</button
                >
              {/if}
              {#if d.source === 'rtl_tcp'}
                <button type="button" class="small" disabled={busy !== null} onclick={() => forget(d)}>Remove</button>
              {/if}
            </div>
          {/if}
        </div>

        {#if info.features.length}
          <p class="muted small-text count">
            {#if d.ranges.length === 0}
              Supported features unknown until the tuner can be read
            {:else}
              Receives {supportedCount(d)} of {enabledCount} enabled features
            {/if}
          </p>
          <ul class="feats">
            {#each info.features as f, i (i)}
              {@const ok = d.supported[i]}
              <li
                class:no={!ok}
                class:off={!f.enabled}
                title={ok
                  ? `${formatMHz(f.freq_hz, 3)} is in range${f.enabled ? '' : ' (turned off in settings)'}`
                  : `Disabled: ${formatMHz(f.freq_hz, 3)} is outside ${describeRanges(d.ranges)}`}
              >
                <span class="mark" aria-hidden="true">{ok ? '✓' : '✕'}</span>
                <span class="grp">{f.group}</span>
                {f.label}
                <span class="muted">{formatMHz(f.freq_hz, 3)}</span>
                {#if !ok}<span class="sr-only">(disabled, out of range)</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
      </article>
    {/each}
    {#if !isAdmin}
      <p class="muted small-text">View only — an admin can switch receivers.</p>
    {/if}
  {/if}
  {#if err && info}<p class="error">{err}</p>{/if}
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }
  .head h2 {
    margin: 0;
  }
  .hint {
    margin: 0 0 10px;
  }
  .source {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-bottom: 8px;
  }
  .source-label {
    font-weight: 600;
    margin-right: 6px;
  }
  .source button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .source button.on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }
  .source .n {
    font-size: 12px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--muted);
  }
  .tcp {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 8px;
  }
  .tcp label {
    display: grid;
    gap: 2px;
    flex: 1 1 180px;
    min-width: 0;
  }
  .tcp label.port {
    flex: 0 1 90px;
  }
  .tcp input {
    width: 100%;
    box-sizing: border-box;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .dev {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-top: 8px;
  }
  .dev.active {
    border-color: var(--accent);
  }
  .dev-head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
  }
  .dev-title {
    min-width: 0;
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    overflow-wrap: anywhere;
  }
  .badge {
    font-size: 12px;
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }
  .count {
    margin: 8px 0 4px;
  }
  .feats {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .feats li {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 12.5px;
    background: var(--surface-2);
  }
  .mark {
    color: var(--ok);
    font-weight: 700;
  }
  .grp {
    color: var(--muted);
  }
  .feats li.no {
    background: transparent;
    border-style: dashed;
    color: var(--muted);
  }
  .feats li.no .mark {
    color: var(--bad);
  }
  .feats li.off {
    opacity: 0.6;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
