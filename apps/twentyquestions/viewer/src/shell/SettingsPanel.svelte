<script lang="ts">
  /**
   * The Settings modal: connection, model, API key, and theme.
   *
   * A thin form over the shared `settings` store. Every field persists to
   * localStorage through the store's setters. The API key field is masked and
   * carries a reminder that it leaves the browser only inside a create body.
   */
  import { settings, type Theme } from "./settings.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // Edit local copies, then commit on save -- so a half-typed URL never thrashes
  // the live connection mid-keystroke.
  let serviceUrl = $state(settings.serviceUrl);
  let model = $state(settings.model);
  let apiKey = $state(settings.apiKey);
  let theme = $state<Theme>(settings.theme);

  function save() {
    settings.setServiceUrl(serviceUrl);
    settings.setModel(model);
    settings.setApiKey(apiKey);
    settings.setTheme(theme);
    onClose();
  }
</script>

<!-- A click on the backdrop or Escape dismisses; clicks inside do not bubble. -->
<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<!-- svelte-ignore a11y_click_events_have_key_events -- backdrop is a dismiss affordance; Escape is handled on window -->
<div class="backdrop" role="presentation" onclick={onClose}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="Settings"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header>
      <h2>Settings</h2>
      <button type="button" class="icon" aria-label="Close" onclick={onClose}>×</button>
    </header>

    <form
      onsubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <label>
        <span>Service URL</span>
        <input bind:value={serviceUrl} placeholder="http://localhost:8000" autocomplete="off" />
      </label>

      <label>
        <span>Model</span>
        <input bind:value={model} placeholder="claude-haiku-4-5-20251001" autocomplete="off" />
      </label>

      <label>
        <span>API key</span>
        <input
          type="password"
          bind:value={apiKey}
          placeholder="sk-..."
          autocomplete="off"
        />
        <small>Stored only in this browser; sent only inside a create-episode request.</small>
      </label>

      <fieldset>
        <legend>Theme</legend>
        <label class="radio">
          <input type="radio" name="theme" value="light" bind:group={theme} /> Light
        </label>
        <label class="radio">
          <input type="radio" name="theme" value="dark" bind:group={theme} /> Dark
        </label>
      </fieldset>

      <div class="actions">
        <button type="button" class="secondary" onclick={onClose}>Cancel</button>
        <button type="submit">Save</button>
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.45);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .dialog {
    width: min(28rem, 92vw);
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 0.75rem;
    box-shadow: 0 1rem 3rem rgba(15, 23, 42, 0.3);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.85rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  header h2 {
    margin: 0;
    font-size: 1rem;
  }

  .icon {
    font-size: 1.3rem;
    line-height: 1;
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 0 0.25rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
    padding: 1rem;
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

  small {
    font-weight: 400;
    color: var(--muted);
  }

  fieldset {
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    display: flex;
    gap: 1rem;
    padding: 0.5rem 0.75rem;
  }

  legend {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted);
    padding: 0 0.35rem;
  }

  label.radio {
    flex-direction: row;
    align-items: center;
    gap: 0.35rem;
    font-weight: 500;
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

  button.secondary {
    background: var(--surface);
    color: var(--muted);
    border-color: var(--border);
  }
</style>
