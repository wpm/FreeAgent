# Collatz viewer

A static browser page that completes the Collatz application: launch an episode, watch each
agent's chain grow step by step, and see agents and the episode finish. It speaks only REST to
`freeagent-api` — viewers never touch NATS — and derives all game state client-side
from the raw data-plane feed, narrowing messages on their `message_type` tag with the generated
types in [`../schema`](../schema/README.md). Deliberately minimal: it exists to prove the
engine → NATS → API → viewer pipeline, not to be a product.

## Layout

| Path | What it is |
| --- | --- |
| `index.html`, `style.css` | The page shell; loads the compiled `dist/main.js`. |
| `src/api.ts` | REST client and hand-written mirrors of the API's control-plane response models. |
| `src/state.ts` | Pure view-state derivation: data-plane records → per-agent chains. |
| `src/main.ts` | DOM wiring: launch form, episode table, chain display, polling. |
| `src/state.test.ts` | Node test-runner tests for the pure logic. |

## Running

Build the viewer, then serve this directory over HTTP (any static server works) with a
`nats-server` and the API running:

```sh
cd apps/collatz/viewer
npm install        # first time only
npm run build      # tsc: src/ -> dist/, compiled against ../schema/collatz.d.ts
npm run serve      # http://localhost:8080, via python3 -m http.server

# elsewhere: nats-server, then the API
uv run freeagent-api
```

Open <http://localhost:8080>, launch an episode (one starting number per agent), and watch each
agent's chain extend until it reaches 1 — the agent is stopped by the environment (`StopAgent`)
and shows **done**; when every chain finishes, the episode's status turns **complete**
(`EpisodeComplete`).

## Tests

```sh
npm test           # tsc + node --test against the compiled output
```

CI builds the viewer against freshly regenerated schema types (see the `schema` job), so a green
build means the viewer compiles against what the engine's pydantic models actually emit.
