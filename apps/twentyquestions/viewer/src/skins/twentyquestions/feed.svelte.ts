/**
 * The Twenty Questions feed view-model: normalize one episode's feed into the
 * reactive state the skin renders.
 *
 * This is the skin's only stateful piece. It opens the episode feed via the
 * shell's `openFeed`, then folds the typed `FeedEvent` stream into:
 *
 *   - `messages`: the public-channel speech, as chat bubbles, plus the Host's
 *     `game_over` highlight -- the same list whether the episode is live or a
 *     sealed replay (that's the point of ADR-0003's atemporal feed),
 *   - `phase`: the connection liveness (`open` history -> `live` -> `closed`),
 *   - `status`/`sealed`/`outcome`: the latest snapshot from `status` events,
 *   - a best-effort question count for the budget chip.
 *
 * It never speaks NATS; it consumes only the service's normalized events.
 */
import {
  isConnectionEvent,
  isMessageEvent,
  isStatusEvent,
  type EpisodeMessage,
  type FeedEvent,
} from "../../contract";
import { isGameOver, type GameOver } from "../../messages";
import { openFeed, type FeedHandle } from "../../shell/service";

/** The liveness of the feed, mirrored from `connection` events. */
export type FeedPhase = "connecting" | "open" | "live" | "closed" | "error";

/** One rendered chat bubble, derived from a public-channel feed message. */
export interface ChatMessage {
  /** Stable key for `{#each}` (the stream sequence). */
  key: string;
  /** Stream sequence -- the authoritative total order. */
  seq: number;
  /** Who spoke: an agent id, or "env"/"host". */
  sender: string;
  /** What was said, rendered to display text. */
  text: string;
  /** True when this frame carried the Host's structured game-over signal. */
  outcome: boolean;
}

/** Render an arbitrary payload to display text (public speech is a bare string). */
function renderPayload(payload: unknown): string {
  if (typeof payload === "string") return payload;
  if (payload === null || payload === undefined) return "";
  return JSON.stringify(payload);
}

/** True when an episode message carries the Host's `game_over` payload. */
function messageIsGameOver(message: EpisodeMessage): boolean {
  // `isGameOver` works on an Envelope shape; an EpisodeMessage exposes the same
  // `payload`, so adapt to a minimal envelope to reuse the guard.
  return isGameOver({ episode_id: "", sender: message.sender, payload: message.payload });
}

/** Extract the GameOver payload from a message, if it carries one. */
function gameOverOf(message: EpisodeMessage): GameOver | null {
  const payload = message.payload as Partial<GameOver> | null | undefined;
  if (payload && typeof payload === "object" && payload.type === "game_over") {
    return payload as GameOver;
  }
  return null;
}

/**
 * The reactive view-model for one episode's feed. Construct it, call `open`
 * with the service base, and read its `$state` fields in the component. Call
 * `close` (or `open` again for another episode) to detach.
 */
export class FeedViewModel {
  /** Public-channel chat bubbles, in stream order. */
  messages = $state<ChatMessage[]>([]);
  /** Feed liveness. */
  phase = $state<FeedPhase>("connecting");
  /** Latest episode status label from `status` snapshots. */
  status = $state("");
  /** Whether the episode is sealed (finished/replayable). */
  sealed = $state(false);
  /** How it finished (won/lost/aborted/…), if known. */
  outcome = $state<string | null>(null);
  /** A human-readable connection/status note. */
  detail = $state<string | null>(null);
  /** Questions the Host announced were asked, when the game-over signal lands. */
  questionsAsked = $state<number | null>(null);

  #handle: FeedHandle | null = null;
  /** Bumped on each open so a stale socket's late frames are ignored. */
  #generation = 0;

  /** Open the feed for `application`/`episodeId` against the service `base`. */
  open(base: string, application: string, episodeId: string): void {
    this.close();
    const generation = ++this.#generation;

    this.messages = [];
    this.phase = "connecting";
    this.status = "";
    this.sealed = false;
    this.outcome = null;
    this.detail = null;
    this.questionsAsked = null;

    this.#handle = openFeed(
      base,
      application,
      episodeId,
      (event) => {
        if (generation === this.#generation) this.#apply(event);
      },
      (error) => {
        if (generation !== this.#generation) return;
        this.phase = "error";
        this.detail = error instanceof Error ? error.message : String(error);
      },
    );
  }

  /** Detach the feed, suppressing any further frames. */
  close(): void {
    this.#generation++;
    this.#handle?.close();
    this.#handle = null;
  }

  /** Fold one feed event into the view-model. */
  #apply(event: FeedEvent): void {
    if (isStatusEvent(event)) {
      this.status = event.status;
      this.sealed = event.sealed;
      this.outcome = event.outcome ?? null;
      this.detail = event.detail ?? this.detail;
      return;
    }
    if (isConnectionEvent(event)) {
      this.phase = event.phase;
      if (event.detail) this.detail = event.detail;
      return;
    }
    if (isMessageEvent(event)) {
      this.#appendMessage(event.message);
    }
  }

  /** Append a public-channel speech message (and pick up any game-over signal). */
  #appendMessage(message: EpisodeMessage): void {
    const over = gameOverOf(message);
    if (over) this.questionsAsked = over.questions_asked;

    // The transcript is public speech; non-public channels (control/env/log)
    // are framework plumbing and not shown as bubbles -- except we still let a
    // game-over signal through so the outcome line is visible.
    if (message.channel !== "public" && !over) return;

    this.messages = [
      ...this.messages,
      {
        key: `seq-${message.stream_seq}`,
        seq: message.stream_seq,
        sender: message.sender,
        text: over
          ? `Game over — ${over.outcome === "win" ? "won" : "lost"} (secret: ${over.secret})`
          : renderPayload(message.payload),
        outcome: over !== null || messageIsGameOver(message),
      },
    ];
  }
}
