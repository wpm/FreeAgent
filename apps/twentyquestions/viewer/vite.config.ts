import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The viewer is a plain static SPA: a Svelte app that talks to NATS over
// websockets at runtime (ADR-0001). `vite build` emits a self-contained bundle
// that can be served from anywhere or shared by URL.
export default defineConfig({
  plugins: [svelte()],
  server: { port: 5173 },
});
