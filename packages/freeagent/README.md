# freeagent

The FreeAgent library: real-time multi-agent LLM interaction with no turn-taking. It is substrate, not policy — it provides environments, episodes, agents, the message envelope, and LLM plumbing; applications decide everything else. Installable standalone (`uv pip install ./packages/freeagent`); it depends only on `nats-py`, `litellm`, `pydantic`, and `pyyaml`. See the repository's `DESIGN.md` for the authoritative design.

## The agent is a fold

An `Agent`'s life is a fold over a single, locally ordered event stream: in-world messages (`perceive`), control messages (`control`), and internal think messages (`handle_think`) merged into one sequence and processed **one handler at a time** on one asyncio loop — no threads. Same state plus the same event sequence yields the same new state and the same outgoing messages, which makes unit testing and replay exact. This local event sequence is exactly the agent's RL trajectory.

The hard rule that makes this work: **handlers are fast and non-blocking. An LLM call never happens inside a handler.** Slow work is spawned as an asyncio task (`Agent.spawn`) whose completion is posted back to the think queue (`Agent.think`) as an ordinary internal message. All the messy asynchrony lives between folds; the fold itself stays simple.

**The outbox.** `act(payload, recipients=None)` does not publish — it appends to an outbox that the runtime flushes only after the handler returns (and discards if the handler raises), so outgoing messages land at handler boundaries, never mid-thought. Recipients are agent IDs (default: broadcast on the public channel; the reserved name `env` addresses the environment's inbox); application code never touches raw NATS subjects.

**The think queue** is *not* behind the outbox: `think()` writes the agent's internal queue directly — it is internal state, not an effect on the world. `schedule_think` and `schedule_periodic_think` provide delayed and periodic self-messages, which is how applications build timers, gatekeeper polls, and drain-the-inbox batching.

## The environment owns the lifecycle

An `Environment` is the same fold model with three differences: no think queue (but timers, which enter its fold as events), a constructor that *creates* the episode (app name, roster, episode id, timeouts), and lifecycle ownership. The base class implements the whole state machine — `setup → running → stopping → ended` (or `aborted`): presence confirmation by request/reply with per-process nonces (a duplicate agent name aborts the episode), the `start` and `shutdown` broadcasts, and the setup timeout, episode timeout, and stopping grace period. A minimal environment is the base class plus a roster, zero overrides.

Control flows one way: only the environment sends on the control subject; agents obey it and never write to it. Agents *can* send app-defined management messages to the environment's inbox (e.g. "the game is over") — the inbox message is information, the control broadcast is the decision.

## LLM infrastructure

- `LLM` is an async client over litellm, used only via the spawn-don't-block pattern. Structured outputs are requested as JSON and validated with Pydantic models. Optional per-call telemetry (prompt, completion, timing) is published to the agent's log-only subject — which is how "considered speaking, chose silence" becomes training data.
- Model resolution (`resolve_model`): explicit argument → configured (runner yml) → the `FREEAGENT_MODEL` env var → auto-detect the cheap tier of whichever provider key is present (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`) → a clear error naming the env vars checked.
- `FakeLLM` is a deterministic, scriptable LLM — no network, no keys — selected through the same mechanism via the model strings `"fake"` (responses registered in test code) and `"fake:<path>"` (a canned YAML file of regex-matched responses). Everything downstream runs without a provider.
- `LLMAgent` is an agent defined primarily by text prompts: its state is the transcript of perceived messages, and each perception spawns (or defers to an in-flight) decision call returning the structured `Decision` schema — speak or stay silent, and what to say. Subclasses customize the prompt, the schema, and structured side-decisions.
- A room of LLMAgents can stall: everyone decides to wait for someone else, and with no new messages no one ever decides again. The opt-in `nudge_interval` config key (seconds) schedules a periodic re-decision during a lull, with a prompt hint that taking the initiative is allowed — the design's "act or stay silent?" gatekeeper pattern. Each agent still decides for itself; there is no turn-taking.

## Testing

The library's own tests run without NATS using `MemoryTransport`; the Collatz integration test (a deterministic no-LLM application in `tests/`) exercises the full stack — lifecycle, messaging, the fold, the outbox, the think queue — against a local NATS container, and skips cleanly when it is not running.
