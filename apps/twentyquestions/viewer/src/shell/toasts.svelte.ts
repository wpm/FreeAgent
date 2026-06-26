/**
 * The shell's toast store: a general-purpose, pan-application notification stack.
 *
 * A toast is a message the operator must acknowledge. Unlike the inline,
 * transient error displays it replaces, a toast persists until the user
 * dismisses it -- errors do not auto-vanish -- and shows its full message text
 * (no truncation into a tooltip). The store lives in `shell/` like the service
 * client, so any skin can raise a toast without knowing about another's UI.
 *
 * It is a Svelte 5 runes class: read `toasts` reactively and mutate through
 * `add`/`dismiss`. A single shared instance (`toasts`) is exported for the app;
 * the class is exported too so tests can drive an isolated store.
 */

/** A toast's severity, driving its accent colour. Errors are the common case. */
export type ToastLevel = "error" | "info";

/** One live notification: a stable id, the full message, and its level. */
export interface Toast {
  /** Monotonic id, unique within this store; the dismiss key and `{#each}` key. */
  id: number;
  /** The full message text, shown verbatim (never truncated). */
  message: string;
  /** Severity accent; defaults to `error` when raised. */
  level: ToastLevel;
}

/** A reactive stack of toasts the operator must dismiss. */
export class ToastStore {
  /** The live toasts, oldest first; render them stacked in a fixed corner. */
  toasts = $state<Toast[]>([]);

  /** Next id to hand out; monotonic so dismissed ids are never reused. */
  #nextId = 1;

  /** Raise a toast and return its id (so a caller can dismiss it later). */
  add(message: string, level: ToastLevel = "error"): number {
    const id = this.#nextId++;
    this.toasts.push({ id, message, level });
    return id;
  }

  /** Remove the toast with this id; a no-op if it is already gone. */
  dismiss(id: number): void {
    this.toasts = this.toasts.filter((toast) => toast.id !== id);
  }
}

/** The single shared toast store for the app shell. */
export const toasts = new ToastStore();
