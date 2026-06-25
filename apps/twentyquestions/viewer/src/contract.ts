/**
 * The episode-service contract (ADR-0003), TypeScript side.
 *
 * Everything under `./generated/` is produced from the FreeAgent JSON Schema
 * (run `pnpm run schemas` at the repo root) and must not be edited by hand.
 * This module composes those generated interfaces into the shapes a UI works
 * with: the REST resource types and the `FeedEvent` discriminated union the
 * service pushes over the per-episode feed. Importing the generated types here
 * also serves as the contract's compile test -- if they drift or fail to
 * regenerate, this file stops type-checking (`pnpm run typecheck`).
 */
import type { CreateEpisodeRequest } from "./generated/create_episode_request.schema";
import type { EpisodeView } from "./generated/episode_view.schema";
import type { ExportEpisodeRequest } from "./generated/export_episode_request.schema";
import type { ExportEpisodeResult } from "./generated/export_episode_result.schema";
import type {
  EpisodeMessage,
  FeedMessageEvent,
} from "./generated/feed_message_event.schema";
import type { FeedConnectionEvent } from "./generated/feed_connection_event.schema";
import type { FeedStatusEvent } from "./generated/feed_status_event.schema";
import type { ImportEpisodeRequest } from "./generated/import_episode_request.schema";
import type { RenameEpisodeRequest } from "./generated/rename_episode_request.schema";
import type { TeardownResult } from "./generated/teardown_result.schema";

export type {
  CreateEpisodeRequest,
  EpisodeMessage,
  EpisodeView,
  ExportEpisodeRequest,
  ExportEpisodeResult,
  FeedConnectionEvent,
  FeedMessageEvent,
  FeedStatusEvent,
  ImportEpisodeRequest,
  RenameEpisodeRequest,
  TeardownResult,
};

/**
 * One event on an episode's feed. The service sends exactly one of these three
 * shapes per frame, discriminated by `type`; the UI never parses a NATS
 * envelope or subject.
 */
export type FeedEvent = FeedMessageEvent | FeedStatusEvent | FeedConnectionEvent;

/** Narrow a feed event to the message-appended shape. */
export function isMessageEvent(event: FeedEvent): event is FeedMessageEvent {
  return event.type === "message";
}

/** Narrow a feed event to the status/seal-change shape. */
export function isStatusEvent(event: FeedEvent): event is FeedStatusEvent {
  return event.type === "status";
}

/** Narrow a feed event to the connection/liveness shape. */
export function isConnectionEvent(event: FeedEvent): event is FeedConnectionEvent {
  return event.type === "connection";
}

/** Parse one feed frame (JSON text) into a typed `FeedEvent`. */
export function parseFeedEvent(frame: string): FeedEvent {
  return JSON.parse(frame) as FeedEvent;
}
