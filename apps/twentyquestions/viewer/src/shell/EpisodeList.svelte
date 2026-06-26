<script lang="ts">
  /**
   * The generic LEFT pane: the episode list.
   *
   * Pan-application and presentation-agnostic. It lists episodes by their
   * friendly `name` with a live/sealed status indicator, supports select, a
   * "New" action (the shell delegates the actual create FORM to the right-pane
   * plugin), inline rename (PATCH), and a right-click context menu with a
   * confirm-guarded Delete (DELETE). It polls the list every few seconds and
   * refetches after mutating actions so it stays fresh.
   */
  import { onMount } from "svelte";

  import type { EpisodeView } from "../contract";
  import { settings } from "./settings.svelte";
  import { deleteEpisode, listEpisodes, renameEpisode } from "./service";
  import { toasts } from "./toasts.svelte";

  let {
    selectedId,
    onSelect,
    onNew,
  }: {
    selectedId: string | null;
    onSelect: (episode: EpisodeView) => void;
    onNew: () => void;
  } = $props();

  let episodes = $state<EpisodeView[]>([]);
  // The polling/refresh error: a single inline banner. It must not spam a toast
  // every poll, so the continuous "can't reach the service" state stays inline;
  // discrete user actions (rename, delete) raise dismissible toasts instead.
  let error = $state<string | null>(null);

  // Inline-rename state: the id being edited and its working title.
  let editingId = $state<string | null>(null);
  let editName = $state("");

  // Context menu + delete-confirm state.
  let menu = $state<{ episode: EpisodeView; x: number; y: number } | null>(null);
  let confirmDelete = $state<EpisodeView | null>(null);

  const POLL_MS = 3000;

  /** Refetch the list, surfacing the service base from settings reactively. */
  async function refresh() {
    try {
      episodes = await listEpisodes(settings.serviceUrl);
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  });

  function startRename(episode: EpisodeView) {
    editingId = episode.id;
    editName = episode.name;
    menu = null;
  }

  async function commitRename(episode: EpisodeView) {
    const name = editName.trim();
    editingId = null;
    if (!name || name === episode.name) return;
    try {
      await renameEpisode(settings.serviceUrl, episode.application, episode.episode_id, { name });
      await refresh();
    } catch (e) {
      toasts.add(e instanceof Error ? e.message : String(e));
    }
  }

  function openMenu(event: MouseEvent, episode: EpisodeView) {
    event.preventDefault();
    menu = { episode, x: event.clientX, y: event.clientY };
  }

  async function doDelete(episode: EpisodeView) {
    confirmDelete = null;
    try {
      await deleteEpisode(settings.serviceUrl, episode.application, episode.episode_id);
      await refresh();
    } catch (e) {
      toasts.add(e instanceof Error ? e.message : String(e));
    }
  }

  // A sealed episode is finished (replayable); an unsealed one is open/live.
  function liveLabel(episode: EpisodeView): string {
    return episode.sealed ? "sealed" : "live";
  }
</script>

<!-- Any click closes the context menu; Escape also dismisses the delete confirm. -->
<svelte:window
  onclick={() => (menu = null)}
  onkeydown={(e) => {
    if (e.key === "Escape") {
      menu = null;
      confirmDelete = null;
    }
  }}
/>

