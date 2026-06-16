<script lang="ts">
  import type { WatchedEpisode } from "../controller.svelte";
  import ConnectionStatus from "./ConnectionStatus.svelte";
  import Transcript from "./Transcript.svelte";

  let {
    episode,
    busy,
    onstop,
    onclose,
  }: {
    episode: WatchedEpisode;
    busy: boolean;
    onstop: (episode: WatchedEpisode) => void;
    onclose: (episode: WatchedEpisode) => void;
  } = $props();

  const viewer = $derived(episode.viewer);
  // A control-launched episode can be stopped over REST while it is running;
  // an observed one (or a finished one) has nothing to stop.
  const stoppable = $derived(
    episode.source === "control" && episode.view?.status === "running",
  );
</script>

<section class="panel">
  <header>
    <div class="meta">
      <h2 title={viewer.subject}>{episode.title}</h2>
      {#if episode.view}
        <span class="tags">
          <span class="tag tag--{episode.view.mode}">{episode.view.mode}</span>
          <span class="tag tag--status" title={episode.view.detail ?? undefined}
            >{episode.view.status}</span
          >
        </span>
      {:else}
        <span class="tags"><span class="tag">observed</span></span>
      {/if}
    </div>
    <div class="right">
      <ConnectionStatus state={viewer.state} detail={viewer.detail} />
      <div class="actions">
        {#if stoppable}
          <button type="button" disabled={busy} onclick={() => onstop(episode)}>Stop</button>
        {/if}
        <button
          type="button"
          class="ghost"
          title="Detach this transcript (does not stop the episode)"
          onclick={() => onclose(episode)}>Close</button
        >
      </div>
    </div>
  </header>
  <Transcript messages={viewer.messages} />
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0.6rem;
    overflow: hidden;
    box-shadow: 0 1px 2px rgba(15, 23, 42, 0.04);
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 0.75rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
  }

  .meta {
    min-width: 0;
  }

  h2 {
    margin: 0;
    font-size: 0.95rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tags {
    display: inline-flex;
    gap: 0.3rem;
    margin-top: 0.2rem;
  }

  .tag {
    font-size: 0.68rem;
    font-weight: 600;
    padding: 0.05rem 0.4rem;
    border-radius: 0.35rem;
    background: var(--bg);
    color: var(--muted);
    border: 1px solid var(--border);
    text-transform: lowercase;
  }

  .tag--replay {
    background: #eef2ff;
    color: #4338ca;
    border-color: #c7d2fe;
  }

  .tag--live {
    background: #ecfdf5;
    color: #047857;
    border-color: #a7f3d0;
  }

  .right {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.4rem;
    flex: none;
  }

  .actions {
    display: flex;
    gap: 0.35rem;
  }

  button {
    font: inherit;
    font-weight: 600;
    font-size: 0.78rem;
    padding: 0.3rem 0.7rem;
    border: 1px solid transparent;
    border-radius: 0.45rem;
    background: #2563eb;
    color: white;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.ghost {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }
</style>
