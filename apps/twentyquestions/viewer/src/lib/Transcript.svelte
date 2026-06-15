<script lang="ts">
  import type { ChatMessage } from "../viewer.svelte";
  import MessageBubble from "./MessageBubble.svelte";

  let { messages }: { messages: ChatMessage[] } = $props();

  let container: HTMLDivElement | undefined = $state();

  // Keep the newest message in view as the room fills up.
  $effect(() => {
    void messages.length;
    if (container) container.scrollTop = container.scrollHeight;
  });
</script>

<div class="transcript" bind:this={container}>
  {#if messages.length === 0}
    <p class="empty">No messages yet. The transcript will appear here as the room speaks.</p>
  {:else}
    {#each messages as message (message.key)}
      <MessageBubble {message} />
    {/each}
  {/if}
</div>

<style>
  .transcript {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 1rem;
  }

  .empty {
    margin: auto;
    color: var(--muted);
    font-style: italic;
    text-align: center;
  }
</style>
