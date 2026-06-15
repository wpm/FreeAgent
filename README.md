# FreeAgent

FreeAgent is a Python framework in which multiple LLM agents interact **without turn-taking**. An agent may speak or remain silent at any moment. Everything happens in real time: an agent on a faster machine reacts faster than one on a slower machine, and that asymmetry is part of the simulation, not a bug. Silence is an action; latency is part of the observation.

Two ideas shape everything else:

- **Substrate, not policy.** The library does not solve latency, decide when agents should speak, or enforce rules. It provides the medium — environments, episodes, agents, messages over NATS — in which application-level agents implement their own strategies for these problems.
- **The wire is the log.** Every message sent during an episode lands in one JetStream stream, and the recorder drains that stream into one Parquet file. Stream sequence numbers provide the authoritative total order; what each agent *experienced* is a different (and equally valid) order, reconstructable from the same log. This is also how episodes become multi-agent RL training data.

[DESIGN.md](DESIGN.md) is the authoritative design document; everything here follows it.

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

The shape is always `free-agent [--log-level LEVEL] APP COMMAND ...`. `APP` is an installed application (`twenty-questions` here, discovered through the `freeagent.apps` entry-point group), and each application defines its own commands. The library supplies the shared root, the config loader, the shared `--parquet-log` option, and the launcher; an application supplies its name, environment, and roster in source and calls into them.

## Reading the log

When you pass `--parquet-log PATH`, the launcher spawns the recorder as its own process and drains the episode's full message stream to that Parquet file. Peek at the public-channel conversation:

```python
import json

import pyarrow.parquet as pq

for row in pq.read_table("out/twentyquestions-fake.parquet").to_pylist():
    if row["subject"].endswith(".public"):
        print(f"{row['stream_seq']:>4}  {row['sender']:<6} {json.loads(row['payload'])}")
```

Every row carries `episode_id`, `stream_seq`, `subject`, `sender`, `received_at` (JetStream's server clock), and the raw `payload` as a JSON string. The recorder is part of the library (`freeagent.recorder`); see `PARQUET_SCHEMA` there for the pinned schema.

## Repository layout

A `uv` workspace: the library plus its applications. Applications depend only on `freeagent`, never on each other, and the library is installable standalone.

| Path | What it is |
|------|------------|
| [`packages/freeagent`](packages/freeagent/README.md) | The library: agents, environments, episode lifecycle, LLM infrastructure, the `free-agent` CLI root + launcher, and the episode recorder (`freeagent.recorder`) |
| [`apps/twentyquestions`](apps/twentyquestions/README.md) | The sample application: its own `free-agent twenty-questions` CLI — one Host, several Players, prompts over code |
| [`docker/nats`](docker/nats) | NATS + JetStream container config (infrastructure, assumed running) |
| [`examples/`](examples) | Episode tunables for the sample app, real and fake |
| [`DESIGN.md`](DESIGN.md) | The authoritative design document |

## Tests

```sh
uv run pytest
```

Unit tests need no NATS and no network. Integration tests (including the end-to-end episode test in `tests/`) talk to the local NATS container and skip with a clear message when it is not running.
