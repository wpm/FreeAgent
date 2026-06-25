<script lang="ts">
  import type { ChatMessage } from "../skins/twentyquestions/feed.svelte";

  let { message }: { message: ChatMessage } = $props();

  const isHost = $derived(message.sender.toLowerCase() === "host");

  // A stable per-sender hue so each Player keeps one colour across the room.
  function hue(name: string): number {
    let h = 0;
    for (let i = 0; i < name.length; i++) {
      h = (h * 31 + name.charCodeAt(i)) % 360;
    }
    return h;
  }
</script>

<div
  class="bubble"
  class:host={isHost}
  class:outcome={message.outcome}
  style="--hue: {hue(message.sender)}"
>
  <span class="sender">{message.sender}</span>
  <p class="text">{message.text}</p>
</div>

<style>
  .bubble {
    max-width: 80%;
    align-self: flex-start;
    background: hsl(var(--hue) 70% 96%);
    border: 1px solid hsl(var(--hue) 60% 88%);
    border-radius: 0.9rem;
    border-top-left-radius: 0.2rem;
    padding: 0.5rem 0.75rem;
  }

  .bubble.host {
    align-self: center;
    max-width: 90%;
    text-align: center;
    background: #fffbeb;
    border-color: #fde68a;
    border-radius: 0.9rem;
  }

  .bubble.outcome {
    background: #ecfdf5;
    border-color: #6ee7b7;
    font-weight: 600;
  }

  .sender {
    display: block;
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: lowercase;
    letter-spacing: 0.02em;
    color: hsl(var(--hue) 45% 40%);
    margin-bottom: 0.15rem;
  }

  .host .sender {
    color: #b45309;
  }

  .text {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.4;
  }
</style>
