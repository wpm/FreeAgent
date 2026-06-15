/**
 * The episode viewer: subscribe to an episode's public channel over NATS and
 * expose its messages and connection status as reactive Svelte state.
 *
 * One code path serves live and replay (ADR-0001): the replayer re-publishes a
 * recorded episode onto NATS with byte-identical subjects, so this subscriber
 * cannot tell them apart. We read the episode's JetStream stream from sequence 1
 * (an ordered consumer with `DeliverPolicy.All`), exactly as the engine does, so
 * the full transcript shows in `stream_seq` order whether the viewer joined at
 * the start, mid-episode, or after the fact.
 *
 * Decoding goes through the generated message types (`./messages`): every frame
 * is a FreeAgent `Envelope`, and on the public channel its `payload` is the bare
 * spoken text.
 *
 * The transport is the maintained nats.js v3 browser client (`@nats-io/nats-core`
 * `wsconnect` + `@nats-io/jetstream`), not the deprecated `nats.ws` package.
 */
import { wsconnect, type NatsConnection } from "@nats-io/nats-core";
import {
  DeliverPolicy,
  jetstream,
  jetstreamManager,
  JetStreamApiCodes,
  JetStreamApiError,
  type ConsumerMessages,
  type JsMsg,
} from "@nats-io/jetstream";

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

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** A public-channel payload is the bare spoken string; anything else is shown raw. */
function renderPayload(payload: Envelope["payload"]): string {
  if (typeof payload === "string") return payload;
  if (payload === null || payload === undefined) return "";
  return JSON.stringify(payload);
}

/**
 * The episode's stream is absent until the environment (live) or the replayer
 * creates it. JetStream signals that with a structured `StreamNotFound` API
 * error (code 10059), so we match on the code rather than the message text.
 */
function isStreamNotFound(err: unknown): boolean {
  return err instanceof JetStreamApiError && err.code === JetStreamApiCodes.StreamNotFound;
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
  /** The active consume iterator, so disconnect can stop it promptly. */
  #consumed: ConsumerMessages | null = null;
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

    const nc = await this.#openConnection(server, generation);
    if (nc === null) return; // superseded while connecting

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

  /**
   * Open a connection, retrying a missing/unreachable server with visible
   * status instead of blocking silently. The viewer may be opened before the
   * server or episode exists and must light up when it appears (it only ever
   * subscribes) — but a wrong URL or a down server should show progress and the
   * underlying error, not a perpetual, featureless "connecting…".
   *
   * Each first-connect attempt is bounded by `timeout` so a down server surfaces
   * quickly; `reconnect` then handles drops after a successful connect. Returns
   * the connection, or null if a later connect/disconnect superseded this one.
   */
  async #openConnection(server: string, generation: number): Promise<NatsConnection | null> {
    for (let attempt = 1; generation === this.#generation; attempt++) {
      try {
        const nc = await wsconnect({
          servers: server,
          name: "twentyquestions-viewer",
          reconnect: true,
          maxReconnectAttempts: -1,
          reconnectTimeWait: 1000,
          timeout: 5000,
        });
        if (generation !== this.#generation) {
          await nc.close();
          return null;
        }
        return nc;
      } catch (err) {
        if (generation !== this.#generation) return null;
        this.state = "connecting";
        this.detail = `cannot reach ${server} (attempt ${attempt}); retrying… [${String(err)}]`;
        await delay(1000);
      }
    }
    return null;
  }

  /** Tear down the active connection, if any. */
  async disconnect(): Promise<void> {
    this.#generation++;
    const consumed = this.#consumed;
    this.#consumed = null;
    consumed?.stop();
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

  /** Mirror connection lifecycle events into the status indicator. */
  async #watchStatus(nc: NatsConnection, generation: number): Promise<void> {
    for await (const status of nc.status()) {
      if (generation !== this.#generation) return;
      switch (status.type) {
        case "disconnect":
          this.state = "reconnecting";
          this.detail = `disconnected from ${status.server}; reconnecting…`;
          break;
        case "reconnect":
          this.state = "live";
          this.detail = `reconnected to ${status.server}`;
          break;
        case "error":
          this.detail = `connection error: ${String(status.error)}`;
          break;
        default:
          break;
      }
    }
  }

  /**
   * Subscribe from sequence 1, re-establishing through transient failures.
   *
   * The episode's JetStream stream is created by the environment (live) or the
   * replayer (replay); it may not exist yet when the viewer opens, so we wait
   * for it rather than failing, mirroring the engine's subscribe-from-seq-1
   * logic. A subscription that drops mid-episode — a transient JetStream/API
   * error, or the ordered consumer's iterator ending on a reconnect — is
   * re-established rather than going permanently dark; the `#lastSeq` de-dup
   * guard means re-reading from sequence 1 never double-renders. The loop ends
   * only when this session is superseded or the connection closes for good.
   */
  async #consume(nc: NatsConnection, subject: string, generation: number): Promise<void> {
    const jsm = await jetstreamManager(nc);
    const js = jetstream(nc);

    while (generation === this.#generation && !nc.isClosed()) {
      let stream: string;
      try {
        stream = await jsm.streams.find(subject);
      } catch (err) {
        if (generation !== this.#generation) return;
        if (isStreamNotFound(err)) {
          this.state = "waiting";
          this.detail = `waiting for the episode to start (${subject})…`;
        } else {
          // A transient API error locating the stream: keep trying.
          this.detail = `locating the episode stream… [${String(err)}]`;
        }
        await delay(500);
        continue;
      }

      try {
        // An ordered (ephemeral) consumer reading the whole stream from sequence
        // 1, filtered to just the public channel — the stream captures every
        // episode subject (`<root>.>`), but the room is only public broadcast.
        const consumer = await js.consumers.get(stream, {
          filter_subjects: subject,
          deliver_policy: DeliverPolicy.All,
        });
        const messages = await consumer.consume();
        if (generation !== this.#generation) {
          messages.stop();
          return;
        }
        this.#consumed = messages;
        if (this.state !== "reconnecting") this.state = "live";
        this.detail = `watching ${subject}`;
        for await (const msg of messages) {
          if (generation !== this.#generation) return;
          this.#ingest(msg);
        }
        // Iterator ended without throwing (e.g. a consumer reset): loop to
        // re-establish from where the de-dup guard left off.
      } catch (err) {
        if (generation !== this.#generation) return;
        this.state = "reconnecting";
        this.detail = `subscription interrupted; retrying… [${String(err)}]`;
        await delay(1000);
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
      envelope = msg.json<Envelope>();
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
