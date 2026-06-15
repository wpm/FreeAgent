/**
 * Viewer configuration: where to connect and which channel to watch.
 *
 * The viewer is read-only and shareable by URL (ADR-0001), so its configuration
 * comes from URL query parameters, falling back to build-time Vite env vars and
 * then to local-dev defaults. The episode's channel can be named two ways, per
 * the issue:
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
