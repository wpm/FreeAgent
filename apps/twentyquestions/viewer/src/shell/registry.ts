/**
 * The right-pane plugin registry: application name -> presentation component.
 *
 * The shell is pan-application and knows nothing about any one game. Each
 * application registers a Svelte component that renders the right pane for its
 * episodes -- the transcript, status chips, and a create composer. The shell
 * looks a plugin up by the selected episode's `application` (or by the default
 * application for the "new" flow).
 *
 * A plugin component receives the props in `PluginProps`: the currently selected
 * episode (or null for a fresh "new" composer), whether a "new game" is being
 * composed, and a callback to report a freshly created episode back to the shell
 * so it can select and open it.
 */
import type { Component } from "svelte";

import type { EpisodeView } from "../contract";

/** Props every right-pane plugin receives from the shell. */
export interface PluginProps {
  /** The selected episode, or null when composing a brand-new game. */
  episode: EpisodeView | null;
  /** True when the operator hit "New" and the create composer should show. */
  composing: boolean;
  /** Report a created episode so the shell selects it and opens its feed. */
  onCreated: (episode: EpisodeView) => void;
  /** Dismiss the create composer without creating (back to the prior view). */
  onCancelCompose: () => void;
}

/** A registered application: its right-pane component. */
export interface PluginEntry {
  component: Component<PluginProps>;
}

const registry = new Map<string, PluginEntry>();

/** Register an application's right-pane plugin under its `application` name. */
export function registerPlugin(application: string, entry: PluginEntry): void {
  registry.set(application, entry);
}

/** Look up a plugin by application name (undefined if none is registered). */
export function getPlugin(application: string): PluginEntry | undefined {
  return registry.get(application);
}

/**
 * The application used for the "new" flow when no episode is selected. The
 * viewer ships with the Twenty Questions skin; a multi-app build would surface
 * a picker instead.
 */
export const DEFAULT_APPLICATION = "twenty-questions";
