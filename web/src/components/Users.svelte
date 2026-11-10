<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Role, type User, type UserRow } from '../lib/api';
  import { confirmAsk } from '../lib/confirm.svelte';

  let { me }: { me: User } = $props();

  let users = $state<UserRow[]>([]);
  let username = $state('');
  let password = $state('');
  let role = $state<Role>('viewer');
  let busy = $state(false);
  let err = $state<string | null>(null);

  async function load() {
    try {
      users = await api.users();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    err = null;
    try {
      await api.createUser(username.trim(), password, role);
      username = password = '';
      role = 'viewer';
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function remove(u: UserRow) {
    if (!(await confirmAsk(`Delete user "${u.username}"? They will be signed out immediately.`, { ok: 'Delete', danger: true }))) return;
    err = null;
    try {
      await api.deleteUser(u.id);
      await load();
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(load);
</script>

<section class="panel">
  <h2>Users</h2>
  <div class="table-wrap">
    <table>
      <thead><tr><th>Username</th><th>Role</th><th>Created</th><th></th></tr></thead>
      <tbody>
        {#each users as u (u.id)}
          <tr>
            <td>{u.username}{u.id === me.id ? ' (you)' : ''}</td>
            <td>{u.role}</td>
            <td class="muted">{new Date(u.created_at * 1000).toLocaleDateString()}</td>
            <td class="actions">
              {#if u.id !== me.id}
                <button type="button" class="small danger" onclick={() => remove(u)}>Delete</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  <h3>Add user</h3>
  <form onsubmit={add}>
    <input placeholder="username" autocomplete="off" bind:value={username} required maxlength="32" />
    <input type="password" placeholder="password (min 10)" autocomplete="new-password" minlength="10" bind:value={password} required />
    <select bind:value={role}>
      <option value="viewer">viewer</option>
      <option value="admin">admin</option>
    </select>
    <button type="submit" class="primary" disabled={busy}>Add</button>
  </form>
  {#if err}<p class="error">{err}</p>{/if}
  <p class="muted small-text">Viewers can watch the spectrum; only admins can retune and manage users.</p>
</section>

<style>
  .table-wrap {
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
  }
  th,
  td {
    text-align: left;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  th {
    color: var(--muted);
    font-weight: 500;
  }
  .actions {
    text-align: right;
  }
  h3 {
    margin: 18px 0 8px;
    font-size: 15px;
  }
  form {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  form input {
    flex: 1 1 150px;
    min-width: 0;
  }
</style>
