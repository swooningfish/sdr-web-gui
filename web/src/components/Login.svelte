<script lang="ts">
  import { api, type User } from '../lib/api';

  let { onlogin }: { onlogin: (u: User) => void } = $props();

  let username = $state('');
  let password = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    err = null;
    try {
      const u = await api.login(username.trim(), password);
      password = '';
      onlogin(u);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<main class="login">
  <form class="panel" onsubmit={submit}>
    <h1>SDR Monitor</h1>
    <p class="muted">Sign in to continue.</p>
    <label>
      Username
      <input autocomplete="username" bind:value={username} required />
    </label>
    <label>
      Password
      <input type="password" autocomplete="current-password" bind:value={password} required />
    </label>
    {#if err}<p class="error">{err}</p>{/if}
    <button type="submit" class="primary" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
  </form>
</main>

<style>
  .login {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: 16px;
  }
  form {
    width: min(360px, 100%);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h1 {
    margin: 0;
    font-size: 22px;
  }
  p {
    margin: 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 14px;
    color: var(--muted);
  }
</style>
