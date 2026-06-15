/**
 * The episode viewer: subscribe to an episode's public channel over NATS and
 * expose its messages and connection status as reactive Svelte state.
 *
 * One code path serves live and replay (ADR-0001): the replayer re-publishes a
 * recorded episode onto NATS with byte-identical subjects, so this subscriber
 * cannot tell them apart. We read the episode's JetStream stream from sequence 1
 * (an ordered consumer with `DeliverPolicy.ALL`), exactly as the engine does, so
 * the full transcript shows in `stream_seq` order whether the viewer joined at
 * the start, mid-episode, or after the fact.
 *
 * Decoding goes through the generated message types (`./messages`): every frame
 * is a FreeAgent `Envelope`, and on the public channel its `payload` is the bare
 * spoken text.
 */
import {
  connect,
  consumerOpts,
  Events,
  JSONCodec,
  type JsMsg,
  type NatsConnection,
} from "nats.ws";

import { isGameOver, type Envelope } from "./messages";

export type ConnectionState =
  | "idle"
  | "connecting"
  | "waiting"
  | "live"
  | "reconnecting"
  | "closed"
  | "error";

/** One rendered chat message: a sender, its text, and its place in the order. */
export interface ChatMessage {
  /** Stream sequence — the authoritative total order across the episode. */
  seq: number;
  /** Stable key for `{#each}` (the envelope's message_id, or a seq fallback). */
  key: string;
  /** Who spoke: an agent id, or "env" for the environment. */
  sender: string;
  /** What was said: the envelope payload rendered to text. */
  text: string;
  /** Whether this frame carried the Host's structured game-over signal. */
  outcome: boolean;
}

const codec = JSONCodec<Envelope>();

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** A public-channel payload is the bare spoken string; anything else is shown raw. */
function renderPayload(payload: Envelope["payload"]): string {
  if (typeof payload === "string") return payload;
  if (payload === null || payload === undefined) return "";
  return JSON.stringify(payload);
}

/** JetStream reports a missing stream when the episode has not started yet. */
function isStreamNotFound(err: unknown): boolean {
  const message = (err instanceof Error ? err.message : String(err)).toLowerCase();
  return (
    message.includes("no stream matches") ||
    message.includes("stream not found") ||
    message.includes("404")
  );
}

export class EpisodeViewer {
  /** Coarse connection state, for the status indicator. */
  state = $state<ConnectionState>("idle");
  /** Human-readable status detail. */
  detail = $state("");
  /** The transcript so far, in stream order. */
  messages = $state<ChatMessage[]>([]);
  /** The server and subject of the active (or last) connection. */
  server = $state("");
  subject = $state("");

  #nc: NatsConnection | null = null;
  /** Bumped on every connect/disconnect; stale async loops check it and bail. */
  #generation = 0;
  /** Highest stream sequence ingested, to drop any redelivered duplicates. */
  #lastSeq = 0;

  /** Connect to *server* and subscribe to *subject*, replacing any prior session. */
  async connect(server: string, subject: string): Promise<void> {
    await this.disconnect();
    const generation = ++this.#generation;

    this.server = server;
    this.subject = subject;
    this.messages = [];
    this.#lastSeq = 0;
    this.state = "connecting";
    this.detail = `connecting to ${server}…`;

    let nc: NatsConnection;
    try {
      nc = await connect({
        servers: server,
        name: "twentyquestions-viewer",
        // Keep retrying through a missing/blinking server: the viewer may be
        // opened before the episode (or replay) starts and must light up when
        // it does, never publishing — only subscribing.
        reconnect: true,
        maxReconnectAttempts: -1,
        reconnectTimeWait: 1000,
        waitOnFirstConnect: true,
      });
    } catch (err) {
      if (generation !== this.#generation) return;
      this.state = "error";
      this.detail = `could not connect to ${server}: ${String(err)}`;
      return;
    }

    if (generation !== this.#generation) {
      // Superseded while connecting; drop this connection.
      await nc.close();
      return;
    }

    this.#nc = nc;
    void this.#watchStatus(nc, generation);
    void this.#consume(nc, subject, generation);
    void nc.closed().then((err) => {
      if (generation !== this.#generation) return;
      this.#nc = null;
      if (this.state !== "error") {
        this.state = "closed";
        this.detail = err ? `connection closed: ${String(err)}` : "connection closed";
      }
    });
  }

  /** Tear down the active connection, if any. */
  async disconnect(): Promise<void> {
    this.#generation++;
    const nc = this.#nc;
    this.#nc = null;
    if (nc && !nc.isClosed()) {
      try {
        await nc.drain();
      } catch {
        // Best-effort teardown; the connection is going away regardless.
      }
    }
    if (this.state !== "idle") {
      this.state = "closed";
      this.detail = "disconnected";
    }
  }

  /** Mirror nats.ws lifecycle events into the status indicator. */
  async #watchStatus(nc: NatsConnection, generation: number): Promise<void> {
    for await (const status of nc.status()) {
      if (generation !== this.#generation) return;
      switch (status.type) {
        case Events.Disconnect:
          this.state = "reconnecting";
          this.detail = `disconnected from ${status.data ?? this.server}; reconnecting…`;
          break;
        case Events.Reconnect:
          this.state = "live";
          this.detail = `reconnected to ${status.data ?? this.server}`;
          break;
        case Events.Error:
          this.detail = `connection error: ${String(status.data)}`;
          break;
        default:
          break;
      }
    }
  }

  /** Subscribe from sequence 1, waiting for the episode's stream to appear. */
  async #consume(nc: NatsConnection, subject: string, generation: number): Promise<void> {
    const js = nc.jetstream();
    while (generation === this.#generation) {
      const opts = consumerOpts();
      opts.orderedConsumer(); // ephemeral, reads from sequence 1, ordered
      try {
        const sub = await js.subscribe(subject, opts);
        if (generation !== this.#generation) {
          await sub.unsubscribe();
          return;
        }
        if (this.state !== "reconnecting") this.state = "live";
        this.detail = `watching ${subject}`;
        for await (const msg of sub) {
          if (generation !== this.#generation) return;
          this.#ingest(msg);
        }
        return; // the subscription ended on its own
      } catch (err) {
        if (generation !== this.#generation) return;
        if (isStreamNotFound(err)) {
          this.state = "waiting";
          this.detail = `waiting for the episode to start (${subject})…`;
          await delay(500);
          continue;
        }
        this.state = "error";
        this.detail = `subscription failed: ${String(err)}`;
        return;
      }
    }
  }

  /** Decode one frame and append it to the transcript, dropping duplicates. */
  #ingest(msg: JsMsg): void {
    const seq = msg.seq;
    if (seq <= this.#lastSeq) return; // a redelivery after a consumer reset
    this.#lastSeq = seq;

    let envelope: Envelope;
    try {
      envelope = codec.decode(msg.data);
    } catch {
      return; // not a JSON envelope; nothing the viewer can render
    }

    this.messages = [
      ...this.messages,
      {
        seq,
        key: envelope.message_id ?? `seq-${seq}`,
        sender: envelope.sender,
        text: renderPayload(envelope.payload),
        outcome: isGameOver(envelope),
      },
    ];
  }
}
