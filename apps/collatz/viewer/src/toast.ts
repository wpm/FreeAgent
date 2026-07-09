/**
 * Toast state: what an error looks like as a toast, and which errors share one.
 *
 * Pure logic only, like `state.ts` — the DOM face of a toast (the stacked cards, the dismiss
 * button) lives in `main.ts`. Toasts are deliberately transient UI state: nothing here persists
 * anything, and nothing expires anything — a toast exists from `add` until the *user's* `dismiss`,
 * and a page reload starts empty (issue #118).
 */

import { ApiError } from "./api.js";

/** What a toast displays: an optional one-line title and the message body. */
export interface ToastContent {
  /** The headline — an API failure's `METHOD /path → status` — or `null` for plain messages. */
  title: string | null;
  /** The human-readable description below the title. */
  body: string;
}

/**
 * Compose a thrown error's toast, structured rather than a dumped `Error.message`.
 *
 * An `ApiError`'s fields become a `METHOD /path → status` title with the server's extracted
 * detail as the body (or a placeholder when the response carried none, so a bodyless 502 still
 * reads as something). Any other `Error` — e.g. the launch form's validation errors — is its
 * message alone, and a non-`Error` throw is stringified.
 */
export function formatToast(error: unknown): ToastContent {
  if (error instanceof ApiError) {
    return {
      title: `${error.method} ${error.path} → ${error.status}`,
      body: error.detail !== "" ? error.detail : "(the response carried no detail)",
    };
  }
  if (error instanceof Error) {
    return { title: null, body: error.message };
  }
  return { title: null, body: String(error) };
}

/** `add`'s outcome: a fresh toast to render, or another occurrence of one already showing. */
export type ToastEvent =
  | { kind: "created"; id: number; content: ToastContent }
  | { kind: "repeated"; id: number; count: number };

/**
 * The set of currently-displayed toasts, owning the coalescing rule.
 *
 * An error identical (same title and body) to a toast still on screen coalesces into it —
 * `add` reports `repeated` with the bumped occurrence count instead of `created`. This is what
 * keeps the background poll from stacking an unbounded pile of the same connectivity complaint
 * during an outage, and it is not auto-dismissal: nothing leaves the store except by `dismiss`.
 * Dismissal resets coalescing — the same error arriving after its toast was dismissed is a new
 * toast, since the user acknowledged only the occurrences they saw.
 */
export class ToastStore {
  private nextId = 1;
  private readonly live = new Map<number, { content: ToastContent; count: number }>();

  /** Record an error's occurrence: coalesce onto an identical live toast or create a new one. */
  add(content: ToastContent): ToastEvent {
    for (const [id, toast] of this.live) {
      if (toast.content.title === content.title && toast.content.body === content.body) {
        toast.count += 1;
        return { kind: "repeated", id, count: toast.count };
      }
    }
    const id = this.nextId;
    this.nextId += 1;
    this.live.set(id, { content, count: 1 });
    return { kind: "created", id, content };
  }

  /** Forget a toast the user dismissed; its content may toast again afterwards. */
  dismiss(id: number): void {
    this.live.delete(id);
  }
}
