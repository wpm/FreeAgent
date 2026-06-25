/**
 * The shell's settings store: connection, model, API key, and theme.
 *
 * These are pan-application operator preferences, persisted to `localStorage`
 * so they survive a reload. The store is a Svelte 5 runes class: read its
 * fields reactively and write them through the setters, which persist on every
 * change. The base URL's initial value is resolved by `config.ts` (honouring a
 * `?service=` link or `VITE_SERVICE_URL`) when nothing is persisted yet.
 *
 * The API key is sensitive: it lives only in this browser's storage and is sent
 * *nowhere* except inside a create-episode body as `api_key` (the service threads
 * it into each agent's config). It is never put on a feed URL or a query string.
 */
import { DEFAULT_SERVICE_URL, resolveServiceUrl } from "../config";

export type Theme = "light" | "dark";

const KEYS = {
  serviceUrl: "freeagent.service.url",
  model: "freeagent.model",
  apiKey: "freeagent.apiKey",
  theme: "freeagent.theme",
} as const;

/** The default model, used until the operator picks one in Settings. */
const DEFAULT_MODEL = "claude-haiku-4-5-20251001";

/** Read one persisted string, tolerating a storage-less environment. */
function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** Write (or clear) one persisted string, tolerating a storage-less environment. */
function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Best-effort: a private-mode browser may refuse storage; settings then
    // live only for this session, which is acceptable.
  }
}

/** Operator settings, reactive and persisted. */
export class Settings {
  /** The episode service base URL (REST + the origin the feed derives from). */
  serviceUrl = $state(resolveServiceUrl());
  /** The model string prefilled into create composers. */
  model = $state(read(KEYS.model) ?? DEFAULT_MODEL);
  /** The provider API key; sent only inside create-episode bodies. */
  apiKey = $state(read(KEYS.apiKey) ?? "");
  /** Light or dark theme, applied at the app-shell root. */
  theme = $state<Theme>(read(KEYS.theme) === "dark" ? "dark" : "light");

  /** Update the base URL (falling back to the default when cleared) and persist. */
  setServiceUrl(value: string): void {
    this.serviceUrl = value.trim() || DEFAULT_SERVICE_URL;
    write(KEYS.serviceUrl, this.serviceUrl);
  }

  /** Update the model string and persist. */
  setModel(value: string): void {
    this.model = value;
    write(KEYS.model, value);
  }

  /** Update the API key and persist (local only). */
  setApiKey(value: string): void {
    this.apiKey = value;
    write(KEYS.apiKey, value);
  }

  /** Update the theme and persist. */
  setTheme(value: Theme): void {
    this.theme = value;
    write(KEYS.theme, value);
  }
}

/** The single shared settings instance for the app. */
export const settings = new Settings();
