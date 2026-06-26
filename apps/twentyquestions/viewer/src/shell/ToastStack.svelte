<script lang="ts">
  /**
   * The shell-level toast stack: renders the shared `toasts` store as a fixed
   * column of dismissible notifications.
   *
   * Pan-application and presentation-agnostic. Each toast shows its full message
   * (no truncation) and a close button; there is no auto-dismiss timeout, so an
   * error stays until the operator acknowledges it. Mount this once at the app
   * root; any skin raises a toast through the store.
   */
  import { toasts } from "./toasts.svelte";
</script>

{#if toasts.toasts.length > 0}
  <div class="toasts" role="region" aria-label="Notifications">
    {#each toasts.toasts as toast (toast.id)}
      <div class="toast" class:error={toast.level === "error"} role="alert">
        <span class="message">{toast.message}</span>
        <button
          type="button"
          class="close"
          aria-label="Dismiss notification"
          onclick={() => toasts.dismiss(toast.id)}
        >
          ×
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .toasts {
    position: fixed;
    bottom: 1rem;
    right: 1rem;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: min(28rem, 92vw);
  }

  .toast {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.6rem 0.75rem;
    border-radius: 0.6rem;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    box-shadow: 0 0.5rem 1.5rem rgba(15, 23, 42, 0.25);
  }

  .toast.error {
    border-color: #fca5a5;
    background: #fef2f2;
    color: #991b1b;
  }

  .message {
    flex: 1;
    font-size: 0.8rem;
    line-height: 1.4;
    /* Show the full message, wrapping and breaking long URLs rather than truncating. */
    word-break: break-word;
  }

  .close {
    flex: none;
    font: inherit;
    font-size: 1.1rem;
    line-height: 1;
    padding: 0 0.15rem;
    border: none;
    background: none;
    color: inherit;
    cursor: pointer;
    opacity: 0.7;
  }

  .close:hover {
    opacity: 1;
  }
</style>
