# Build FreeAgent v1.0

## Mission

Implement version 1.0 of FreeAgent, a Python framework for real-time multi-agent LLM interaction with no turn-taking, plus its three companion applications, in this repository.

**The authoritative specification is `DESIGN.md` in this directory. Read it in full before writing any code. Where this prompt and DESIGN.md conflict, DESIGN.md wins. Do not redesign, extend, or "improve" the architecture — every concept in DESIGN.md was decided deliberately.**

## Technology constraints

- Python ≥ 3.12, fully type-annotated, `asyncio` only — no threads anywhere.
- `uv` workspace exactly as laid out in DESIGN.md's repository layout: the `freeagent` library under `packages/`, the three applications under `apps/`. The library must be installable standalone; applications depend only on `freeagent`, never on each other.
- Runtime dependencies: `nats-py` (with JetStream), `litellm`, `pydantic`, `pyarrow`, `pyyaml`. Dev dependencies: `pytest`, `pytest-asyncio`, `ruff`.
- `docker/nats/`: a docker-compose file running NATS with JetStream enabled. Infrastructure is assumed running; no FreeAgent process manages it.

## Build order

Phase 1 is sequential and everything depends on it. Phases 2a–2d are mutually independent — no shared files, parallelize freely. Phase 3 is sequential, after all of phase 2.

### Phase 1 — `packages/freeagent` (the library)

Implement per DESIGN.md sections: core concepts, NATS subjects, messages, episode lifecycle, agent internals, environment internals, LLM infrastructure.

Invariants that are easy to get wrong — treat as a checklist:

- Handlers (`perceive`, `control`, think handlers) are fast and non-blocking. An LLM call never happens inside a handler; slow work is a spawned asyncio task whose completion re-enters the fold as a think message. State this in docstrings too.
- `act` appends to the outbox; the runtime publishes only after the handler returns. The think queue is NOT behind the outbox — it is written directly.
- Each agent processes one handler at a time over a single merged, locally ordered event stream (external messages + think messages). This is a fold: same state + same event → same new state + same outbox.
- Subject layout exactly as specified, including the `<app>` prefix. Application code never touches raw subjects; `act` addresses sets of agent IDs, default broadcast.
- Message envelope is exactly: `message_id`, `episode_id`, `sender`, `payload`. No timestamps — all timing comes from JetStream server metadata.
- JetStream consumers read each episode's stream from sequence 1.
- Control subject is strictly environment → agents (`start`, `shutdown`). Agents send app-defined management messages to the environment's inbox subject. Environment-originated request/reply uses episode-scoped reply subjects (`<app>.episode.<id>.reply.<req-id>`), never `_INBOX.>`.
- Lifecycle state machine in the base Environment: setup → running → stopping → ended, plus aborted; presence confirmation via request/reply with per-process nonce (duplicate name ⇒ abort); setup timeout, episode timeout, stopping grace period — all timers entering the environment's fold as events.
- Agent IDs: `[A-Za-z0-9_-]+`, validated in the constructor; `env` reserved.
- LLM wrapper over litellm with model resolution: explicit argument → config → `FREEAGENT_MODEL` env var → auto-detect cheapest tier from present provider API keys → clear error listing the env vars checked. Structured outputs via JSON schema + Pydantic validation. Optional per-call telemetry to the agent's log-only subject.
- `LLMAgent` base class: transcript-of-perceived-messages as state, prompt-driven decisions about when/what to say, plus a think-queue base class with delayed/periodic scheduling.
- Include a deterministic, scriptable fake LLM (responses supplied by test code or canned files) selectable via the same model-resolution mechanism, so everything downstream can run without network or keys.

Done when: library imports cleanly, unit tests for the fold/outbox/lifecycle/ID-validation/model-resolution pass with no network, `ruff check` clean.

### Phase 2a — `apps/recorder`

