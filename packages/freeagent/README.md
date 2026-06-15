# freeagent

The FreeAgent library: real-time multi-agent LLM interaction with no turn-taking. It is substrate, not policy — it provides the medium in which agents interact, and applications decide everything else. It is installable on its own. See the repository's [design document](../../docs/DESIGN.md) for the authoritative design.

## Architecture

The library gives an application four things and stays out of the way of the rest.

**Agents.** An agent reacts to a single, locally ordered stream of events — messages it perceives, lifecycle signals it receives, and results of its own background work — one at a time. The same starting state and the same sequence of events always produce the same outcome, which makes agent behavior testable and replayable, and makes each agent's event stream a clean record of its experience. Slow work, such as an LLM call, runs in the background and re-enters the agent as an ordinary event, so reactions stay quick and an agent is never blocked mid-thought. Agents address each other and the environment by name; the library handles the messaging underneath.

**The environment.** Each episode has one environment that owns its lifecycle — confirming everyone has joined, starting the episode, ending it, and enforcing the timeouts and the wind-down grace period. Lifecycle control flows one way: the environment directs, agents obey. Agents can tell the environment things (for example, that a game is over), but the decision to end an episode is always the environment's.

**LLM infrastructure.** An async LLM client, structured outputs validated against a schema, automatic model selection from whichever provider key is present, optional per-call telemetry, and a deterministic fake LLM that lets whole episodes run with no network and no keys. There is also a prompt-driven agent base for agents defined mostly by text rather than code, including built-in handling for the case where every agent is waiting for someone else to speak.

**The log.** Every message in an episode is captured in order, and the recorder drains that capture into one Parquet file per episode — the same record that doubles as multi-agent training data.

None of this is mandatory: an application uses as much of it as it needs.

## Tests

```sh
uv run pytest packages/freeagent
```

The library's own tests run with no NATS and no network. A full-stack integration test exercises a complete episode against a local NATS container and skips cleanly when it is not running.
