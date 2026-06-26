import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// The viewer talks only to the episode service (ADR-0003), so there is no NATS
// transport to exercise in a real browser any more. Pure-data tests (the service
// client, the toast store) run in the default node environment; component tests
// opt into jsdom with a `// @vitest-environment jsdom` docblock and render real
// Svelte components.
//
// `resolve.conditions: ["browser"]` makes Vite pick Svelte's *client* build so
// `mount(...)` works under test; without it the SSR build is resolved and
// component mounting throws `lifecycle_function_unavailable`.
export default defineConfig({
  plugins: [svelte()],
  resolve: { conditions: ["browser"] },
  test: {
    include: ["src/**/*.test.ts"],
    passWithNoTests: true,
  },
});
