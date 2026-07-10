# Free Agent

_Real time multi-agent framework_

## Quickstart

Free Agent runs on a small **platform** — a NATS network and the `freeagent-api`
process — that is shared by every installed application. You turn the platform on
once, then run as many application sessions against it as you like.

Turn the platform on:
```shell
uv run start
```

Run the Collatz application:
```shell
uv run collatz
```

`uv run collatz` is a foreground **session**: it makes sure the platform is up
(starting NATS and the API if they are not already running), builds the viewer,
serves it, and prints its address. Open it in a browser:

<http://localhost:8080>

Launch an episode (one starting number per agent) and watch each agent's Collatz
chain grow step by step until every chain reaches 1 and the episode completes.

Press **Ctrl-C** to end the session. That stops only what the session started
(the viewer); the platform keeps running, so the next `uv run collatz` — or any
other application — starts instantly.

When you are done for good, turn the platform off:
```shell
uv run stop
```

`start` and `stop` are the platform's on/off switch; they are idempotent, so
running `start` when everything is already up simply reports it. Starting NATS
needs Docker running; if it is not, `start` says so and exits without a
traceback.

**After changing the platform config** (`docker/compose.yaml` or
`docker/nats/nats-server.conf`), run `uv run stop` then `uv run start` to apply
it. `start` owns the platform and reconciles NATS against the compose file, but a
running app *session* (`uv run collatz`) deliberately leaves an already-running
NATS alone so it never disrupts another session — so a config change reaches a
live platform only through the `stop`/`start` switch, not through a session.

> **How the pieces fit together:** see
> [ADR-0009: One-command app launch](docs/decision-history/0009-one-command-app-launch.md)
> and its companion process map,
> [`docs/launch-process-map.html`](docs/launch-process-map.html), for who starts
> what, who owns what, and how the processes talk.
>
> **Writing your own launchable application?** The
> [`freeagent-sdk` README](packages/freeagent-sdk/README.md#making-your-application-launchable)
> walks through making `uv run <your-app>` work in your own repository.

## Testing

Run the whole suite with:
```shell
uv run pytest
```

Tests come in two tiers:
unit tests, which need no external services, and integration tests, which run against a real
`nats-server` subprocess started by the test fixtures on a random free port. Install the binary once
for local integration runs:
```shell
brew install nats-server
```

To run only the unit tests — no NATS binary required — deselect the integration marker:
```shell
uv run pytest -m "not integration"
```
