/**
 * Viewer configuration: where the episode service lives.
 *
 * The browser no longer speaks NATS (ADR-0003); it talks only to the episode
 * service over HTTP/WebSocket. The base URL is resolved, in order of priority:
 *
 *   1. a persisted setting (localStorage, written by the Settings panel),
 *   2. a `?service=` URL query parameter (a shareable override),
 *   3. the build-time `VITE_SERVICE_URL` env var,
 *   4. the local-dev default `http://localhost:8000` (the mock service).
 *
 * The settings store owns persistence; this module only resolves the *initial*
 * value so a shared link or env override is honoured on first load.
 */

/** Local-dev default: the mock episode service (`python -m freeagent.service.mock`). */
export const DEFAULT_SERVICE_URL = "http://localhost:8000";

/** The localStorage key under which the Settings panel persists the base URL. */
export const SERVICE_URL_KEY = "freeagent.service.url";

/**
 * Resolve the service base URL on startup. A persisted setting wins; then a
 * `?service=` query override; then the Vite env var; then the local default.
 */
export function resolveServiceUrl(search: string = window.location.search): string {
  const stored = readStored();
  if (stored) return stored;

  const params = new URLSearchParams(search);
  const fromUrl = params.get("service");
  if (fromUrl) return fromUrl;

  const env = import.meta.env;
  return env.VITE_SERVICE_URL ?? DEFAULT_SERVICE_URL;
}

/** Read the persisted base URL, tolerating a storage-less environment. */
function readStored(): string | null {
  try {
    return window.localStorage.getItem(SERVICE_URL_KEY);
  } catch {
    return null;
  }
}

/**
 * Derive the WebSocket origin from an HTTP base URL: `http://` -> `ws://`,
 * `https://` -> `wss://`, preserving host and port. Used to build the per-episode
 * feed URL from the same configured base the REST calls use.
 */
export function wsOrigin(httpBase: string): string {
  const url = new URL(httpBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  // Drop any path/query the base might carry; the feed path is appended by the client.
  url.pathname = "";
  url.search = "";
  return url.toString().replace(/\/$/, "");
}
