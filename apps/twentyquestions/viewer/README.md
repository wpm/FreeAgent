# Twenty Questions viewer

The Twenty Questions **viewer** — a TypeScript web app (Vite + Svelte) that
renders an episode as a chat-room transcript. It talks **only to the episode
service** (ADR-0003) over HTTP and a per-episode WebSocket feed — **never NATS**.
The service mediates the wire; the browser consumes a small, normalized feed of
events (a message was appended, the status/seal changed, the connection's
liveness changed). It is the single-language sibling of [`../engine`](../engine),
which holds the Python application, and a member of the root JavaScript workspace
(`pnpm-workspace.yaml` globs `apps/*/viewer`).

## Shape: a generic shell + an app skin

The viewer is factored **generic-left / app-right** (ADR-0003):

- **`src/shell/`** — the pan-application shell, which knows nothing about any one
  game: the episode-service client (`service.ts`), settings (`settings.svelte.ts`
  / `SettingsPanel.svelte` — connection URL, model, API key kept in browser local
  storage, light/dark theme), the generic left-pane episode list
  (`EpisodeList.svelte` — friendly names, live/sealed status, select, new,
  rename, right-click-delete), and a right-pane plugin `registry.ts`.
- **`src/skins/twentyquestions/`** — the app skin registered as the right pane
  for `twenty-questions`: a feed view-model (`feed.svelte.ts`) and the
  presentation (`TwentyQuestions.svelte`) — the transcript, status/budget chips,
  and the start-game composer.

## One view for live and replay

The skin renders the **same view** whether an episode is live or a sealed replay:
it consumes the service's feed, which streams an episode's full history and then
tails live appends, marking the boundary but never letting an observer tell live
from replay (the invariant now lives at the Python → UI boundary, ADR-0003). A
sealed episode is simply the same read with no further appends.

## Running it

The viewer is a **separate host process** that talks to the episode service over
REST and the per-episode feed (ADR-0004) — it is not served by the service and is
not part of the Docker network. So you run two things: the **backend** (the
service + NATS), then the **viewer**.

```sh
# 1. Bring up the backend (REST API on http://localhost:8000):
docker compose -f docker/compose.yml up --build

# 2. From the repo root, install the JS workspace and start the viewer:
pnpm install
pnpm --filter twentyquestions-viewer run dev        # http://localhost:5173
```

Or develop against the canned **mock** (no NATS, no backend) — it implements the
whole contract with canned data:

```sh
uv run python -m freeagent.service.mock        # serves http://localhost:8000
```

Other scripts (from this directory):

```sh
pnpm run dev        # the Vite dev server — http://localhost:5173
pnpm run build      # static production bundle in dist/
pnpm run preview    # serve the built bundle
pnpm run typecheck  # svelte-check (also the schema contract's compile test)
pnpm run test       # vitest (currently no unit tests; passes clean)
```

### Configuring the connection

The service base URL is resolved from, in order: the **Settings** panel (stored
in browser local storage), a `?service=` URL query parameter, the build-time
`VITE_SERVICE_URL`, then the default `http://localhost:8000`. The WebSocket feed
origin is derived from the same base. The model and provider **API key** are set
in Settings and kept in the browser — the key only ever rides inside a
create-episode request body, never on the bus.

## Generated contract types

`src/generated/` holds TypeScript types generated from the FreeAgent JSON
Schema — the framework envelope and the **episode-service contract**
(`packages/freeagent/schemas/`) plus this app's wire payloads (`../schemas/`).
They are committed, but **do not edit them by hand**: they are regenerated from
the Pydantic models that are the single source of truth.

```sh
# From the repo root — regenerates JSON Schema from Pydantic, then types:
pnpm run schemas

# Or just the TypeScript half, from this directory:
pnpm run generate
```

`src/contract.ts` composes the generated interfaces into the resource types and
the `FeedEvent` discriminated union the UI works with (with type guards), and
`src/contract.samples.ts` type-checks sample payloads against them. Importing the
generated types also serves as the contract's compile test — `pnpm run typecheck`
fails if they drift or go missing.
