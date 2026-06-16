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

## Control service

The CLI launches one episode and blocks. The **control service** is the long-running alternative: a small REST API that owns a set of running episodes and launches, lists, observes, and stops them on demand — the same supervised launch as `run`, driven over HTTP instead of from a terminal.

```sh
free-agent serve            # binds 127.0.0.1:8000 by default
```

It binds to **loopback with no auth**, consistent with the local NATS testbed (see [SECURITY.md](SECURITY.md)), and verifies NATS is reachable on startup and on every create — a down server yields a clear `503`, never a hang. The REST resources live under `/freeagent/<application>/<episode>` and map, at the API boundary, onto the existing NATS subjects — the service introduces no new wire subjects:

| Method & path | Does |
| --- | --- |
| `POST /freeagent/{application}/episodes` | create a **live** episode (or **replay** a recorded log) |
| `GET  /freeagent/episodes` | list every running episode |
| `GET  /freeagent/{application}/episodes/{episode}` | one episode's status |
| `POST /freeagent/{application}/episodes/{episode}/stop` | graceful operator-abort |
| `POST /freeagent/teardown` | bring everything down |

The create body carries the same settable config the YAML does — `nats_url`, an optional `episode_id`, and per-component `config` blocks (the Host's `secret`, an agent's `model`, timeouts) — plus a `mode` of `live` or `replay` and, for a live run, an optional `parquet_log` to record it. A created episode is observable over NATS **exactly like a `run`-launched one**: a viewer subscribes by mapping the REST id to the response's `subject_root`.

```sh
# Create a live episode, then watch the service's view of it.
curl -s -XPOST localhost:8000/freeagent/twenty-questions/episodes \
  -H 'content-type: application/json' \
  -d '{"mode":"live","agents":{"host":{"config":{"secret":"otter"}}}}'
curl -s localhost:8000/freeagent/episodes
```

The service is **API-only**: it serves no static bundle. The browser viewer is served by its own Vite dev server and calls the API cross-origin, so CORS is configured for that origin (`http://localhost:5173` by default). Serving the built viewer from one origin is a deferred convenience. Two v1 limits worth knowing: the registry is **in memory** (a service restart forgets the episodes it was tracking — and tears their processes down with it, so nothing is orphaned), and the service does **not** manage the NATS Docker container (start it yourself, as above).

## Project structure

A `uv` workspace containing the **library** and its **applications**. The library is the substrate and is usable on its own; each application depends only on the library, never on another application. The repository ships one sample application, Twenty Questions, plus the NATS infrastructure config and ready-to-run example episodes. Each component has its own README, and [the design document](docs/DESIGN.md) is authoritative.

Each application is split into two self-contained, single-language siblings: `apps/<app>/engine/` is the Python application (environment, roster, prompts, CLI, its own `pyproject.toml`), and `apps/<app>/viewer/` is the TypeScript web GUI that observes an episode over NATS (its own `package.json`). Keeping the two in separate directories stops the toolchains from colliding: the `uv` workspace globs `apps/*/engine`, while a root JavaScript workspace (`pnpm-workspace.yaml`) globs `apps/*/viewer` so every viewer shares one lockfile and toolchain.

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
