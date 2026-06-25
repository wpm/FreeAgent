import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// The viewer talks only to the episode service (ADR-0003), so there is no NATS
// transport to exercise in a real browser any more. Unit tests (if any) run in
// the default node environment; `passWithNoTests` keeps `pnpm test` green while
// the suite is empty.
export default defineConfig({
  plugins: [svelte()],
  test: {
    include: ["src/**/*.test.ts"],
    passWithNoTests: true,
  },
});
