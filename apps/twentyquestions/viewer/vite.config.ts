import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The viewer is a plain static SPA: a Svelte app that talks to the episode
// service over HTTP + a per-episode WebSocket feed at runtime (ADR-0003), never
// NATS. `vite build` emits a self-contained bundle the service serves from its
// own origin.
export default defineConfig({
  plugins: [svelte()],
  server: { port: 5173 },
});
