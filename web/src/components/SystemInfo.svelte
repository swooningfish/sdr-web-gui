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

  const yes = (b: boolean) => (b ? 'yes' : 'no');
</script>

<section class="panel">
  <h2>Server</h2>
  {#if err}<p class="error">{err}</p>{/if}
  {#if sys}
    <dl class="small-text">
      <dt>sdrd version</dt><dd>{sys.version}</dd>
      <dt>IQ source</dt><dd>{sys.source}</dd>
      <dt>Live audio</dt>
      <dd>
        {sys.audio_max_listeners === 0 ? 'off' : `${sys.audio_listeners} of ${sys.audio_max_listeners} slots in use`}
        <span class="muted">(<code>SDRD_AUDIO_MAX_LISTENERS</code>)</span>
      </dd>
      <dt>Viewers can listen</dt><dd>{yes(sys.audio_viewers)} <span class="muted">(<code>SDRD_AUDIO_VIEWERS</code>)</span></dd>
      <dt>Viewers see sensors</dt><dd>{yes(sys.ism_viewers)} <span class="muted">(<code>SDRD_ISM_VIEWERS</code>)</span></dd>
      <dt>Event history</dt>
      <dd>{sys.event_retention_days} days <span class="muted">(SSTV images and pass recordings are kept)</span></dd>
      <dt>Sign-in lasts</dt><dd>{sys.session_hours} h</dd>
      <dt>Behind a proxy</dt><dd>{yes(sys.trust_proxy)}</dd>
      <dt>Map tiles</dt><dd><code>{sys.map_tiles}</code></dd>
    </dl>
    <p class="muted small-text">These come from <code>SDRD_*</code> settings in <code>.env</code>; change them there and restart.</p>
  {:else if !err}
    <p class="muted">Loading…</p>
  {/if}
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
