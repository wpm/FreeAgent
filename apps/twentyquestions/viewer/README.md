# Twenty Questions viewer

The Twenty Questions **viewer** — a TypeScript web app (Vite + Svelte) that
renders episodes as chat-room live transcripts. It is also a **controller**: it
talks to the FreeAgent control service over REST to configure and launch
episodes, then watches several at once. Two planes, kept strictly apart:

- **Control (REST).** Every mutation — start a live episode, replay a recording,
  stop one — goes over REST to the control service (`src/control.ts`). The
  configurable surface is the server/NATS URLs, the Host's secret, a model
  override, and the logging file; the **player count is fixed** (the roster is
  source-defined for v1).
- **Observe (NATS).** Transcripts are read **entirely over NATS**: the viewer
  subscribes to each episode's public channel over websockets and **never
  publishes**. The control plane does the launching; the NATS side stays
  read-only, exactly as the original single-episode viewer was. A subject named
  by hand (or by a shared URL) can still be attached read-only with no control
  service at all.

The transport is the maintained [nats.js](https://github.com/nats-io/nats.js) v3
browser client ([`@nats-io/nats-core`](https://www.npmjs.com/package/@nats-io/nats-core)'s
`wsconnect` plus [`@nats-io/jetstream`](https://www.npmjs.com/package/@nats-io/jetstream)).
It is the single-language sibling of
[`../engine`](../engine), which holds the Python application, and a member of the
root JavaScript workspace (`pnpm-workspace.yaml` globs `apps/*/viewer`). Keeping
the TypeScript tree out of `engine/` is deliberate: it stops the Python tooling
(ruff, pytest, packaging) from globbing a foreign subtree.

## One code path for live and replay

The viewer cannot tell a live episode from a replay, and has exactly one code
path for both. It reads the episode's JetStream stream from sequence
1 (an ordered consumer with `DeliverPolicy.ALL`), exactly as the engine does, so
the whole transcript renders in `stream_seq` order whether it joined at the
start, mid-episode, or after the fact. A replay re-publishes a recorded episode
onto NATS with byte-identical subjects, so the same subscription just works.

## Running it

The viewer needs a NATS server with the websocket listener enabled (see
[`docker/nats`](../../../docker/nats)); the default is `ws://localhost:8080`.

```sh
# From the repo root (installs the whole JS workspace):
pnpm install

# From this directory:
pnpm run dev        # Vite dev server (http://localhost:5173)
pnpm run build      # static production bundle in dist/
pnpm run preview    # serve the built bundle
pnpm run typecheck  # svelte-check (also the schema contract's compile test)
pnpm run test       # browser-mode test (needs NATS + a Playwright browser)
```

### Browser test

`src/viewer.browser.test.ts` is a [Vitest browser-mode](https://vitest.dev/guide/browser/)
test: it loads the viewer in a real headless Chromium, seeds a few public-channel
frames onto a live NATS exactly as an episode (or replay) would, and asserts the
chat transcript — including the Host's outcome line — renders and the connection
reports itself live. Unlike a Node/jsdom test it exercises the genuine
`wsconnect` WebSocket transport and Svelte rendering, which is the only way to
cover the in-browser path. It needs a NATS server with the websocket listener
(`ws://localhost:8080`, or `VITE_NATS_WS_URL`) and the Playwright browser
(`pnpm exec playwright install chromium`). CI runs it in the `viewers` job.

### Controller: configure, launch, watch

With the control service running the app drives episodes itself, over REST — no
CLI in the loop:

```sh
# Terminal 1 — start NATS (client 4222, websocket 8080):
docker compose -f docker/nats/docker-compose.yml up -d

# Terminal 2 — start the control service (REST on :8000, talks NATS on :4222):
python -m freeagent.service.server

# Terminal 3 — the controller app:
pnpm --filter twentyquestions-viewer run dev   # http://localhost:5173
```

In the **Configuration** row, point **Control service** at the REST URL
(`http://localhost:8000`), **NATS websocket** at the browser-facing listener
(`ws://localhost:8080`), and **Application** at the dashed REST name
(`twenty-questions`). Then:

- **Start episode** — optionally set the Host's **secret**, a **model** override
  (applied to the Host and every Player), a **logging file** to record to, and a
  fixed **episode id**; the service launches it live and its transcript appears.
  Press it again to start a **second concurrently**: each episode gets its own
  panel.
- **Stop** (on a panel) — gracefully stops that episode over REST; its finished
  transcript stays on screen. **Close** just detaches the transcript.
- **Replay** — give the path to a recorded Parquet log; the service replays it
  onto NATS and the viewer watches it through the **same transcript path** as a
  live episode (ADR-0001).
- **Refresh** — re-reads every launched episode's status from the service.

The control service binds to loopback (no auth, local testbed) and is configured
to allow the Vite dev server's origin; its NATS URL and the viewer's websocket
URL are two listeners on the same NATS server (different ports), so the websocket
URL is configured separately rather than derived. Build-time defaults:
`VITE_CONTROL_URL`, `VITE_NATS_WS_URL`, `VITE_APPLICATION`. The same fields are
URL query parameters (`?control=…&server=…&application=…`).

### Observe a subject directly (read-only, no control service)

The original read-only path is unchanged: connection settings come from URL query
parameters (the viewer is shareable by URL), so a link encodes exactly what to
watch. The channel can be named two ways:

- **app name + episode id** — `?app=twentyquestions&episode=<id>`
- **a full subject** — `?subject=twentyquestions.episode.<id>.public`

and the server is `?server=ws://localhost:8080`. A full `subject` wins when both
are given. The same subject is editable in the UI's **Observe subject** bar.
Build-time defaults can be set with `VITE_NATS_WS_URL`, `VITE_APP_NAME`, and
`VITE_EPISODE_ID`. When the URL already names an episode the viewer attaches on
load; otherwise fill in the bar and press **Connect**. The per-panel status
indicator shows connecting / waiting-for-episode / live / reconnecting, and the
client auto-reconnects, so a panel may be opened before the episode starts and
will light up when it does. This drives an episode launched any other way — the
CLI, or a replay onto a separate NATS:

```sh
# Run a live episode by hand (give it a known id so you can link to it):
free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions-fake.yml
#   then attach the viewer with ?episode=<id-printed-by-run>

# Or replay a recording onto a separate, local NATS and point the viewer there:
free-agent replay out/twentyquestions-fake.parquet --nats-url nats://localhost:4223
#   ?server=ws://localhost:8081&episode=<id>
```

The transcript shows the public-channel conversation — Player deliberation, the
questions and the Host's answers — ending with the Host's in-world game-over
announcement, the same whether live or replayed.

## Generated message types

`src/generated/` holds TypeScript types generated from the FreeAgent JSON
Schema — the framework envelope (`packages/freeagent/schemas/`) and this app's
wire payloads (`../schemas/`). They are committed, but **do not edit them by
hand**: they are regenerated from the Pydantic models that are the single
source of truth (see the repo-root README, "Message schemas").

```sh
# From the repo root — regenerates JSON Schema from Pydantic, then types:
pnpm run schemas

# Or just the TypeScript half, from this directory:
pnpm run generate
```

`src/messages.ts` builds the viewer-facing message types on the generated
output (e.g. narrowing an `Envelope` to the Host's `GameOver` signal), and
`src/viewer.svelte.ts` decodes every wire frame through them. Importing the
generated types also serves as the contract's compile test — `pnpm run
typecheck` fails if they drift or go missing.
