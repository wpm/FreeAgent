/**
 * The viewer's REST client for `freeagent-api`, plus hand-written mirrors of the API's
 * control-plane response models.
 *
 * Only the *data-plane payloads* have generated types (`../schema/collatz.d.ts`, regenerated from
 * the engine's pydantic models); the API's own response shapes are SDK-level, stable, and small,
 * so they are mirrored here by hand from `freeagent.api.app` / `freeagent.api.episodes`.
 */

/** The lifecycle states of `freeagent.api.episodes.EpisodeState`. */
export type EpisodeState = "created" | "running" | "complete" | "stopped" | "failed";

/** States an episode never leaves; mirror of `freeagent.api.episodes.TERMINAL_STATES`. */
export const TERMINAL_STATES: ReadonlySet<EpisodeState> = new Set([
  "complete",
  "stopped",
  "failed",
] satisfies EpisodeState[]);

/** Mirror of `freeagent.api.episodes.EpisodeStatus`. */
export interface EpisodeStatus {
  application: string;
  episode_id: string;
  episode_root: string;
  state: EpisodeState;
  agents_alive: string[];
  message_count: number;
  worker_exit_code: number | null;
}

/** Mirror of `freeagent.api.episodes.DataPlaneRecord`: one opaque data-plane message. */
export interface DataPlaneRecord {
  seq: number;
  subject: string;
  message_type: string | null;
  received_at: number;
  payload: unknown;
}

/**
 * A failed API request, carrying the request and response facts as fields.
 *
 * The structured fields exist for display (the toast layer composes its own title and body from
 * them); `message` keeps the flat everything-in-one-string form for logs and non-toast consumers.
 */
export class ApiError extends Error {
  constructor(
    /** The request's HTTP method. */
    readonly method: string,
    /** The request's path below the client's base URL. */
    readonly path: string,
    /** The response's HTTP status code. */
    readonly status: number,
    /**
     * The human-readable failure description: the `detail` field of the API's JSON error body
     * when it has one (FastAPI's error shape), otherwise the raw response text — possibly empty.
     */
    readonly detail: string,
  ) {
    super(`${method} ${path} failed (${status}): ${detail}`);
    this.name = "ApiError";
  }
}

/**
 * Pull the human-readable detail out of an error response body.
 *
 * The API's errors are FastAPI-shaped — `{"detail": "..."}` — so a JSON object with a string
 * `detail` yields that string. Anything else (non-JSON, validation-error arrays, other shapes)
 * falls back to the raw text rather than guessing at structure.
 */
function extractDetail(body: string): string {
  try {
    const parsed: unknown = JSON.parse(body);
    if (
      parsed !== null &&
      typeof parsed === "object" &&
      "detail" in parsed &&
      typeof (parsed as { detail: unknown }).detail === "string"
    ) {
      return (parsed as { detail: string }).detail;
    }
  } catch {
    // Not JSON; the raw text is the best available description.
  }
  return body;
}

/** A thin fetch wrapper over the API's REST surface, rooted at `baseUrl`. */
export class ApiClient {
  constructor(readonly baseUrl: string) {}

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, init);
    if (!response.ok) {
      const body = await response.text();
      throw new ApiError(init?.method ?? "GET", path, response.status, extractDetail(body));
    }
    // A bodyless success (the API's DELETE answers 204) has no JSON to parse.
    if (response.status === 204) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }

  /** `GET /applications`: the bare names of every installed application. */
  async applications(): Promise<string[]> {
    const body = await this.request<{ applications: string[] }>("/applications");
    return body.applications;
  }

  /** `GET .../episodes`: an application's episodes' statuses, in creation order. */
  async episodes(application: string): Promise<EpisodeStatus[]> {
    const body = await this.request<{ episodes: EpisodeStatus[] }>(
      `/applications/${application}/episodes`,
    );
    return body.episodes;
  }

  /** `POST .../episodes`: provision an episode; the config is the application's, opaque here. */
  async createEpisode(
    application: string,
    episodeId: string | null,
    config: object,
  ): Promise<EpisodeStatus> {
    return await this.request<EpisodeStatus>(`/applications/${application}/episodes`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ episode_id: episodeId, config }),
    });
  }

  /** `GET .../episodes/{id}`: one episode's lifecycle status. */
  async status(application: string, episodeId: string): Promise<EpisodeStatus> {
    return await this.request<EpisodeStatus>(`/applications/${application}/episodes/${episodeId}`);
  }

  /** `GET .../messages`: the episode's data-plane feed so far, in arrival order. */
  async messages(application: string, episodeId: string): Promise<DataPlaneRecord[]> {
    const body = await this.request<{ messages: DataPlaneRecord[] }>(
      `/applications/${application}/episodes/${episodeId}/messages`,
    );
    return body.messages;
  }

  /** `DELETE .../episodes/{id}`: stop the episode. */
  async stop(application: string, episodeId: string): Promise<void> {
    await this.request<void>(`/applications/${application}/episodes/${episodeId}`, {
      method: "DELETE",
    });
  }
}
