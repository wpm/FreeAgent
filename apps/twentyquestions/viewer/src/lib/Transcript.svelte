<script lang="ts">
  import type { ChatMessage } from "../skins/twentyquestions/feed.svelte";
  import MessageBubble from "./MessageBubble.svelte";

  let { messages }: { messages: ChatMessage[] } = $props();

  let container: HTMLDivElement | undefined = $state();

  // Pixels from the bottom within which the user counts as "following along".
  const STICK_THRESHOLD = 64;
  // Whether the user was pinned to the bottom *before* this batch rendered.
  let following = true;

  // Measure before the DOM updates: only keep auto-scrolling when the user is
  // already at the bottom, so a new message never yanks them away from earlier
  // exchanges they scrolled up to read (noticeable on a fast replay).
  $effect.pre(() => {
    void messages.length;
    if (!container) return;
    following =
      container.scrollHeight - container.scrollTop - container.clientHeight <= STICK_THRESHOLD;
  });

  // Keep the newest message in view, but only if they were following.
  $effect(() => {
    void messages.length;
    if (container && following) container.scrollTop = container.scrollHeight;
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
