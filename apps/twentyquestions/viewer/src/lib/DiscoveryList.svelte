<script lang="ts">
  import type { DiscoveredEntry } from "../controller.svelte";

  let {
    episodes,
    busy,
    error,
    onwatch,
    onstop,
  }: {
    episodes: DiscoveredEntry[];
    busy: boolean;
    error: string;
    onwatch: (entry: DiscoveredEntry) => void;
    onstop: (entry: DiscoveredEntry) => void;
  } = $props();

  /** A compact "3m ago" / "just now" from an ISO timestamp; "—" if unparseable. */
  function ago(iso: string): string {
    const then = Date.parse(iso);
    if (Number.isNaN(then)) return "—";
    const secs = Math.max(0, Math.round((Date.now() - then) / 1000));
    if (secs < 5) return "just now";
    if (secs < 60) return `${secs}s ago`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
    if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
    return `${Math.floor(secs / 86400)}d ago`;
  }
</script>

<section class="discovery">
  <header class="head">
    <h2>Episodes on the bus</h2>
    <span class="hint">discovered over JetStream — live or replayed, from any source</span>
  </header>

  {#if error}
    <p class="detail error">{error}</p>
  {/if}

  {#if episodes.length === 0}
    <p class="empty">No episodes yet — start one above, or run one from the CLI.</p>
  {:else}
    <ul class="list">
      {#each episodes as entry (entry.subjectRoot)}
        <li class="item">
          <div class="ident">
            <span class="id" title={entry.subjectRoot}>{entry.episodeId}</span>
            <span class="facts">
              {#if entry.view}
                <span class="tag tag--{entry.view.mode}">{entry.view.mode}</span>
                <span class="tag tag--status" title={entry.view.detail ?? undefined}
                  >{entry.view.status}</span
                >
              {:else}
                <span class="tag">present</span>
              {/if}
              <span class="meta">{entry.messageCount} msg · active {ago(entry.lastActiveAt)}</span>
            </span>
          </div>
          <div class="actions">
            {#if entry.view?.controllable}
              <button type="button" disabled={busy} onclick={() => onstop(entry)}>Stop</button>
            {/if}
            <button type="button" class="primary" onclick={() => onwatch(entry)}>Watch</button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .discovery {
    padding: 0.7rem 1.25rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    margin-bottom: 0.5rem;
  }

  h2 {
    margin: 0;
    font-size: 0.95rem;
  }

  .hint {
    font-size: 0.72rem;
    color: var(--muted);
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface-2);
  }

  .ident {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .id {
    font-weight: 600;
    font-size: 0.85rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .facts {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem;
  }

  .meta {
    font-size: 0.72rem;
    color: var(--muted);
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

  .actions {
    display: flex;
    gap: 0.35rem;
    flex: none;
  }

  button {
    font: inherit;
    font-weight: 600;
    font-size: 0.78rem;
    padding: 0.3rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 0.45rem;
    background: var(--surface);
    color: var(--muted);
    cursor: pointer;
  }

  button.primary {
    background: #2563eb;
    color: white;
    border-color: transparent;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .detail {
    margin: 0 0 0.4rem;
    font-size: 0.78rem;
    color: var(--muted);
  }

  .detail.error {
    color: #dc2626;
  }

  .empty {
    margin: 0;
    color: var(--muted);
    font-style: italic;
    font-size: 0.82rem;
  }
</style>
