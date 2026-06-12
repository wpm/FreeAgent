# free-agent (the runner)

The dev/demo runner: one command takes an episode from nothing to `ended` instead of six terminals.

```sh
free-agent run <config>.yml
```

It validates the configuration first — names, class references, base classes, all before launching anything — then runs every roster member and the environment as separate OS processes (agents really are independent processes, exactly as they would be across machines), plus the recorder when enabled. Launch order doesn't matter: agents start idle and JetStream replay means nothing can be missed, only delayed. The runner waits for the environment process — its exit code is the episode outcome — gives agents their stopping grace period to wind down on their own, terminates stragglers, waits for the recorder to finish writing, and prints a one-line summary:

```
free-agent: app=twentyquestions episode_id=<id> state=ended
```

## Configuration

Agent names are assigned top-down from this file; agents never choose their own. The directory containing the yml is put on the import path (parent and children), so application modules can live next to their config.

```yaml
app: twentyquestions                 # required, [A-Za-z0-9_-]+; the subject-root prefix
episode_id: ep-2026-06-11            # optional; auto-generated (uuid4 hex) when omitted
nats_url: nats://localhost:4222      # optional, this default
recorder:                            # optional; absent == disabled
  enabled: true                      # optional, default true when the block is present
  output: out/episode.parquet        # optional, default "<episode_id>.parquet"
environment:
  class: module.path:ClassName       # required; must subclass freeagent.Environment
  config: {setup_timeout: 30, episode_timeout: 600, grace_period: 5}   # verbatim kwargs
agents:                              # required, at least one; keys are the roster
  alice:
    class: module.path:ClassName     # required; must subclass freeagent.Agent
    config: {model: "fake:path.yml", system_prompt: "..."}             # verbatim kwargs
```

Validation fails fast with a clear error: invalid or reserved names (`env`), malformed `module:ClassName` references, duplicate YAML keys, unimportable classes, and wrong base classes are all rejected before any process starts.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | episode ended normally |
| 2 | episode aborted (roster incomplete at the setup timeout, or a duplicate agent name) |
| 1 | configuration, launch, or internal error — including operator interruption (Ctrl-C) |
