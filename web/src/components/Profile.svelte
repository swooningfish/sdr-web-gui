<script lang="ts">
  import type { User } from '../lib/api';
  import Account from './Account.svelte';

  let { user, onlogout }: { user: User; onlogout: () => void } = $props();
</script>

<div class="profile">
  <section class="panel">
    <h2>{user.username}</h2>
    <dl class="small-text">
      <dt>Role</dt>
      <dd>{user.role === 'admin' ? 'Admin: can retune the radio and change settings' : 'Viewer: can watch and listen'}</dd>
    </dl>
    <button type="button" class="small" onclick={onlogout}>Sign out</button>
  </section>
  <Account {user} />
</div>

<style>
  .profile {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 340px), 1fr));
    gap: 16px;
    align-items: start;
    max-width: 900px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 14px;
    margin: 0 0 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
</style>
