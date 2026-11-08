<script lang="ts">
  import type { RadioState, User } from '../lib/api';
  import SdrDevices from './SdrDevices.svelte';
  import DecoderSettings from './DecoderSettings.svelte';
  import AprsSettings from './AprsSettings.svelte';
  import SstvSettings from './SstvSettings.svelte';
  import LocationSettings from './LocationSettings.svelte';
  import ScheduleSettings from './ScheduleSettings.svelte';
  import Integrations from './Integrations.svelte';
  import Users from './Users.svelte';
  import SystemInfo from './SystemInfo.svelte';

  export type SettingsTab = 'receiver' | 'decoders' | 'passes' | 'integrations' | 'users' | 'system';

  let {
    radio,
    me,
    tab = $bindable('receiver'),
  }: { radio: RadioState | null; me: User; tab?: SettingsTab } = $props();

  const TABS: { id: SettingsTab; label: string }[] = [
    { id: 'receiver', label: 'Receiver' },
    { id: 'decoders', label: 'Decoders' },
    { id: 'passes', label: 'Passes' },
    { id: 'integrations', label: 'Integrations' },
    { id: 'users', label: 'Users' },
    { id: 'system', label: 'System' },
  ];
</script>

<div class="settings">
  <div class="tabs" role="tablist" aria-label="Settings sections">
    {#each TABS as t (t.id)}
      <button
        type="button"
        role="tab"
        id="settings-tab-{t.id}"
        aria-selected={tab === t.id}
        aria-controls="settings-panel"
        class:active={tab === t.id}
        onclick={() => (tab = t.id)}>{t.label}</button
      >
    {/each}
  </div>

  <div class="panels" id="settings-panel" role="tabpanel" aria-labelledby="settings-tab-{tab}">
    {#if tab === 'receiver'}
      <SdrDevices {radio} isAdmin={true} />
    {:else if tab === 'decoders'}
      <DecoderSettings {radio} />
      <AprsSettings {radio} />
      <SstvSettings {radio} />
    {:else if tab === 'passes'}
      <LocationSettings />
      <ScheduleSettings {radio} />
    {:else if tab === 'integrations'}
      <Integrations />
    {:else if tab === 'users'}
      <Users {me} />
    {:else}
      <SystemInfo />
    {/if}
  </div>
</div>

<style>
  .settings {
    display: grid;
    grid-template-columns: 180px minmax(0, 1fr);
    gap: 16px;
    align-items: start;
  }
  .tabs {
    display: grid;
    gap: 4px;
    position: sticky;
    top: 70px;
  }
  .tabs button {
    text-align: left;
    background: transparent;
    border-color: transparent;
  }
  .tabs button.active {
    background: var(--surface-2);
    border-color: var(--border);
    font-weight: 600;
  }
  .panels {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 380px), 1fr));
    gap: 16px;
    align-items: start;
    min-width: 0;
  }
  @media (max-width: 760px) {
    .settings {
      grid-template-columns: minmax(0, 1fr);
    }
    .tabs {
      position: static;
      display: flex;
      overflow-x: auto;
      scrollbar-width: none;
      border-bottom: 1px solid var(--border);
      padding-bottom: 6px;
    }
    .tabs button {
      flex: none;
    }
  }
</style>
