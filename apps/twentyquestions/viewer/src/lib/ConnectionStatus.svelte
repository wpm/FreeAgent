<script lang="ts">
  import type { FeedPhase } from "../skins/twentyquestions/feed.svelte";

  let { state, detail }: { state: FeedPhase; detail: string } = $props();

  const LABELS: Record<FeedPhase, string> = {
    connecting: "Connecting",
    open: "Loading history",
    live: "Live",
    closed: "Replay (sealed)",
    error: "Error",
  };
</script>

<span class="status status--{state}" title={detail}>
  <span class="dot"></span>
  <span class="label">{LABELS[state]}</span>
</span>

<style>
  .status {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--muted);
  }

  .dot {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    background: var(--muted);
    flex: none;
  }

  .status--live .dot {
    background: #22c55e;
    box-shadow: 0 0 0 0 rgba(34, 197, 94, 0.6);
    animation: pulse 1.8s ease-out infinite;
  }
  .status--live .label {
    color: #16a34a;
  }

  .status--connecting .dot,
  .status--open .dot {
    background: #f59e0b;
    animation: blink 1s step-start infinite;
  }

  .status--closed .dot {
    background: var(--muted);
  }

  .status--error .dot {
    background: #ef4444;
  }
  .status--error .label {
    color: #dc2626;
  }

  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 rgba(34, 197, 94, 0.5);
    }
    100% {
      box-shadow: 0 0 0 0.5rem rgba(34, 197, 94, 0);
    }
  }

  @keyframes blink {
    50% {
      opacity: 0.25;
    }
  }
</style>
