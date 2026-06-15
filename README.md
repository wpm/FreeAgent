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
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml
```

Run a **real** game. It requires a provider key — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`; the cheap tier of whichever provider is detected is used automatically, or set `FREEAGENT_MODEL` to any litellm model string:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
uv run free-agent twenty-questions run examples/twentyquestions.yml
```

Either way, one command launches the environment and the agents as separate processes, runs the episode to `ended`, and prints a one-line summary. To also capture the episode's full message log to a Parquet file, add `--parquet-log PATH` (the path must not already exist — the recorder never overwrites a finished log):

```sh
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml \
  --parquet-log out/twentyquestions-fake.parquet
```

The shape is always `free-agent [--log-level LEVEL] APP COMMAND ...`. `APP` is an installed application (`twenty-questions` here), and each application defines its own commands. The library supplies the shared pieces — the command root, the launcher, the recording option — and applications plug into them.

## Reading the log

When you pass `--parquet-log PATH`, the episode's full message log is written to a Parquet file: one row per message, carrying the episode id, an authoritative sequence number, the channel, the sender, a server timestamp, and the message payload. It is an ordinary columnar table — read it with any Parquet tool to replay a game, analyze behavior, or build training data.

## Replaying an episode

A recorded episode replays as **NATS playback, not log reading** (see [ADR-0001](docs/decision-history/0001-gui-viewers-over-nats.md)): the replayer re-publishes a Parquet log's messages onto a NATS server using byte-identical subjects, so a viewer subscribes the same way whether it is watching a live episode or a replay — one code path, no idea which it is seeing.

```sh
# Point a second, local nats-server at a different port so replay never mixes
# with live traffic, then replay onto it.
free-agent replay out/twentyquestions-fake.parquet --nats-url nats://localhost:4223
```

`replay` is a top-level command, a sibling of the per-application sub-commands, because the Parquet log is uniform across every application — one tool replays any app's episode and never needs app-specific code. Messages are published on their original subjects in `stream_seq` order; inter-message timing is preserved by default, scaled by `--speed` (e.g. `--speed 2.0`), or dropped entirely with `--as-fast-as-possible`. Start is the command; stop is Ctrl-C. Pause and seek live on the library's `Replayer` class for an embedding GUI to drive.

## Project structure

A `uv` workspace containing the **library** and its **applications**. The library is the substrate and is usable on its own; each application depends only on the library, never on another application. The repository ships one sample application, Twenty Questions, plus the NATS infrastructure config and ready-to-run example episodes. Each component has its own README, and [the design document](docs/DESIGN.md) is authoritative.

Each application is split into two self-contained, single-language siblings (see [ADR-0001](docs/decision-history/0001-gui-viewers-over-nats.md)): `apps/<app>/engine/` is the Python application (environment, roster, prompts, CLI, its own `pyproject.toml`), and `apps/<app>/viewer/` is the TypeScript web GUI that observes an episode over NATS (its own `package.json`). Keeping the two in separate directories stops the toolchains from colliding: the `uv` workspace globs `apps/*/engine`, while a root JavaScript workspace (`pnpm-workspace.yaml`) globs `apps/*/viewer` so every viewer shares one lockfile and toolchain.

## Tests

```sh
uv run pytest
```

Unit tests need no NATS and no network. Integration tests (including the end-to-end episode test) talk to the local NATS container and skip with a clear message when it is not running.
