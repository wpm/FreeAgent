<script lang="ts">
  import { onMount } from "svelte";

  import { resolveConfig } from "./config";
  import { EpisodeController, type LaunchSettings } from "./controller.svelte";
  import DiscoveryList from "./lib/DiscoveryList.svelte";
  import EpisodePanel from "./lib/EpisodePanel.svelte";

  const controller = new EpisodeController();
  const initial = resolveConfig();

  // The launch surface. The roster (player count) is source-defined for v1, so
  // these are the only knobs: the Host's secret, a model override for every LLM
  // agent, the logging file, and an optional fixed episode id.
  let settings = $state<LaunchSettings>({ secret: "", model: "", parquetLog: "", episodeId: "" });

  // The read-only observe bar: attach a transcript to a subject by hand, exactly
  // as the original single-episode viewer did (no control plane involved).
  let subject = $state(initial.subject);
  let replayPath = $state("");

  function observe() {
    const subj = subject.trim();
    if (!subj) return;
    controller.observe(controller.config.natsWs, subj);
  }

  function startLive() {
    void controller.startLive(settings);
  }

  function startReplay() {
    const path = replayPath.trim();
    if (!path) return;
    void controller.startReplay(path);
  }

  // A shared link that already names an episode just works: attach to it on load
  // through the same read-only path. Either way, start discovering every episode
  // on the bus so any of them is watchable with one click — no hand-typed subject.
  onMount(() => {
    if (initial.subject) observe();
    void controller.startDiscovery();
    return () => controller.stopDiscovery();
  });
</script>

<main>
  <header class="top">
    <div class="titles">
      <h1>Twenty Questions</h1>
      <p class="subtitle">control plane — configure, launch, and watch episodes</p>
    </div>
    <button
      type="button"
      class="ghost"
      disabled={controller.busy}
      onclick={() => controller.refresh()}>Refresh</button
    >
  </header>

  <section class="config">
    <label>
      <span>Control service</span>
      <input
        bind:value={controller.config.control}
        placeholder="http://localhost:8000"
        autocomplete="off"
      />
    </label>
    <label>
      <span>NATS websocket</span>
      <input
        bind:value={controller.config.natsWs}
        placeholder="ws://localhost:8080"
        autocomplete="off"
      />
    </label>
    <label>
      <span>Application</span>
      <input
        bind:value={controller.config.application}
        placeholder="twenty-questions"
        autocomplete="off"
      />
    </label>
  </section>

  <section class="launch">
    <form
      class="row"
      onsubmit={(event) => {
        event.preventDefault();
        startLive();
      }}
    >
      <label class="grow">
        <span>Host secret</span>
        <input bind:value={settings.secret} placeholder="(random canned secret)" autocomplete="off" />
      </label>
      <label>
        <span>Model</span>
        <input bind:value={settings.model} placeholder="(app default)" autocomplete="off" />
      </label>
      <label>
        <span>Logging file</span>
        <input bind:value={settings.parquetLog} placeholder="out/episode.parquet" autocomplete="off" />
      </label>
      <label>
        <span>Episode id</span>
        <input bind:value={settings.episodeId} placeholder="(auto)" autocomplete="off" />
      </label>
      <button type="submit" disabled={controller.busy}>Start episode</button>
    </form>

    <form
      class="row"
      onsubmit={(event) => {
        event.preventDefault();
        startReplay();
      }}
    >
      <label class="grow">
        <span>Replay log</span>
        <input bind:value={replayPath} placeholder="out/episode.parquet" autocomplete="off" />
      </label>
      <button type="submit" disabled={controller.busy}>Replay</button>
    </form>

    <form
      class="row"
      onsubmit={(event) => {
        event.preventDefault();
        observe();
      }}
    >
      <label class="grow">
        <span>Observe subject (fallback — discovery above is the primary path)</span>
        <input
          bind:value={subject}
          placeholder="twentyquestions.episode.&lt;id&gt;.public"
          autocomplete="off"
        />
      </label>
      <button type="submit" class="secondary">Connect</button>
    </form>

    {#if controller.error}
      <p class="detail error">{controller.error}</p>
    {/if}
  </section>

  <DiscoveryList
    episodes={controller.discovered}
    busy={controller.busy}
    error={controller.discoveryError}
    onwatch={(entry) => controller.watchDiscovered(entry)}
    onstop={(entry) => controller.stopDiscovered(entry)}
  />

  {#if controller.episodes.length === 0}
    <p class="empty">
      No episodes yet. Start one above, replay a recorded log, or attach to a subject to watch.
    </p>
  {:else}
    <div class="grid">
      {#each controller.episodes as episode (episode.key)}
        <EpisodePanel
          {episode}
          busy={controller.busy}
          onstop={(e) => controller.stop(e)}
          onclose={(e) => controller.close(e)}
        />
      {/each}
    </div>
  {/if}
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    margin: 0 auto;
    background: var(--bg);
  }

  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1.25rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  .titles h1 {
    margin: 0;
    font-size: 1.15rem;
  }

  .subtitle {
    margin: 0;
    font-size: 0.78rem;
    color: var(--muted);
  }

  .config,
  .launch {
    padding: 0.7rem 1.25rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .config {
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    background: var(--surface-2);
  }

  .launch {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.5rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.72rem;
    font-weight: 600;
    color: var(--muted);
  }

  label.grow {
    flex: 1;
    min-width: 12rem;
  }

  input {
    font: inherit;
    font-size: 0.85rem;
    padding: 0.4rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
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
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.secondary,
  button.ghost {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }

  .detail {
    margin: 0.2rem 0 0;
    font-size: 0.78rem;
    color: var(--muted);
  }

  .detail.error {
    color: #dc2626;
  }

  .empty {
    margin: auto;
    color: var(--muted);
    font-style: italic;
    text-align: center;
    padding: 2rem;
  }

  .grid {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(20rem, 1fr));
    gap: 0.9rem;
    padding: 0.9rem 1.25rem;
    align-content: start;
  }

  /* Each panel gets a bounded height so its transcript scrolls independently. */
  .grid :global(.panel) {
    height: min(32rem, 70vh);
  }
</style>