Drains an episode's JetStream stream to one Parquet file, schema per DESIGN.md's logging section (`episode_id`, `stream_seq`, `subject`, `sender`, `received_at` from server metadata, `payload` raw). Runnable concurrently with a live episode or after it. CLI: NATS URL, app name, episode id, output path.

Done when: given a recorded stream, produces a Parquet file whose rows round-trip the messages exactly, raw payloads unparsed.

### Phase 2b — `apps/free-agent` (the runner)

CLI named `free-agent` that reads a `*.yml` episode configuration — application name, episode id (or auto-generated), NATS URL, roster (agent names → importable agent class + per-agent config including prompts and model), environment class + config (timeouts), recorder on/off — validates it (unique names, valid IDs), launches all processes locally, and exits with a meaningful status when the episode ends. Names are assigned top-down from this config; agents never choose their own.

Done when: a single `free-agent run <config>.yml` takes an episode from nothing to `ended`, with all processes cleaned up.

### Phase 2c — `apps/twentyquestions`

Per DESIGN.md's Twenty Questions section. Several `Player` agents and one `Host`, all `LLMAgent` subclasses defined almost entirely by prompts — write as little code as possible. Host: secret from CLI/config or random from a canned list; LLM classifies each utterance (question / guess / deliberation) and judges guesses and answers questions; the question count is incremented in code on a question verdict. Players: deliberate openly on the public channel to choose the next question, hear the Host's game-over announcement, say goodbye, and leave — all in the prompt. Host signals the environment's inbox when the game is over; the minimal environment subclass reacts to exactly that one message by initiating shutdown. Goodbyes happen inside the stopping grace period.

Ship an example `twentyquestions.yml` for the runner (3 players, 1 host).

Done when: an episode runs end-to-end with the fake LLM (scripted exchange) and, when a real key is present, with a real model.

### Phase 2d — Collatz test application

The library's no-LLM test application, in the `freeagent` package's test suite. Agents exchange numbers and apply Collatz steps; the episode ends when 1 is reached. Deterministic, no network beyond NATS, exercises the full stack: lifecycle (including presence and shutdown), messaging, the fold, the outbox, think queue, recorder-compatible logging.

Done when: it runs as a pytest integration test against a local NATS container and asserts the full expected message sequence.

### Phase 3 — integration, verification, docs

- End-to-end test: NATS up via docker compose → `free-agent run` Twenty Questions with the fake LLM → assert episode reaches `ended` → run the recorder → open the Parquet file and assert expected subjects, senders, and ordering by `stream_seq`.
- Root `README.md`: what FreeAgent is (lead with no-turn-taking), quickstart (docker compose, export an API key, `free-agent run apps/twentyquestions/examples/twentyquestions.yml`), pointer to DESIGN.md. Brief READMEs per package/app.
- Final pass: `uv sync` from scratch works; `ruff check .` clean; `pytest` green — unit tests with no NATS and no network, integration tests skipped with a clear message when NATS isn't running.

## Acceptance criteria (the definition of done)

1. `uv sync && ruff check . && pytest` succeeds on a clean checkout with Docker available and no LLM API keys set.
2. `uv pip install ./packages/freeagent` works in a fresh venv — the library stands alone.
3. With NATS up: `free-agent run apps/twentyquestions/examples/twentyquestions-fake.yml` (fake LLM) completes an episode to `ended` and the recorder writes a valid Parquet file.
4. With a real API key: `free-agent run apps/twentyquestions/examples/twentyquestions.yml` plays a watchable, complete game of Twenty Questions.

## Non-goals — do not build

No recruiter beyond a stub module with a docstring. No human/terminal adapter. No batch/concurrent-episode runner (but never assume only one episode exists at a time — subject isolation must hold). No web UI. No log parsers beyond the raw recorder. No NATS management (clustering, auth, provisioning). No retries/robustness layers beyond what nats-py and litellm provide. No turn-taking abstractions of any kind — if you find yourself writing a scheduler that decides whose turn it is, stop and reread DESIGN.md.
