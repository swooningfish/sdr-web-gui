<script lang="ts">
  /** Model · ID · channel · type, with the ID in monospace so it's easy to compare. */
  import type { Device } from '../lib/api';

  let { device, showModel = true }: { device: Device; showModel?: boolean } = $props();
</script>

<span class="ident">
  {#if showModel}<span>{device.model}</span>{/if}
  {#if device.dev_id}<span class="id" title="Transmitter ID">ID <code>{device.dev_id}</code></span>{/if}
  {#if device.channel}<span>ch {device.channel}</span>{/if}
  {#if device.kind}<span>{device.kind}</span>{/if}
</span>

<style>
  .ident {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 0 6px;
    color: var(--muted);
    font-size: 13px;
  }
  /* Middle-dot separators between whichever parts are present. */
  .ident > span + span::before {
    content: '·';
    margin-right: 6px;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
  }
</style>
