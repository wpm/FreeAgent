# FreeAgent

FreeAgent is a Python framework in which multiple LLM agents interact **without turn-taking**. An agent may speak or remain silent at any moment. Everything happens in real time: an agent on a faster machine reacts faster than one on a slower machine, and that asymmetry is part of the simulation, not a bug. Silence is an action; latency is part of the observation.

Two ideas shape everything else:

- **Substrate, not policy.** The library does not solve latency, decide when agents should speak, or enforce rules. It provides the medium — environments, episodes, agents, messages over NATS — in which application-level agents implement their own strategies for these problems.
- **The wire is the log.** Every message sent during an episode lands in one JetStream stream, and the recorder drains that stream into one Parquet file. Stream sequence numbers provide the authoritative total order; what each agent *experienced* is a different (and equally valid) order, reconstructable from the same log. This is also how episodes become multi-agent RL training data.

[DESIGN.md](DESIGN.md) is the authoritative design document; everything here follows it.

## Quickstart

Prerequisites: Python ≥ 3.12, [uv](https://docs.astral.sh/uv/), and Docker.

```sh
docker compose -f docker/nats/docker-compose.yml up -d   # NATS + JetStream
uv sync
```

Run a complete episode with **no API key** — the deterministic fake LLM plays a scripted game of Twenty Questions:

```sh
uv run free-agent run examples/twentyquestions-fake.yml
```

Run a **real** game (requires a provider key; the cheap tier of whichever provider is detected is used automatically, or set `FREEAGENT_MODEL` to any litellm model string):

```sh
export ANTHROPIC_API_KEY=...   # or OPENAI_API_KEY / GEMINI_API_KEY, or FREEAGENT_MODEL
uv run free-agent run examples/twentyquestions.yml
```

Either way, one command launches the environment, the agents, and the recorder as separate processes, runs the episode to `ended`, and prints a one-line summary.

## Reading the log

The recorder runs automatically (the `recorder:` block in the example configs) and writes the episode's full message log to Parquet — for the fake game, `out/twentyquestions-fake.parquet`. Peek at the public-channel conversation:

```python
import json

import pyarrow.parquet as pq

for row in pq.read_table("out/twentyquestions-fake.parquet").to_pylist():
    if row["subject"].endswith(".public"):
        print(f"{row['stream_seq']:>4}  {row['sender']:<6} {json.loads(row['payload'])}")
```

Every row carries `episode_id`, `stream_seq`, `subject`, `sender`, `received_at` (JetStream's server clock), and the raw `payload` as a JSON string. See [apps/recorder](apps/recorder/README.md) for the schema.

## Repository layout

A `uv` workspace: the library plus three applications. Applications depend only on `freeagent`, never on each other, and the library is installable standalone.

| Path | What it is |
|------|------------|
| [`packages/freeagent`](packages/freeagent/README.md) | The library: agents, environments, episode lifecycle, LLM infrastructure |
| [`apps/free-agent`](apps/free-agent/README.md) | The `free-agent` runner CLI: one episode from one YAML file |
| [`apps/recorder`](apps/recorder/README.md) | `freeagent-recorder`: drains an episode's JetStream stream to Parquet |
| [`apps/twentyquestions`](apps/twentyquestions/README.md) | The sample application: one Host, several Players, prompts over code |
| [`docker/nats`](docker/nats) | NATS + JetStream container config (infrastructure, assumed running) |
| [`examples/`](examples) | Episode configurations for the runner, real and fake |
| [`DESIGN.md`](DESIGN.md) | The authoritative design document |

## Tests

```sh
uv run pytest
```

Unit tests need no NATS and no network. Integration tests (including the end-to-end episode test in `tests/`) talk to the local NATS container and skip with a clear message when it is not running.
