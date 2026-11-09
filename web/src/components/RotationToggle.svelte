<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { countdown } from '../lib/passes';
  import { schedule } from '../lib/schedule.svelte';

  /** Quick on/off for profile rotation, with what it's doing right now. */
  let { isAdmin, onsettings }: { isAdmin: boolean; onsettings?: () => void } = $props();

  let busy = $state(false);
  let err = $state<string | null>(null);

  onMount(() => schedule.use());
  const info = $derived(schedule.info);
  const now = $derived(schedule.now);
  const load = () => schedule.load();

  const on = $derived(info?.config.rotation ?? false);
  const profiles = $derived(info?.config.profiles ?? []);
  const st = $derived(info?.status ?? null);

  async function toggle() {
    if (!info) return;
    busy = true;
    err = null;
    try {
      await api.setRotation(!on);
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if info}
  <div class="rotation">
    <div class="row">
      <button
        type="button"
        role="switch"
        class="switch"
        aria-checked={on}
        aria-label="Rotation between profiles"
        disabled={!isAdmin || busy || (!on && profiles.length === 0)}
        title={!isAdmin
          ? 'Only admins can change this'
          : profiles.length === 0
            ? 'Add rotation profiles in Settings → Rotation'
            : on
              ? 'Stop rotating; the radio stays where it is'
              : 'Cycle through the rotation profiles'}
        onclick={toggle}
      >
        <span class="knob"></span>
      </button>
      <span class="label">Rotation <strong>{on ? 'on' : 'off'}</strong></span>
    </div>
    <p class="small-text muted state">
      {#if st?.mode === 'pass' && st.active}
        Paused while the {st.active.satellite} pass is on.
      {:else if on && st?.profile}
        {st.profile}{st.holding ? ', holding for an SSTV image' : st.until_ms ? `, next switch ${countdown(st.until_ms, now)}` : ''}.
      {:else if on}
        Starting…
      {:else if profiles.length}
        Off: the radio stays where you tune it.
      {:else}
        No profiles yet.
        {#if isAdmin && onsettings}<button type="button" class="link" onclick={onsettings}>Add some</button>{/if}
      {/if}
    </p>
    {#if profiles.length}
      <p class="small-text muted cycle">{profiles.map((p) => `${p.name} ${p.dwell_s} s`).join(' → ')}</p>
    {/if}
    {#if err}<p class="error small-text">{err}</p>{/if}
  </div>
{/if}

<style>
  .rotation {
    display: grid;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .switch {
    position: relative;
    width: 38px;
    height: 22px;
    padding: 0;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    flex: none;
  }
  .switch[aria-checked='true'] {
    background: var(--accent);
    border-color: var(--accent);
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--text);
    transition: transform 0.15s ease;
  }
  .switch[aria-checked='true'] .knob {
    transform: translateX(16px);
    background: var(--accent-text);
  }
  .label {
    font-size: 14px;
  }
  .state,
  .cycle {
    margin: 0;
  }
  .cycle {
    overflow-wrap: anywhere;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
  }
</style>
