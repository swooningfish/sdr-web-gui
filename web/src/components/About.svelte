<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';

  const webVersion = __APP_VERSION__;
  let serverVersion = $state<string | null>(null);

  onMount(async () => {
    try {
      serverVersion = (await api.system()).version;
    } catch {
      serverVersion = null;
    }
  });

  const REPO = 'https://github.com/swooningfish/sdr-web-gui';

  const PAGES: { name: string; text: string }[] = [
    { name: 'Spectrum', text: 'The receiver front panel, live spectrum and waterfall. Tune, pick a mode and listen in the browser; bookmarks open from the side.' },
    { name: 'Dashboard', text: 'Tiles for the sensors you care about most. Drag to reorder.' },
    { name: 'Sensors', text: 'Everything rtl_433 has decoded (TPMS, weather stations, meters…), with history, raw packets and an estimated distance.' },
    { name: 'APRS', text: 'Position reports and messages decoded by Direwolf, on a map and in a list.' },
    { name: 'Aircraft', text: 'ADS-B aircraft on a map or globe, with the receiver’s range.' },
    { name: 'Mesh', text: 'Meshtastic and MeshCore nodes and messages heard over LoRa.' },
    { name: 'Satellites', text: 'Upcoming passes, automatic pass recordings and decoded SSTV pictures (including ARISS events).' },
    { name: 'Settings', text: 'Receivers, decoders, rotation, pass scheduling, integrations (MQTT), users and server facts. Admins only.' },
  ];
</script>

<section class="panel">
  <h2>About</h2>
  <dl class="small-text">
    <dt>Web app version</dt><dd>{webVersion}</dd>
    <dt>sdrd version</dt><dd>{serverVersion ?? '…'}</dd>
    <dt>Source</dt><dd><a href={REPO} target="_blank" rel="noopener">{REPO.replace('https://', '')}</a></dd>
  </dl>
  <p class="muted small-text">
    A self-hosted, browser-controlled RTL-SDR monitor, inspired by
    <a href="https://github.com/adswill/OnAir" target="_blank" rel="noopener">OnAir</a>.
  </p>
</section>

<section class="panel">
  <h2>Help</h2>
  <dl class="small-text">
    {#each PAGES as p (p.name)}
      <dt>{p.name}</dt><dd>{p.text}</dd>
    {/each}
  </dl>
  <p class="muted small-text">
    Pages can be linked to: the address bar’s <code>#…</code> part names the page and tab.
    Full documentation is in the <a href="{REPO}#readme" target="_blank" rel="noopener">README</a>.
  </p>
</section>

<style>
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 16px;
    margin: 0 0 10px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
  }
</style>
