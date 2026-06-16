/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Default NATS websocket URL (e.g. ws://localhost:8080). */
  readonly VITE_NATS_WS_URL?: string;
  /** Default application name used to build the public subject. */
  readonly VITE_APP_NAME?: string;
  /** Default episode id, if baked in at build time. */
  readonly VITE_EPISODE_ID?: string;
  /** Default control-service base URL (e.g. http://localhost:8000). */
  readonly VITE_CONTROL_URL?: string;
  /** Default dashed REST application name (e.g. twenty-questions). */
  readonly VITE_APPLICATION?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
