# ADR-0009: One-command app launch

**Status:** Proposed
**Date:** 2026-07-08
**Deciders:** Bill McNeill

> Companion diagram: [`../launch-process-map.html`](../launch-process-map.html)
> (who starts what, who owns what, how the processes talk).

## Context

Running the Collatz application today takes four terminals: `uv run start`
(NATS in docker), `uv run freeagent-api`, `npm install && npm run build &&
npm run serve` in `apps/collatz/viewer`, and a browser. We want `uv run
collatz` to do all of that, the mechanism must generalize to every future
in-repo application, and it must work for a third-party author whose
application lives in their own repository and merely depends on
`freeagent-sdk`.

The *loading* half of this problem is already solved by
[ADR-0006](0006-entry-point-application-loading.md): applications register in
the `freeagent.applications` entry-point group and the worker finds them by
name. The *launching* half — bringing up the serving stack around an
application — has no mechanism at all.

Two constraints shape the answer:

- **The API is app-agnostic but environment-bound.** One `freeagent-api`
  serves every installed application, so it belongs to the platform, not to
  any app. But it spawns `python -m freeagent.worker.cli` in its own
  environment, so that environment must have every runnable application
  installed — including ones pip-installed after any docker image was built.
  Until the worker pool ([ADR-0005](0005-the-worker-pool.md)) decouples
  episode execution from the API process, the API must run on the host, in
  the venv where the applications live. It cannot join NATS in the compose
  file yet.

- **`uv run <name>` resolves any console script installed in the
  environment**, regardless of which distribution declares it. A name in an
  app's own `pyproject.toml` works at this repo's root *and* in a third-party
  repo, with no central registry of names.

## Decision

**Each application declares its own console script; a shared harness in
`freeagent.sdk.launch` does the orchestration; the API moves into the
`start`/`stop` switch.**

### Per-app names, app-owned

`apps/collatz/pyproject.toml` declares
`collatz = "freeagent.app.collatz.launch:main"` under `[project.scripts]`.
`main()` is a few lines that hand a launcher object to the shared harness. A
third-party author does the identical thing in their own distribution
(`acme = "acme.launch:main"`), and `uv run acme` works in their repo.

### The harness: `freeagent.sdk.launch`

A stdlib-only SDK module with three parts:

- A `Service` value (name, `prepare` commands run to completion, a
  long-running `serve` command, working directory, URL to print) and a
  `Launcher` protocol (`name`, `services()`) — the launching sibling of
  ADR-0006's `Application` protocol. A launcher describes serving processes
  only; it never creates episodes. Episode creation stays with clients
  (viewer → API → worker), preserving the API-never-imports-worker layering.
- `run(launcher)`: ensure the platform is up, run each service's `prepare`
  steps, spawn each `serve`, print URLs, block until SIGINT, then terminate
  in reverse order **only what it spawned**. `uv run collatz` is therefore a
  foreground *session* (like `npm run serve`), not a switch: Ctrl-C ends the
  app layer and never touches the platform.
- Platform ensure-and-own: the SDK ships the NATS compose file and server
  config as package data, so "bring up the platform" works from any repo.
  NATS is ensured via its `/healthz` monitoring endpoint and `docker compose
  up --detach --wait`; the API via its `/health` endpoint and a detached
  spawn recorded in a pid file under a state directory. That directory is the
  repo root's `.freeagent/` in a git repository, and a cwd-independent
  user-level directory otherwise (`$XDG_STATE_HOME/freeagent/`, falling back
  to `~/.freeagent/`) so that a third-party `ensure` and a later `stop`, run
  from different directories, agree on one pid file (issue #116).

### The API joins the platform switch

`uv run start` becomes: compose up NATS **and** spawn `freeagent-api`
detached (pid file); `uv run stop` kills the API and takes the network down.
Sessions never own the API — they call the same ensure code path — so two
concurrent app sessions share one API and neither can kill it under the
other. The pid file is scaffolding with a known demolition date: when
ADR-0005's worker pool lands, only workers need applications installed, the
API becomes containerizable, and the platform switch goes back to being pure
docker.

## Options considered

### Option A: Script per app in the root `pyproject.toml`

Simple, but the root project must be edited for every app and cannot be
edited at all by a third-party author. Fails the out-of-repo requirement.

### Option B: Containerize the app layer; compose profiles as switches

`docker compose --profile collatz up` makes every layer a true switch with
docker owning all state. But the API cannot be containerized while it spawns
workers in-process (environment-bound, above), the npm dev loop dies, and
third-party authors would have to ship images. Revisit after ADR-0005.

### Option C: Detached per-app switches

`uv run collatz` returns immediately; `uv run collatz --stop` tears down.
On/off symmetry with `start`/`stop`, but requires pid tracking for *every*
app's processes — a mini-supervisor sprawled across apps. Rejected in favor
of confining pid management to exactly one process (the API) in exactly one
place (the platform switch).

### Option D: Session-owned API

The first session to find the API down spawns and owns it; Ctrl-C kills it
under any other session sharing it. Rejected: the API is app-agnostic and
belongs to no session.

### Option E: Foreground session + app-owned script + SDK harness (chosen)

The decision above. A `freeagent.launchers` entry-point group (for a generic
`launch <name>` command) is deliberately deferred until something needs it.

## Consequences

- `uv run start; uv run collatz` goes from cold clone to browser; Ctrl-C
  leaves the platform up for the next session.
- A third-party app gets the same experience by depending on
  `freeagent-sdk`, adding `freeagent-api` to its dev dependencies (the
  harness spawns the script and must fail clearly when it is missing), and
  declaring one console script.
- The `start`/`stop` switch now manages one host process via pid file —
  accepted as scaffolding until the worker pool.
- Wheel-installed apps cannot `npm run build`; published apps should ship
  built viewer assets as package data and treat `prepare` as the in-repo dev
  path. Documentation, not machinery, for now.
- Import-linter contracts are untouched: apps and the harness import SDK
  names only.

## Action items

Tracked as the *app launch* epic (see `setup-epic-launch.sh`): SDK platform
ensure/own code, folding the API into `start`/`stop`, the session harness,
`uv run collatz`, and documentation.
