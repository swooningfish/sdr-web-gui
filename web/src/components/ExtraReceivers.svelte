<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ExtraReceiver } from '../lib/api';
  import ExtraReceiverRow from './ExtraReceiver.svelte';

  let list = $state<ExtraReceiver[]>([]);
  let err = $state<string | null>(null);

  async function load() {
    try {
      list = (await api.receivers()).receivers;
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    load();
    const t = setInterval(load, 4000);
    return () => clearInterval(t);
  });
</script>

<section class="panel">
  <h2>Mesh, Sensors and APRS receivers</h2>
  <p class="muted small-text">
    These decoders normally listen through the main receiver, so they only hear anything while it's tuned to their band.
    Give one a spare dongle and it listens all the time, alongside the main receiver: the dongle stays on that decoder's
    band and follows its settings. If the dongle drops out, the decoder goes back to the main receiver until it's back.
  </p>
  {#each list as rx (rx.use)}
    <ExtraReceiverRow {rx} onchange={load} />
  {/each}
  {#if err}<p class="error">{err}</p>{/if}
</section>
