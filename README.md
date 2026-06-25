# Free Agent

Free Agent is a Python framework in which multiple LLM agents interact **without turn-taking**. An agent may speak or remain silent at any moment. Everything happens in real time: an agent on a faster machine reacts faster than one on a slower machine, and that asymmetry is part of the simulation, not a bug. Silence is an action; latency is part of the observation.

Two ideas shape everything else:

- **Substrate, not policy.** The library does not solve latency, decide when agents should speak, or enforce rules. It provides the medium — environments, episodes, agents, messages over NATS — in which application-level agents implement their own strategies for these problems.
- **The wire is the log.** Every message sent during an episode lands in one JetStream stream, and the recorder drains that stream into one Parquet file. Stream sequence numbers provide the authoritative total order; what each agent *experienced* is a different (and equally valid) order, reconstructable from the same log. This is also how episodes become multi-agent RL training data.

[DESIGN.md](docs/DESIGN.md) is the authoritative design document; everything here follows it.

## Quickstart

Prerequisites: Python ≥ 3.12, [uv](https://docs.astral.sh/uv/), and Docker.

Start NATS with JetStream, then sync the workspace:

```sh
docker compose -f docker/nats/docker-compose.yml up -d
uv sync
```

Run a complete episode with **no API key** — the deterministic fake LLM plays a scripted game of Twenty Questions:

```sh
uv run free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions-fake.yml
```

Run a **real** game. It requires a provider key — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`; the cheap tier of whichever provider is detected is used automatically, or set `FREEAGENT_MODEL` to any litellm model string:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
uv run free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions.yml
```

Either way, one command launches the environment and the agents as separate processes, runs the episode to `ended`, and prints a one-line summary. To also capture the episode's full message log to a Parquet file, add `--parquet-log PATH` (the path must not already exist — the recorder never overwrites a finished log):

```sh
uv run free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions-fake.yml \
  --parquet-log out/twentyquestions-fake.parquet
```

The shape is always `free-agent [--log-level LEVEL] APP COMMAND ...`. `APP` is an installed application (`twenty-questions` here), and each application defines its own commands. The library supplies the shared pieces — the command root, the launcher, the recording option — and applications plug into them.

## Reading the log

When you pass `--parquet-log PATH`, the episode's full message log is written to a Parquet file: one row per message, carrying the episode id, an authoritative sequence number, the channel, the sender, a server timestamp, and the message payload. It is an ordinary columnar table — read it with any Parquet tool to replay a game, analyze behavior, or build training data.

## Replaying an episode

A recorded episode replays as **NATS playback, not log reading**: the replayer re-publishes a Parquet log's messages onto a NATS server using byte-identical subjects, so a viewer subscribes the same way whether it is watching a live episode or a replay — one code path, no idea which it is seeing.

```sh
# Point a second, local nats-server at a different port so replay never mixes
# with live traffic, then replay onto it.
free-agent replay out/twentyquestions-fake.parquet --nats-url nats://localhost:4223
```

`replay` is a top-level command, a sibling of the per-application sub-commands, because the Parquet log is uniform across every application — one tool replays any app's episode and never needs app-specific code. Messages are published on their original subjects in `stream_seq` order; inter-message timing is preserved by default, scaled by `--speed` (e.g. `--speed 2.0`), or dropped entirely with `--as-fast-as-possible`. Start is the command; stop is Ctrl-C. Pause and seek live on the library's `Replayer` class for an embedding GUI to drive.

## Episode service (the backend)

The CLI launches one episode and blocks. The **episode service** is the long-running alternative: a small, **app-independent** REST API (plus a per-episode WebSocket feed) that creates, lists, observes, renames, deletes, and stops episodes on demand, driven over HTTP. It serves **no UI** and bundles no application ([ADR-0004](docs/decision-history/0004-app-independent-service.md)) — a UI is a separate process that calls it cross-origin (see [the viewer](#watching-a-game-in-the-browser)). An episode is a durable, named, **JetStream-resident** object ([ADR-0003](docs/decision-history/0003-the-atemporal-episode.md)): the service's source of truth is JetStream, not in-memory state, so a restart loses nothing.

The simplest way to run the backend is the **two-service Docker network** — NATS (internal) plus the service (the only published port). See [`docker/README.md`](docker/README.md):

```sh
docker compose -f docker/compose.yml up --build   # REST API on http://localhost:8000
```

Or run the service directly:

```sh
free-agent serve        # REST + feed on 127.0.0.1:8000
```

It binds **loopback with no auth** by default, consistent with the local NATS testbed (see [SECURITY.md](SECURITY.md)). The REST resources live under `/freeagent/<application>/<episode>`:

| Method & path | Does |
| --- | --- |
| `POST   /freeagent/{application}/episodes` | create a **live** episode |
| `GET    /freeagent/episodes` | list every episode (from the durable record) |
| `GET    /freeagent/{application}/episodes/{episode}` | one episode's status |
| `PATCH  /freeagent/{application}/episodes/{episode}` | rename (friendly name) |
| `DELETE /freeagent/{application}/episodes/{episode}` | delete (remove the stream) |
| `POST   /freeagent/{application}/episodes/{episode}/stop` | graceful operator-abort |
| `POST   /freeagent/{application}/episodes/{episode}/export` | drain a sealed episode to Parquet |
| `POST   /freeagent/import` | play a Parquet log into a fresh episode |
| `WS     /freeagent/{application}/episodes/{episode}/feed` | the normalized per-episode feed |

The create body carries the settable config — an optional `episode_id` and `name`, a `model` and `api_key` threaded into agent config, and per-component `config` blocks (the Host's `secret`, timeouts) — plus an optional `parquet_log` to record the run.

```sh
# Create a live episode, then watch the service's view of it.
curl -s -XPOST localhost:8000/freeagent/twenty-questions/episodes \
  -H 'content-type: application/json' \
  -d '{"name":"otters","agents":{"host":{"config":{"secret":"otter"}}}}'
curl -s localhost:8000/freeagent/episodes
```

The browser **never speaks NATS** (ADR-0003): a UI talks only to the service, which relays a small normalized feed (a message was appended, the status/seal changed, the connection's liveness changed). For UI development without NATS, a canned **mock** implements the whole contract: `uv run python -m freeagent.service.mock`.

### Watching a game in the browser

The Twenty Questions **viewer** is a separate web app (Vite + Svelte) that you run on the host; it is **not** part of the backend image or the Docker network ([ADR-0004](docs/decision-history/0004-app-independent-service.md)). With the backend up (above), start the viewer and point it at the service:

```sh
pnpm install                                  # once, to fetch the JS toolchain
pnpm --filter twentyquestions-viewer run dev  # http://localhost:5173
```

By default it talks to the service at `http://localhost:8000`; override that in the viewer's **Settings** panel (or with `?service=…`). See [`apps/twentyquestions/viewer/README.md`](apps/twentyquestions/viewer/README.md).

## Project structure

A `uv` workspace containing the **library** and its **applications**. The library is the substrate and is usable on its own; each application depends only on the library, never on another application. The repository ships one sample application, Twenty Questions, plus the NATS infrastructure config and ready-to-run example episodes. Each component has its own README, and [the design document](docs/DESIGN.md) is authoritative.

Each application is split into two self-contained, single-language siblings: `apps/<app>/engine/` is the Python application (environment, roster, prompts, CLI, its own `pyproject.toml`), and `apps/<app>/viewer/` is the TypeScript web GUI that observes an episode through the episode service's feed — never NATS (ADR-0003) — with its own `package.json`. Keeping the two in separate directories stops the toolchains from colliding: the `uv` workspace globs `apps/*/engine`, while a root JavaScript workspace (`pnpm-workspace.yaml`) globs `apps/*/viewer` so every viewer shares one lockfile and toolchain.

## Message schemas (single source of truth)

The NATS message types are defined once, in Python (Pydantic), and the
TypeScript viewers consume generated types — no hand-maintained duplication.
The pipeline has two steps, wired behind one command:

1. **JSON Schema from Pydantic.** `python -m freeagent.schema` exports the
   framework envelope to `packages/freeagent/schemas/`, and each app exports
   its wire payloads (`python -m twentyquestions.schema` →
   `apps/twentyquestions/schemas/`). Schemas live beside the package that owns
   the model, so a future repo split inherits a clean contract. Both the
   `.schema.json` artifacts are committed.
2. **TypeScript from JSON Schema.** `json-schema-to-typescript` emits `.d.ts`
   types into each viewer (`apps/twentyquestions/viewer/src/generated/`), which
   the viewer imports.

Regenerate both from the repo root with a single command (Python via `uv`,
TypeScript via the `pnpm` workspace):

```sh
pnpm install      # once, to fetch the JS toolchain
pnpm run schemas  # JSON Schema from Pydantic, then TypeScript from JSON Schema
```

After changing a wire model, run `pnpm run schemas` and commit the regenerated
artifacts. CI fails if the committed schemas or generated types are stale, and
`pnpm run typecheck` confirms the viewers still compile against them. Guard
tests (`test_schema.py`) assert the committed JSON Schema matches the models.

## Tests

```sh
uv run pytest
```

Unit tests need no NATS and no network. Integration tests (including the end-to-end episode test) talk to the local NATS container and skip with a clear message when it is not running.
