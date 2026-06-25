<script lang="ts">
  /**
   * The Twenty Questions skin: the right-pane plugin for application
   * "twenty-questions".
   *
   * PURE PRESENTATION over the normalized feed. Given the selected episode, it
   * opens that episode's feed (via the shared `FeedViewModel`, which uses the
   * shell's `openFeed`) and renders the salvaged chat transcript, a row of
   * status/outcome/budget chips, and -- whether live or sealed (replay) -- the
   * same view. When the shell signals "compose a new game", it shows a
   * start-game form instead and reports the created episode back to the shell.
   */
  import type { PluginProps } from "../../shell/registry";
  import { createEpisode } from "../../shell/service";
  import { settings } from "../../shell/settings.svelte";
  import Transcript from "../../lib/Transcript.svelte";
  import ConnectionStatus from "../../lib/ConnectionStatus.svelte";
  import { FeedViewModel } from "./feed.svelte";

  const APPLICATION = "twenty-questions";

  let { episode, composing, onCreated, onCancelCompose }: PluginProps = $props();

  const feed = new FeedViewModel();

  // (Re)open the feed whenever the selected episode changes; detach on teardown
  // or while composing a new game (no episode to watch yet).
  $effect(() => {
    const ep = episode;
    if (!ep || composing) {
      feed.close();
      return;
    }
    feed.open(settings.serviceUrl, ep.application, ep.episode_id);
    return () => feed.close();
  });

  // Best-effort budget chip: count Player questions (public, non-host speech),
  // or fall back to the Host's announced count from the game-over signal.
  const questionsAsked = $derived(
    feed.questionsAsked ??
      feed.messages.filter(
        (m) => !m.outcome && m.sender.toLowerCase() !== "host" && m.sender.toLowerCase() !== "env",
      ).length,
  );

  // --- Start-game composer state ---
  let secret = $state("");
  let model = $state(settings.model);
  let name = $state("");
  let creating = $state(false);
  let createError = $state<string | null>(null);

  // Keep the model field in step with settings until the operator edits it.
  $effect(() => {
    model = settings.model;
  });

  async function startGame() {
    if (!secret.trim() || creating) return;
    creating = true;
    createError = null;
    try {
      const created = await createEpisode(settings.serviceUrl, APPLICATION, {
        name: name.trim() || undefined,
        model: model.trim() || undefined,
        api_key: settings.apiKey || undefined,
        agents: { host: { config: { secret: secret.trim() } } },
      });
      secret = "";
      name = "";
      onCreated(created);
    } catch (e) {
      createError = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }
</script>

<section class="skin">
  {#if composing || !episode}
    <!-- Start-game composer: the app-specific create form the shell delegates. -->
    <div class="composer">
      <h2>New Twenty Questions game</h2>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          startGame();
        }}
      >
        <label>
          <span>Secret (what the host is thinking of)</span>
          <input bind:value={secret} placeholder="elephant" autocomplete="off" required />
        </label>
        <label>
          <span>Name (optional)</span>
          <input bind:value={name} placeholder="auto-generated if blank" autocomplete="off" />
        </label>
        <label>
          <span>Model</span>
          <input bind:value={model} placeholder="claude-haiku-4-5-20251001" autocomplete="off" />
        </label>
        {#if createError}
          <p class="error">{createError}</p>
        {/if}
        <div class="actions">
          {#if composing}
            <button type="button" class="secondary" onclick={onCancelCompose}>Cancel</button>
          {/if}
          <button type="submit" disabled={creating || !secret.trim()}>
            {creating ? "Starting…" : "Start game"}
          </button>
        </div>
      </form>
    </div>
  {:else}
    <header>
      <div class="titles">
        <h2>{episode.name}</h2>
        <p class="subtitle">Twenty Questions</p>
      </div>
      <ConnectionStatus state={feed.phase} detail={feed.detail ?? ""} />
    </header>

    <div class="chips">
      <span class="chip">{feed.status || episode.status}</span>
      <span class="chip" class:sealed={feed.sealed}>{feed.sealed ? "sealed" : "live"}</span>
      {#if feed.outcome}
        <span class="chip outcome">{feed.outcome}</span>
      {/if}
      <span class="chip budget">{questionsAsked} / 20 questions</span>
    </div>

    {#if feed.detail}
      <p class="detail" class:error={feed.phase === "error"}>{feed.detail}</p>
    {/if}

    <Transcript messages={feed.messages} />
  {/if}
</section>

<style>
  .skin {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--surface);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  .titles h2 {
    margin: 0;
    font-size: 1.1rem;
  }

  .subtitle {
    margin: 0;
    font-size: 0.78rem;
    color: var(--muted);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
  }

  .chip {
    font-size: 0.72rem;
    font-weight: 600;
    padding: 0.2rem 0.55rem;
    border-radius: 999px;
    background: hsl(140 60% 92%);
    color: hsl(140 45% 30%);
    border: 1px solid hsl(140 50% 82%);
  }

  .chip.sealed {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }

  .chip.outcome {
    background: #ecfdf5;
    color: #047857;
    border-color: #6ee7b7;
    text-transform: capitalize;
  }

  .chip.budget {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }

  .detail {
    margin: 0;
    padding: 0.35rem 1rem;
    font-size: 0.75rem;
    color: var(--muted);
    background: var(--surface-2);
    border-bottom: 1px solid var(--border);
  }

  .detail.error {
    color: #dc2626;
  }

  .composer {
    padding: 1.25rem;
    max-width: 32rem;
  }

  .composer h2 {
    margin: 0 0 1rem;
    font-size: 1.1rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted);
  }

  input {
    font: inherit;
    font-size: 0.85rem;
    padding: 0.4rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    color: var(--text);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  button {
    font: inherit;
    font-weight: 600;
    font-size: 0.85rem;
    padding: 0.45rem 0.9rem;
    border: 1px solid transparent;
    border-radius: 0.5rem;
    background: #2563eb;
    color: white;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  button.secondary {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }

  .error {
    margin: 0;
    font-size: 0.78rem;
    color: #dc2626;
  }
</style>
