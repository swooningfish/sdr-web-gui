<script lang="ts">
  import { api, type User } from '../lib/api';

  let { user }: { user: User } = $props();

  let current = $state('');
  let next = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let msg = $state<{ ok: boolean; text: string } | null>(null);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (next !== confirm) {
      msg = { ok: false, text: 'New passwords do not match' };
      return;
    }
    busy = true;
    msg = null;
    try {
      await api.changePassword(current, next);
      current = next = confirm = '';
      msg = { ok: true, text: 'Password changed. Other sessions have been signed out.' };
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>Account</h2>
  <p class="muted">Signed in as <strong>{user.username}</strong> ({user.role}).</p>
  <form onsubmit={submit}>
    <input type="text" autocomplete="username" value={user.username} hidden readonly />
    <label>Current password <input type="password" autocomplete="current-password" bind:value={current} required /></label>
    <label>New password (min 10 chars) <input type="password" autocomplete="new-password" minlength="10" bind:value={next} required /></label>
    <label>Confirm new password <input type="password" autocomplete="new-password" minlength="10" bind:value={confirm} required /></label>
    {#if msg}<p class={msg.ok ? 'success' : 'error'}>{msg.text}</p>{/if}
    <button type="submit" class="primary" disabled={busy}>Change password</button>
  </form>
</section>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 380px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 14px;
    color: var(--muted);
  }
  button {
    align-self: flex-start;
  }
</style>
