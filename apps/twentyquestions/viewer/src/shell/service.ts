/**
 * The episode-service client: every REST call plus the per-episode feed.
 *
 * The browser talks ONLY to the episode service (ADR-0003) -- no NATS. This
 * module wraps the contract endpoints (see `../contract`) as plain typed
 * functions and exposes `openFeed`, which opens the per-episode WebSocket,
 * parses each frame into a typed `FeedEvent`, and hands back a close handle.
 *
 * The base URL comes from settings and is passed in per call, so the client is
 * stateless and easy to test. `wsOrigin` derives the `ws(s)://` feed origin from
 * the same configured `http(s)://` base the REST calls use.
 */
import { wsOrigin } from "../config";
import {
  parseFeedEvent,
  type CreateEpisodeRequest,
  type EpisodeView,
  type ExportEpisodeRequest,
  type ExportEpisodeResult,
  type FeedEvent,
  type ImportEpisodeRequest,
  type RenameEpisodeRequest,
} from "../contract";

/** Trim a trailing slash so we can join paths without doubling up. */
function trimBase(base: string): string {
  return base.replace(/\/$/, "");
}

/** A failed REST call, carrying the HTTP status for callers that branch on it. */
export class ServiceError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "ServiceError";
  }
}

/**
 * Build a `ServiceError` for a failed response, appending the server's `detail`
 * when the body carries one.
 *
 * The service's error handlers (e.g. `UnknownAppError` in `freeagent/service/app.py`)
 * return `{"detail": "..."}` -- the actual explanation. A status alone reads like a
 * routing bug; the detail says *why*. We read the body defensively: a non-JSON or
 * empty body, or one without a string `detail`, just yields the status-only message.
 */
async function serviceError(
  method: string,
  url: string,
  response: Response,
): Promise<ServiceError> {
  let message = `${method} ${url} -> ${response.status}`;
  try {
    const body: unknown = await response.json();
    const detail = (body as { detail?: unknown })?.detail;
    if (typeof detail === "string" && detail) {
      message += `: ${detail}`;
    }
  } catch {
    // A non-JSON or empty body carries no detail; the status-only message stands.
  }
  return new ServiceError(response.status, message);
}

/** Issue a JSON request and decode the body, raising `ServiceError` on failure. */
async function requestJson<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, {
    ...init,
    headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
  });
  if (!response.ok) {
    throw await serviceError(init?.method ?? "GET", url, response);
  }
  return (await response.json()) as T;
}

/** GET /freeagent/episodes -> every episode across applications. */
export function listEpisodes(base: string): Promise<EpisodeView[]> {
  return requestJson<EpisodeView[]>(`${trimBase(base)}/freeagent/episodes`);
}

/** GET one episode by id. */
export function getEpisode(
  base: string,
  application: string,
  episodeId: string,
): Promise<EpisodeView> {
  return requestJson<EpisodeView>(
    `${trimBase(base)}/freeagent/${application}/episodes/${episodeId}`,
  );
}

/** POST a create-episode request (201) -> the new episode. */
export function createEpisode(
  base: string,
  application: string,
  body: CreateEpisodeRequest,
): Promise<EpisodeView> {
  return requestJson<EpisodeView>(`${trimBase(base)}/freeagent/${application}/episodes`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/** PATCH an episode's friendly name. */
export function renameEpisode(
  base: string,
  application: string,
  episodeId: string,
  body: RenameEpisodeRequest,
): Promise<EpisodeView> {
  return requestJson<EpisodeView>(
    `${trimBase(base)}/freeagent/${application}/episodes/${episodeId}`,
    { method: "PATCH", body: JSON.stringify(body) },
  );
}

/** DELETE an episode (204; no body). */
export async function deleteEpisode(
  base: string,
  application: string,
  episodeId: string,
): Promise<void> {
  const url = `${trimBase(base)}/freeagent/${application}/episodes/${episodeId}`;
  const response = await fetch(url, { method: "DELETE" });
  if (!response.ok && response.status !== 204) {
    throw await serviceError("DELETE", url, response);
  }
}

/** POST stop -> seal a running episode. */
export function stopEpisode(
  base: string,
  application: string,
  episodeId: string,
): Promise<EpisodeView> {
  return requestJson<EpisodeView>(
    `${trimBase(base)}/freeagent/${application}/episodes/${episodeId}/stop`,
    { method: "POST" },
  );
}

/** POST export -> drain a sealed episode to Parquet. */
export function exportEpisode(
  base: string,
  application: string,
  episodeId: string,
  body: ExportEpisodeRequest,
): Promise<ExportEpisodeResult> {
  return requestJson<ExportEpisodeResult>(
    `${trimBase(base)}/freeagent/${application}/episodes/${episodeId}/export`,
    { method: "POST", body: JSON.stringify(body) },
  );
}

/** POST import -> play a Parquet log into a fresh episode. */
export function importEpisode(
  base: string,
  body: ImportEpisodeRequest,
): Promise<EpisodeView> {
  return requestJson<EpisodeView>(`${trimBase(base)}/freeagent/import`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/** A handle to an open feed: call `close()` to detach the WebSocket. */
export interface FeedHandle {
  close(): void;
}

/**
 * Open the per-episode feed over WebSocket.
 *
 * The server pushes one JSON `FeedEvent` per text frame: a `status` snapshot,
 * a `connection` (open), `message` history, a `connection` (live) boundary,
 * live `message`s, and a `connection` (closed) for a sealed episode. Each frame
 * is parsed with `parseFeedEvent` and handed to `onEvent`. Transport failures
 * and unparseable frames go to `onError`. The returned handle closes the socket
 * and suppresses any further callbacks.
 */
export function openFeed(
  base: string,
  application: string,
  episodeId: string,
  onEvent: (event: FeedEvent) => void,
  onError?: (error: unknown) => void,
): FeedHandle {
  const origin = wsOrigin(base);
  const url = `${origin}/freeagent/${application}/episodes/${episodeId}/feed`;
  const socket = new WebSocket(url);
  let closed = false;

  socket.onmessage = (event) => {
    if (closed) return;
    try {
      onEvent(parseFeedEvent(String(event.data)));
    } catch (error) {
      onError?.(error);
    }
  };
  socket.onerror = () => {
    if (!closed) onError?.(new Error(`feed socket error for ${episodeId}`));
  };

  return {
    close() {
      closed = true;
      // Drop handlers first so an in-flight close never re-enters callbacks.
      socket.onmessage = null;
      socket.onerror = null;
      if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
        socket.close();
      }
    },
  };
}
