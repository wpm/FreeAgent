import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The viewer is a plain static SPA: a Svelte app that talks to the episode
// service over HTTP + a per-episode WebSocket feed at runtime (ADR-0003), never
// NATS. It runs as its own host process (ADR-0004) and calls the service
// cross-origin; `vite build` emits a self-contained bundle served however the
// operator chooses (e.g. `pnpm dev`, a static host).
export default defineConfig({
  plugins: [svelte()],
  server: { port: 5173 },
});
