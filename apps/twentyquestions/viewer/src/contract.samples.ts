/**
 * Sample contract payloads, type-checked against the generated types.
 *
 * These mirror what the mock service (`python -m freeagent.service.mock`) and
 * the real service emit. They are not used at runtime; they exist so
 * `pnpm run typecheck` proves the generated types accept real-shaped payloads
 * and that the `FeedEvent` union narrows correctly (ADR-0003 / #39 acceptance:
 * "generated TS types exist and typecheck against sample payloads").
 */
import type { CreateEpisodeRequest, EpisodeView, FeedEvent } from "./contract";
import { isMessageEvent, isStatusEvent } from "./contract";

export const sampleEpisode: EpisodeView = {
  id: "elephant",
  application: "twenty-questions",
  app: "twentyquestions",
  episode_id: "elephant",
  subject_root: "twentyquestions.episode.elephant",
  name: "elephant",
  mode: "live",
  status: "ended",
  sealed: true,
  outcome: "won",
  detail: "won in 3 questions",
  nats_url: "nats://localhost:4222",
  created_at: "2026-06-25T12:00:00Z",
};

export const sampleCreate: CreateEpisodeRequest = {
  mode: "live",
  name: "my game",
  model: "claude-haiku-4-5-20251001",
  agents: { host: { config: { secret: "otter" } } },
};

export const sampleFeed: FeedEvent[] = [
  { type: "status", status: "running", sealed: false, name: "elephant", outcome: null },
  { type: "connection", phase: "open" },
  {
    type: "message",
    message: {
      stream_seq: 3,
      subject: "twentyquestions.episode.elephant.public",
      channel: "public",
      sender: "alice",
      received_at: "2026-06-25T12:00:03Z",
      payload: "Is it an animal?",
    },
  },
  { type: "connection", phase: "live" },
];

/** A trivial exercise of the narrowing guards, so they are covered statically. */
export function summarize(events: FeedEvent[]): { messages: number; status: string | null } {
  let messages = 0;
  let status: string | null = null;
  for (const event of events) {
    if (isMessageEvent(event)) messages += 1;
    else if (isStatusEvent(event)) status = event.status;
  }
  return { messages, status };
}
