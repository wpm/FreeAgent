/**
 * Viewer configuration: where to connect and which channel to watch.
 *
 * Two configuration surfaces live here:
 *
 *   - the **observe** surface (`resolveConfig`) — the read-only, shareable-by-URL
 *     NATS settings of the original viewer: a websocket server and a public
 *     subject. Unchanged from the single-episode viewer (ADR-0001).
 *   - the **control** surface (`resolveControllerConfig`) — where the REST
 *     control service lives and which websocket NATS the launched episodes are
 *     watched over, so the app can configure and launch episodes, not just
 *     observe one named by hand.
 *
 * Both come from URL query parameters, falling back to build-time Vite env vars
 * and then to local-dev defaults. The observe channel can be named two ways:
 *
 *   - app name + episode id  ?app=twentyquestions&episode=<id>
 *   - a full subject         ?subject=twentyquestions.episode.<id>.public
 *
 * A full `subject`, when given, wins. The websocket server is `?server=...`.
 */

/** Local-dev default websocket listener (see docker/nats/nats-server.conf). */
const DEFAULT_SERVER = "ws://localhost:8080";

/** This is the Twenty Questions viewer; its app name is fixed by default. */
const DEFAULT_APP = "twentyquestions";

/** Local-dev default control-service base URL (see freeagent.service.server). */
const DEFAULT_CONTROL = "http://localhost:8000";

/**
 * The dashed REST/entry-point name of this app, distinct from the undashed
 * subject prefix `DEFAULT_APP`. The control service routes by this name.
 */
const DEFAULT_APPLICATION = "twenty-questions";

export interface ViewerConfig {
  /** NATS websocket URL to connect to. */
  server: string;
  /** Public-channel subject to subscribe to; "" when no episode was named. */
  subject: string;
}

/** The public broadcast subject for one episode: `<app>.episode.<id>.public`. */
export function publicSubject(app: string, episode: string): string {
  return `${app}.episode.${episode}.public`;
}

/** Resolve the viewer's configuration from the URL, env, and defaults. */
export function resolveConfig(search: string = window.location.search): ViewerConfig {
  const params = new URLSearchParams(search);
  const env = import.meta.env;

  const server = params.get("server") ?? env.VITE_NATS_WS_URL ?? DEFAULT_SERVER;

  const fullSubject = params.get("subject");
  if (fullSubject) {
    return { server, subject: fullSubject };
  }

  const app = params.get("app") ?? env.VITE_APP_NAME ?? DEFAULT_APP;
  const episode = params.get("episode") ?? env.VITE_EPISODE_ID ?? "";
  const subject = episode ? publicSubject(app, episode) : "";
  return { server, subject };
}

/** Where the control plane lives and how its episodes are watched. */
export interface ControllerConfig {
  /** Control-service REST base URL (e.g. http://localhost:8000). */
  control: string;
  /**
   * The NATS *websocket* URL the launched episodes are watched over. This is the
   * browser-facing listener of the same NATS the service publishes to over its
   * own `nats://` URL — the two are different listeners (ports) on one server,
   * so the websocket URL is configured here rather than derived from the
   * service's NATS URL.
   */
  natsWs: string;
  /** The dashed REST application name the control service routes by. */
  application: string;
}

/** Resolve the control-plane configuration from the URL, env, and defaults. */
export function resolveControllerConfig(
  search: string = window.location.search,
): ControllerConfig {
  const params = new URLSearchParams(search);
  const env = import.meta.env;

  const control = params.get("control") ?? env.VITE_CONTROL_URL ?? DEFAULT_CONTROL;
  const natsWs = params.get("server") ?? env.VITE_NATS_WS_URL ?? DEFAULT_SERVER;
  const application = params.get("application") ?? env.VITE_APPLICATION ?? DEFAULT_APPLICATION;
  return { control, natsWs, application };
}
