/**
 * The control-service REST client: how the browser app configures, launches,
 * and stops episodes.
 *
 * This is the app's whole control plane. The browser **never publishes to NATS**
 * (ADR-0001): every mutation — start a live episode, replay a recording, stop
 * one — goes over REST to the FreeAgent control service, and the NATS side stays
 * strictly read-only (the `EpisodeViewer` only ever subscribes). It is a small
 * module *inside* the app, not a standalone SDK: only as much shared JS as the
 * viewer needs to talk to the service.
 *
 * The wire shapes mirror the service's Pydantic models
 * (`freeagent.service.models`): a `CreateEpisodeRequest` in, an `EpisodeView`
 * out. The settable config an operator drives from the browser — the Host's
 * secret, a model override, the logging file — rides inside the `config` blocks
 * keyed exactly as the app's settable-config surface advertises them
 * (`agents.host.config.secret`, `agents.<name>.config.model`, …), so the browser
 * sets exactly what the CLI sets from YAML.
 *
 * The REST resources live under `/freeagent/<application>/<episode>`, where
 * `<application>` is the dashed entry-point name (`twenty-questions`) — distinct
 * from the undashed subject prefix (`twentyquestions`). A returned view carries
 * its own `subject_root` (`<app>.episode.<id>`); the viewer builds its public
 * subject from that mapping rather than re-deriving it, since a replay's subject
 * identity comes from the recording, not from the REST id.
 */

/** An episode is launched live (real environment + roster) or replayed. */
export type EpisodeMode = "live" | "replay";

/** One component's verbatim constructor `config` block (environment or agent). */
export interface ComponentConfig {
  config: Record<string, unknown>;
}

/**
 * The body of a create-episode request. Mirrors the service's
 * `CreateEpisodeRequest`: `mode` selects the launch path, the `config` blocks
 * carry the settable surface for a live launch, and `parquet_path` names the
 * recording to replay. Everything but `mode` is optional.
 */
export interface CreateEpisodeRequest {
  mode: EpisodeMode;
  /** Omit to let the service mint a short id; supply one (`[A-Za-z0-9_-]+`) to fix it. */
  episode_id?: string | null;
  /** Override the service's default NATS URL for this episode. */
  nats_url?: string | null;
  /** Record a *live* episode to this Parquet path (the "logging file"). */
  parquet_log?: string | null;
  /** The recorded Parquet log to replay; required when `mode` is `replay`. */
  parquet_path?: string | null;
  environment?: ComponentConfig;
  agents?: Record<string, ComponentConfig>;
}

/**
 * The REST view of one registered episode — what list/get/create/stop return.
 *
 * `id` is the service-level REST id used in the resource path; `app` and
 * `episode_id` are the subject-level identity, and `subject_root` is what a
 * viewer subscribes under. For a live episode the REST `id` *is* the
 * `episode_id`; for a replay they differ, so always read `subject_root`.
 */
export interface EpisodeView {
  id: string;
  application: string;
  app: string;
  episode_id: string;
  subject_root: string;
  mode: EpisodeMode;
  status: string;
  detail: string | null;
  /**
   * Whether the service still holds a handle it can gracefully stop — true only
   * while the episode is running under *this* service instance. After a restart
   * a pre-restart episode has no view at all (the in-memory registry forgot it),
   * so discovery finds it over JetStream but offers no Stop.
   */
  controllable: boolean;
  nats_url: string;
  created_at: string;
}

/** The result of a teardown: how many episodes were brought down. */
export interface TeardownResult {
  stopped: number;
}

/** The public broadcast subject a viewer watches for an episode view. */
export function publicSubjectOf(view: EpisodeView): string {
  return `${view.subject_root}.public`;
}

/** A control-service request that came back as a non-2xx response. */
export class ControlError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ControlError";
    this.status = status;
  }
}

/**
 * A thin REST client bound to one control-service base URL.
 *
 * Each method maps onto one endpoint and returns the decoded JSON, raising a
 * `ControlError` (carrying the service's `detail` message and HTTP status) on a
 * non-2xx response so the UI can show *why* a launch was refused — NATS down
 * (503), unknown app (404), id taken (409), bad config (400) — rather than a
 * bare failure.
 */
export class ControlClient {
  readonly baseUrl: string;

  constructor(baseUrl: string) {
    // Trim a trailing slash so `${baseUrl}/freeagent/...` never doubles up.
    this.baseUrl = baseUrl.replace(/\/+$/, "");
  }

  /** Create (live) or replay an episode of *application*; returns its view. */
  async createEpisode(application: string, body: CreateEpisodeRequest): Promise<EpisodeView> {
    return this.#send<EpisodeView>(
      "POST",
      `/freeagent/${encodeURIComponent(application)}/episodes`,
      body,
    );
  }

  /** Every registered episode the service is tracking, in creation order. */
  async listEpisodes(): Promise<EpisodeView[]> {
    return this.#send<EpisodeView[]>("GET", "/freeagent/episodes");
  }

  /** The current view of one episode (its live status and detail). */
  async getEpisode(application: string, id: string): Promise<EpisodeView> {
    return this.#send<EpisodeView>(
      "GET",
      `/freeagent/${encodeURIComponent(application)}/episodes/${encodeURIComponent(id)}`,
    );
  }

  /** Gracefully stop one episode; returns its now-terminal view. */
  async stopEpisode(application: string, id: string): Promise<EpisodeView> {
    return this.#send<EpisodeView>(
      "POST",
      `/freeagent/${encodeURIComponent(application)}/episodes/${encodeURIComponent(id)}/stop`,
    );
  }

  /** Bring every episode down at once; returns how many were stopped. */
  async teardown(): Promise<TeardownResult> {
    return this.#send<TeardownResult>("POST", "/freeagent/teardown");
  }

  /** Issue one request and decode it, turning a non-2xx into a `ControlError`. */
  async #send<T>(method: string, path: string, body?: unknown): Promise<T> {
    let response: Response;
    try {
      response = await fetch(`${this.baseUrl}${path}`, {
        method,
        headers: body === undefined ? undefined : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (err) {
      // A network-level failure (service down, CORS, wrong URL) never reaches an
      // HTTP status; surface the cause with the same shape as a service error.
      throw new ControlError(0, `cannot reach control service at ${this.baseUrl} [${String(err)}]`);
    }
    if (!response.ok) {
      throw new ControlError(response.status, await detailOf(response));
    }
    return (await response.json()) as T;
  }
}

/** The service's `{ detail }` problem message, falling back to the status text. */
async function detailOf(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { detail?: unknown };
    if (typeof body.detail === "string" && body.detail) return body.detail;
  } catch {
    // Not a JSON problem document; fall back to the status line below.
  }
  return `${response.status} ${response.statusText}`.trim();
}