<aside class="list">
  <header>
    <h2>Episodes</h2>
    <button type="button" class="new" onclick={onNew}>New</button>
  </header>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <ul>
    {#each episodes as episode (episode.id)}
      <li>
        <button
          type="button"
          class="row"
          class:selected={episode.id === selectedId}
          onclick={() => onSelect(episode)}
          oncontextmenu={(e) => openMenu(e, episode)}
        >
          <span class="dot" class:sealed={episode.sealed}></span>
          <span class="meta">
            {#if editingId === episode.id}
              <!-- Stop the click from re-selecting while editing. -->
              <input
                class="rename"
                bind:value={editName}
                onclick={(e) => e.stopPropagation()}
                onkeydown={(e) => {
                  if (e.key === "Enter") commitRename(episode);
                  else if (e.key === "Escape") editingId = null;
                }}
                onblur={() => commitRename(episode)}
                autocomplete="off"
              />
            {:else}
              <span class="name">{episode.name}</span>
            {/if}
            <span class="status">{liveLabel(episode)} · {episode.status}</span>
          </span>
        </button>
      </li>
    {/each}

    {#if episodes.length === 0 && !error}
      <li class="empty">No episodes yet.</li>
    {/if}
  </ul>
</aside>

{#if menu}
  <div class="menu" style="left: {menu.x}px; top: {menu.y}px" role="menu">
    <button type="button" role="menuitem" onclick={() => menu && startRename(menu.episode)}>
      Rename
    </button>
    <button
      type="button"
      role="menuitem"
      class="danger"
      onclick={() => {
        if (menu) confirmDelete = menu.episode;
        menu = null;
      }}
    >
      Delete
    </button>
  </div>
{/if}

{#if confirmDelete}
  <!-- svelte-ignore a11y_click_events_have_key_events -- backdrop is a dismiss affordance; Escape is handled on window -->
  <div class="backdrop" role="presentation" onclick={() => (confirmDelete = null)}>
    <div
      class="confirm"
      role="dialog"
      aria-modal="true"
      aria-label="Confirm delete"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
    >
      <p>Delete <strong>{confirmDelete.name}</strong>? This cannot be undone.</p>
      <div class="actions">
        <button type="button" class="secondary" onclick={() => (confirmDelete = null)}>
          Cancel
        </button>
        <button
          type="button"
          class="danger"
          onclick={() => confirmDelete && doDelete(confirmDelete)}
        >
          Delete
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--border);
    background: var(--surface-2);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 0.85rem;
    border-bottom: 1px solid var(--border);
  }

  header h2 {
    margin: 0;
    font-size: 0.95rem;
  }

  .new {
    font: inherit;
    font-size: 0.78rem;
    font-weight: 600;
    padding: 0.3rem 0.7rem;
    border: 1px solid transparent;
    border-radius: 0.5rem;
    background: #2563eb;
    color: white;
    cursor: pointer;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0.4rem;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    width: 100%;
    text-align: left;
    font: inherit;
    padding: 0.5rem 0.55rem;
    border: 1px solid transparent;
    border-radius: 0.55rem;
    background: none;
    color: var(--text);
    cursor: pointer;
  }

  .row:hover {
    background: var(--surface);
  }

  .row.selected {
    background: var(--surface);
    border-color: var(--border);
  }

  .dot {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    flex: none;
    background: #22c55e;
  }

  .dot.sealed {
    background: var(--muted);
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    min-width: 0;
  }

  .name {
    font-weight: 600;
    font-size: 0.85rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    font-size: 0.7rem;
    color: var(--muted);
  }

  .rename {
    font: inherit;
    font-size: 0.85rem;
    padding: 0.15rem 0.35rem;
    border: 1px solid var(--border);
    border-radius: 0.35rem;
    background: var(--surface);
    color: var(--text);
  }

  .empty {
    color: var(--muted);
    font-style: italic;
    font-size: 0.8rem;
    padding: 0.6rem;
  }

  .error {
    margin: 0;
    padding: 0.4rem 0.85rem;
    font-size: 0.75rem;
    color: #dc2626;
    /* The banner now shows the full server message (often a long URL); wrap and
       break it rather than overflowing the narrow left pane. */
    word-break: break-word;
  }

  .menu {
    position: fixed;
    z-index: 60;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    box-shadow: 0 0.5rem 1.5rem rgba(15, 23, 42, 0.2);
    overflow: hidden;
    min-width: 8rem;
  }

  .menu button {
    font: inherit;
    font-size: 0.82rem;
    text-align: left;
    padding: 0.45rem 0.75rem;
    border: none;
    background: none;
    color: var(--text);
    cursor: pointer;
  }

  .menu button:hover {
    background: var(--surface-2);
  }

  .menu button.danger {
    color: #dc2626;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.45);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
  }

  .confirm {
    width: min(22rem, 92vw);
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 0.75rem;
    padding: 1rem;
    box-shadow: 0 1rem 3rem rgba(15, 23, 42, 0.3);
  }

  .confirm p {
    margin: 0 0 1rem;
    font-size: 0.9rem;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .actions button {
    font: inherit;
    font-weight: 600;
    font-size: 0.82rem;
    padding: 0.4rem 0.85rem;
    border: 1px solid transparent;
    border-radius: 0.5rem;
    cursor: pointer;
    background: #2563eb;
    color: white;
  }

  .actions button.secondary {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }

  .actions button.danger {
    background: #dc2626;
  }
</style>
