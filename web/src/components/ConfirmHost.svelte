<script lang="ts">
  import { confirms } from '../lib/confirm.svelte';

  let dialog = $state<HTMLDialogElement | undefined>();
  let okBtn = $state<HTMLButtonElement | undefined>();
  let cancelBtn = $state<HTMLButtonElement | undefined>();
  const req = $derived(confirms.current);

  // A modal <dialog> sits in the top layer, so it also covers the bookmark
  // drawer and the SSTV lightbox (itself a modal dialog).
  $effect(() => {
    if (!dialog) return;
    if (req) {
      if (!dialog.open) dialog.showModal();
      // Enter shouldn't delete anything by accident.
      (req.danger ? cancelBtn : okBtn)?.focus();
    } else if (dialog.open) {
      dialog.close();
    }
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<dialog
  bind:this={dialog}
  aria-labelledby="confirm-msg"
  oncancel={(e) => {
    // Esc: answer "no" ourselves so the queue stays in step.
    e.preventDefault();
    confirms.answer(false);
  }}
  onkeydown={(e) => {
    // Keep Esc from also reaching page-level shortcuts underneath.
    if (e.key === 'Escape') e.stopPropagation();
  }}
  onclick={(e) => {
    if (e.target === dialog) confirms.answer(false);
  }}
>
  {#if req}
    <div class="box">
      <p id="confirm-msg">{req.message}</p>
      <div class="actions">
        <button type="button" bind:this={cancelBtn} onclick={() => confirms.answer(false)}>
          {req.cancel ?? 'Cancel'}
        </button>
        <button
          type="button"
          class={req.danger ? 'go-danger' : 'primary'}
          bind:this={okBtn}
          onclick={() => confirms.answer(true)}
        >
          {req.ok ?? 'OK'}
        </button>
      </div>
    </div>
  {/if}
</dialog>

<style>
  dialog {
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    color: var(--text);
    width: min(420px, calc(100vw - 32px));
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.45);
  }
  dialog::backdrop {
    background: rgba(0, 0, 0, 0.5);
  }
  dialog[open] {
    animation: pop 0.12s ease-out;
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-6px) scale(0.98);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    dialog[open] {
      animation: none;
    }
  }
  .box {
    padding: 18px 18px 14px;
  }
  p {
    margin: 0 0 16px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .actions button {
    min-width: 84px;
  }
  .go-danger {
    background: var(--bad);
    border-color: var(--bad);
    color: #fff;
    font-weight: 600;
  }
</style>
