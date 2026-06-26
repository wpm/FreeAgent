<script lang="ts">
  /**
   * The app shell: a pan-application generic-left / app-right layout.
   *
   * The left pane (`EpisodeList`) is generic and knows nothing about any game.
   * The right pane is PLUGGABLE: the shell selects a plugin component by the
   * selected episode's `application` (or the default application for the "new"
   * flow) from the registry. The shell owns selection, the "new" trigger, the
   * theme, and the Settings modal; it delegates every app-specific concern --
   * the transcript and the create FORM -- to the plugin.
   */
  import type { EpisodeView } from "./contract";
  import EpisodeList from "./shell/EpisodeList.svelte";
  import SettingsPanel from "./shell/SettingsPanel.svelte";
  import ToastStack from "./shell/ToastStack.svelte";
  import { DEFAULT_APPLICATION, getPlugin } from "./shell/registry";
  import { settings } from "./shell/settings.svelte";

  // Register the bundled skins (side-effect import wires them into the registry).
  import "./skins/twentyquestions/index";

  let selected = $state<EpisodeView | null>(null);
  let composing = $state(false);
  let showSettings = $state(false);

  // Which application's plugin renders the right pane: the selected episode's,
  // or the default when composing a brand-new game.
  const application = $derived(
    composing || !selected ? DEFAULT_APPLICATION : selected.application,
  );
  const plugin = $derived(getPlugin(application));

  function select(episode: EpisodeView) {
    composing = false;
    selected = episode;
  }

  function startNew() {
    composing = true;
    selected = null;
  }

  // The plugin reports a freshly created episode: select it and drop out of the
  // compose state so its feed opens.
  function onCreated(episode: EpisodeView) {
    composing = false;
    selected = episode;
  }
</script>

<div class="app" data-theme={settings.theme}>
  <div class="layout">
    <div class="left">
      <EpisodeList selectedId={selected?.id ?? null} onSelect={select} onNew={startNew} />
      <button type="button" class="settings" onclick={() => (showSettings = true)}>
        Settings
      </button>
    </div>

    <main class="right">
      {#if plugin}
        {@const Plugin = plugin.component}
        <Plugin
          episode={selected}
          {composing}
          {onCreated}
          onCancelCompose={() => (composing = false)}
        />
      {:else if selected}
        <p class="placeholder">No viewer registered for “{selected.application}”.</p>
      {:else}
        <p class="placeholder">Select an episode, or start a new one.</p>
      {/if}
    </main>
  </div>

  {#if showSettings}
    <SettingsPanel onClose={() => (showSettings = false)} />
  {/if}

  <!-- Shell-level toast stack: any pane raises errors here; they persist until dismissed. -->
  <ToastStack />
</div>

<style>
  /* The shell carries the theme via data-theme; the dark palette overrides the
     same custom properties app.css defines for light. */
  .app {
    height: 100%;
  }

  .app[data-theme="dark"] {
    --bg: #0b1120;
    --surface: #111827;
    --surface-2: #1e293b;
    --border: #334155;
    --text: #e2e8f0;
    --muted: #94a3b8;
    background: var(--bg);
    color: var(--text);
  }

  .layout {
    display: grid;
    grid-template-columns: minmax(14rem, 20rem) 1fr;
    height: 100%;
    background: var(--bg);
  }

  .left {
    display: grid;
    grid-template-rows: 1fr auto;
    min-height: 0;
  }

  .settings {
    font: inherit;
    font-size: 0.8rem;
    font-weight: 600;
    padding: 0.55rem;
    border: none;
    border-top: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--muted);
    cursor: pointer;
  }

  .settings:hover {
    color: var(--text);
  }

  .right {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .placeholder {
    margin: 0;
    padding: 2rem;
    color: var(--muted);
    font-style: italic;
  }
</style>
