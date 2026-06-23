/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Default NATS websocket URL (e.g. ws://localhost:8080). */
  readonly VITE_NATS_WS_URL?: string;
  /** Default application name used to build the public subject. */
  readonly VITE_APP_NAME?: string;
  /** Default episode id, if baked in at build time. */
  readonly VITE_EPISODE_ID?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
