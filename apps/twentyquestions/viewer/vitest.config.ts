import { svelte } from "@sveltejs/vite-plugin-svelte";
import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";

// Browser-mode tests run the viewer in a real (headless Chromium) browser, so
// the assertions exercise the actual `wsconnect` WebSocket transport and Svelte
// rendering — not a Node/jsdom stand-in. They need a NATS server with the
// websocket listener reachable at VITE_NATS_WS_URL (default ws://localhost:8080).
export default defineConfig({
  plugins: [svelte()],
  test: {
    include: ["src/**/*.browser.test.ts"],
    testTimeout: 30000,
    browser: {
      enabled: true,
      provider: playwright(),
      headless: true,
      instances: [{ browser: "chromium" }],
    },
  },
});
