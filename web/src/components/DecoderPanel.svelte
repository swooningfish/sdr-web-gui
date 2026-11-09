<script lang="ts">
  import { dabChannel } from '../lib/audio';
  import { formatMHz } from '../lib/format';
  import { PTY, type Listener } from '../lib/listener.svelte';

  /** What the server decodes from the channel being heard: Morse, digital
   * voice and RDS. Shown under the spectrum while one of them is on. */
  let { rx }: { rx: Listener } = $props();

  const l = $derived(rx.listen);
  const showRds = $derived(l.mode === 'wfm' && l.on);
  const showDab = $derived(l.mode === 'dab' && l.on);
  const any = $derived(rx.cwActive || rx.digitalActive || showRds || showDab);
  const dabKind = (k: string | null) => (k === 'dab+' ? 'DAB+' : k === 'mp2' ? 'DAB' : k === 'other' ? 'other' : '');

  let cwBox = $state<HTMLElement | undefined>();
  let logBox = $state<HTMLElement | undefined>();
  // Keep the newest Morse and console line in view.
  $effect(() => {
    void rx.cwText;
    if (cwBox) cwBox.scrollTop = cwBox.scrollHeight;
  });
  $effect(() => {
    void rx.dsdLog;
    void rx.consoleOn;
    if (logBox) logBox.scrollTop = logBox.scrollHeight;
  });

  const party = (p: string | null) => p ?? '?';
  const clock = (unix: number) =>
    new Date(unix * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
</script>

{#if any}
  <section class="panel decoders" aria-label="Decoded">
    {#if rx.cwActive}
      <div class="decoder">
        <div class="dec-head small-text">
          <h3>Morse{rx.cwWpm ? ` · ${Math.round(rx.cwWpm)} WPM` : ''}</h3>
          {#if rx.cwText}<button type="button" class="small" onclick={() => (rx.cwText = '')}>Clear</button>{/if}
        </div>
        <div class="cw-text" bind:this={cwBox} role="log" aria-label="Decoded Morse">
          {#if rx.cwText}{rx.cwText}{:else}<span class="muted">{l.on ? 'Listening for Morse…' : 'Press Play to decode Morse.'}</span
            >{/if}
        </div>
      </div>
    {/if}

    {#if rx.digitalActive}
      <div class="decoder digital">
        <div class="dec-head small-text">
          <h3>
            Digital voice{#if l.on && rx.syncFresh && rx.sync}
              · <span class="sync">{rx.sync.mode} sync</span>{/if}
          </h3>
          <span class="row">
            <button
              type="button"
              class="small"
              class:active={rx.consoleOn}
              aria-pressed={rx.consoleOn}
              title="Show what the decoder prints: every frame it syncs to"
              onclick={() => (rx.consoleOn = !rx.consoleOn)}>Console</button
            >
            {#if rx.heard.length || rx.dsdLog.length}
              <button
                type="button"
                class="small"
                onclick={() => {
                  rx.heard = [];
                  rx.dsdLog = [];
                }}>Clear</button
              >
            {/if}
          </span>
        </div>
        {#if rx.dsdErr}
          <p class="warn small-text">{rx.dsdErr}</p>
        {:else if !l.on}
          <p class="small-text muted">Press Play to decode digital voice.</p>
        {:else if rx.onAir.length}
          {#each rx.onAir as c (c.slot)}
            <div class="onair" class:emergency={c.emergency}>
              {#if c.protocol}<span class="badge">{c.protocol}</span>{/if}
              {#if rx.onAir.length > 1}<span class="small-text muted">TS{c.slot + 1}</span>{/if}
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
            {rx.syncFresh ? 'Receiving…' : 'Listening for DMR, D-STAR, YSF, NXDN, P25, dPMR and M17…'}
          </p>
        {/if}
        {#if rx.heard.length}
          <ul class="heard small-text" aria-label="Recent digital calls">
            {#each rx.heard as e (e.id)}
              <li title={e.summary}>
                <span class="muted num-t">{clock(e.time)}</span>
                {#if e.protocol}<span class="badge">{e.protocol}</span>{/if}
                <strong>{party(e.src)}</strong>
                <span class="muted">→ {party(e.dst)}</span>
                {#if e.code}<span class="muted">{e.code}</span>{/if}
                {#if e.enc}<span class="tag">encrypted</span>{/if}
                {#if e.emergency}<span class="tag warn">emergency</span>{/if}
                {#if e.hz !== l.hz}<span class="muted">{formatMHz(e.hz, 4)}</span>{/if}
                {#if e.alias}<span class="extra">“{e.alias}”</span>{/if}
                {#if e.text}<span class="extra">{e.text}</span>{/if}
                {#if e.gps}<span class="extra muted">{e.gps}</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#if rx.consoleOn}
          <pre class="console" bind:this={logBox} aria-label="Decoder console">{#if rx.dsdLog.length}{rx.dsdLog.join(
                '\n',
              )}{:else}<span class="muted">Nothing decoded yet.</span>{/if}</pre>
        {/if}
      </div>
    {/if}

    {#if showDab}
      <div class="decoder dab" aria-label="DAB">
        <div class="dec-head small-text">
          <h3>
            DAB{dabChannel(l.hz) ? ` ${dabChannel(l.hz)}` : ''}{rx.dab?.ensemble ? ` · ${rx.dab.ensemble}` : ''}
          </h3>
          {#if rx.dab?.codec}<span class="badge">{rx.dab.codec}</span>{/if}
        </div>
        {#if rx.dab?.error}<p class="small-text error-text">{rx.dab.error}</p>{/if}
        {#if !rx.inBand}
          <p class="small-text muted pi">
            The receiver needs to be centred on this multiplex at 2.048 MS/s{l.on ? ' (an admin can tune it)' : ''}.
          </p>
        {:else if rx.dab?.services.length}
          <ul class="stations" aria-label="Stations in this multiplex">
            {#each rx.dab.services as s (s.sid)}
              <li>
                <button
                  type="button"
                  class="station"
                  class:playing={rx.dab.service === s.sid}
                  aria-pressed={rx.dab.service === s.sid}
                  disabled={s.kind !== null && s.kind !== 'mp2' && s.kind !== 'dab+'}
                  title="SId {s.sid}{s.bitrate ? ` · ${s.bitrate} kbit/s` : ''}"
                  onclick={() => rx.pickService(s.sid)}
                >
                  <span class="st-name">{s.label}</span>
                  <span class="st-meta muted">{[dabKind(s.kind), s.bitrate ? `${s.bitrate}k` : ''].filter(Boolean).join(' · ')}</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="small-text muted pi">{rx.dab?.locked ? 'Reading the station list…' : 'Looking for the multiplex…'}</p>
        {/if}
      </div>
    {/if}

    {#if showRds}
      <div class="decoder rds" aria-label="RDS">
        <div class="dec-head small-text"><h3>RDS</h3></div>
        {#if rx.rds?.ps || rx.rds?.rt}
          <div class="ps-row">
            <strong class="ps">{rx.rds.ps?.trim() || '········'}</strong>
            {#if rx.rds.pty}<span class="pty small-text">{PTY[rx.rds.pty] ?? `PTY ${rx.rds.pty}`}</span>{/if}
          </div>
          {#if rx.rds.rt}<p class="rt small-text">{rx.rds.rt}</p>{/if}
          {#if rx.rds.pi}<p class="small-text muted pi">PI {rx.rds.pi}</p>{/if}
        {:else}
          <p class="small-text muted pi">{rx.rds?.groups ? 'Decoding…' : 'Looking for station data…'}</p>
        {/if}
      </div>
    {/if}
  </section>
{/if}

<style>
  .decoders {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 320px), 1fr));
    gap: 12px;
    padding: 12px;
  }
  .decoder {
    min-width: 0;
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
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }
  .row {
    display: flex;
    gap: 4px;
  }
  .active {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
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
  .rt,
  .pi {
    margin: 4px 0 0;
    overflow-wrap: anywhere;
  }
  .error-text {
    margin: 4px 0;
    color: var(--bad);
    overflow-wrap: anywhere;
  }
  .stations {
    list-style: none;
    margin: 4px 0 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 6px;
  }
  .station {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 6px 10px;
    text-align: left;
  }
  .station.playing {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 18%, var(--surface-2));
  }
  .st-name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .st-meta {
    font-size: 11px;
  }
  .warn {
    color: var(--warn);
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
