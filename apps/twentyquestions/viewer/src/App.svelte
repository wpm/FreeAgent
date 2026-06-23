<script lang="ts">
  import { onMount } from "svelte";

  import { resolveConfig } from "./config";
  import { EpisodeViewer } from "./viewer.svelte";
  import ConnectionStatus from "./lib/ConnectionStatus.svelte";
  import Transcript from "./lib/Transcript.svelte";

  const initial = resolveConfig();
  const viewer = new EpisodeViewer();

  let server = $state(initial.server);
  let subject = $state(initial.subject);

  const active = $derived(
    viewer.state === "connecting" ||
      viewer.state === "waiting" ||
      viewer.state === "live" ||
      viewer.state === "reconnecting",
  );

  function start() {
    const s = server.trim();
    const subj = subject.trim();
    if (!s || !subj) return;
    void viewer.connect(s, subj);
  }

  // Auto-connect when the URL already names an episode, so a shared link just
  // works; otherwise wait for the operator to fill in the subject.
  onMount(() => {
    if (initial.subject) start();
  });
</script>

<main>
  <header>
    <div class="titles">
      <h1>Twenty Questions</h1>
      <p class="subtitle">live transcript over NATS</p>
    </div>
    <ConnectionStatus state={viewer.state} detail={viewer.detail} />
  </header>

  <form
    class="controls"
    onsubmit={(event) => {
      event.preventDefault();
      start();
    }}
  >
    <label>
      <span>Server</span>
      <input bind:value={server} placeholder="ws://localhost:8080" autocomplete="off" />
    </label>
    <label class="grow">
      <span>Subject</span>
      <input
        bind:value={subject}
        placeholder="twentyquestions.episode.&lt;id&gt;.public"
        autocomplete="off"
      />
    </label>
    <button type="submit">{active ? "Reconnect" : "Connect"}</button>
    {#if active}
      <button type="button" class="secondary" onclick={() => viewer.disconnect()}>Disconnect</button>
    {/if}
  </form>

  {#if viewer.detail}
    <p class="detail" class:error={viewer.state === "error"}>{viewer.detail}</p>
  {/if}

  <Transcript messages={viewer.messages} />
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    max-width: 720px;
    margin: 0 auto;
    background: var(--surface);
    box-shadow: 0 0 2.5rem rgba(15, 23, 42, 0.08);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1rem;
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

  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.5rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
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

  button.secondary {
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
</style>
