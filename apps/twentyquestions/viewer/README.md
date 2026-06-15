# Twenty Questions viewer

The Twenty Questions **viewer** — a TypeScript web app (Vite + Svelte) that
renders an episode as a chat-room live transcript. It observes the episode
**entirely over NATS**, subscribing to the public channel over websockets and
never publishing: human interaction is a non-goal for now. The
transport is the maintained [nats.js](https://github.com/nats-io/nats.js) v3
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

### Configuring the connection

Connection settings come from URL query parameters (the viewer is read-only and
shareable by URL), so a link encodes exactly what to watch. The channel can be
named two ways:

- **app name + episode id** — `?app=twentyquestions&episode=<id>`
- **a full subject** — `?subject=twentyquestions.episode.<id>.public`

and the server is `?server=ws://localhost:8080`. A full `subject` wins when both
are given. The same fields are editable in the UI's connect bar. Build-time
defaults can be set with `VITE_NATS_WS_URL`, `VITE_APP_NAME`, and
`VITE_EPISODE_ID`. When the URL already names an episode the viewer connects on
load; otherwise fill in the connect bar and press **Connect**. The status
indicator shows connecting / waiting-for-episode / live / reconnecting, and the
client auto-reconnects, so the viewer may be opened before the episode starts and
will light up when it does.

### Against a live episode

```sh
# Terminal 1 — start NATS (client 4222, websocket 8080):
docker compose -f docker/nats/docker-compose.yml up -d

# Terminal 2 — run a live episode (give it a known id so you can link to it):
free-agent twenty-questions run examples/twentyquestions-fake.yml

# Terminal 3 — the viewer; open the printed URL with the episode id:
pnpm --filter twentyquestions-viewer run dev
#   http://localhost:5173/?episode=<id-printed-by-run>
```

The run prints `episode_id=<id>` on exit; set `episode_id:` in the YAML to fix it
ahead of time. The transcript shows the public-channel conversation — Player
deliberation, the questions and the Host's answers — ending with the Host's
in-world game-over announcement.

### Against a replay

```sh
# Record an episode:
free-agent twenty-questions run examples/twentyquestions-fake.yml \
  --parquet-log out/twentyquestions-fake.parquet

# Replay it onto a separate, local NATS and point the viewer there:
free-agent replay out/twentyquestions-fake.parquet --nats-url nats://localhost:4223
#   open the viewer with ?server=ws://localhost:8081&episode=<id>
```

The replay shows the same transcript as the live run, through the same code path.

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
