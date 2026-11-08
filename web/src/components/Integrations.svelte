<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type SystemInfo } from '../lib/api';

  let sys = $state<SystemInfo | null>(null);
  let err = $state<string | null>(null);

  async function load() {
    try {
      sys = await api.system();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  });
</script>

<section class="panel">
  <h2>MQTT / Home Assistant</h2>
  {#if err}<p class="error">{err}</p>{/if}
  {#if sys}
    {#if sys.mqtt && sys.mqtt_status}
      <p class="status small-text">
        <span class="dot" class:ok={sys.mqtt_status.connected} class:bad={!sys.mqtt_status.connected}></span>
        {sys.mqtt_status.connected ? 'Connected to' : 'Not connected to'}
        {sys.mqtt_status.server}{sys.mqtt_status.error ? `: ${sys.mqtt_status.error}` : ''}
      </p>
      <table class="small-text">
        <tbody>
          <tr><th>Status</th><td><code>{sys.mqtt.prefix}/status</code></td></tr>
          <tr>
            <th>Every event</th>
            <td>{#if sys.mqtt.events}<code>{sys.mqtt.prefix}/events/&lt;decoder&gt;</code>{:else}off (<code>SDRD_MQTT_EVENTS=false</code>){/if}</td>
          </tr>
          <tr><th>Tracked devices</th><td><code>{sys.mqtt.prefix}/devices/&lt;device&gt;/state</code></td></tr>
          <tr><th>Discovery</th><td><code>{sys.mqtt.discovery_prefix}/…/config</code></td></tr>
        </tbody>
      </table>
      <p class="muted small-text">
        Devices you mark as tracked (Sensors, APRS) appear in Home Assistant; tracked APRS stations also show on its
        map. Connection settings come from <code>SDRD_MQTT_*</code> in <code>.env</code>.
      </p>
    {:else}
      <p class="muted small-text">
        Not configured. Set <code>SDRD_MQTT_HOST</code> (and optionally <code>SDRD_MQTT_USER</code> /
        <code>SDRD_MQTT_PASSWORD</code>) in <code>.env</code> and restart.
      </p>
    {/if}
  {:else if !err}
    <p class="muted">Loading…</p>
  {/if}
</section>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  table {
    border-collapse: collapse;
    margin: 6px 0 10px;
  }
  th,
  td {
    text-align: left;
    padding: 4px 12px 4px 0;
    vertical-align: top;
  }
  th {
    color: var(--muted);
    font-weight: 500;
    white-space: nowrap;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
    overflow-wrap: anywhere;
  }
</style>
