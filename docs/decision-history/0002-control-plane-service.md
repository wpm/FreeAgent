# ADR-0002: A persistent control-plane service over a REST façade

**Status:** Accepted
**Date:** 2026-06-16
**Deciders:** Bill McNeill

## Context

Until now, watching a FreeAgent episode meant orchestrating four moving parts by
hand: start NATS, run `free-agent twenty-questions run` with a chosen episode id,
start the viewer's dev server, and hand-edit a URL to carry that same id. The
launcher is a **run-to-completion CLI** — `freeagent.cli.orchestrate.run_episode`
spawns the agents and environment as child processes, blocks until the
environment exits, maps the exit code to ended/aborted/error, prints a summary,
and returns. That shape is right for one episode from a terminal and wrong for a
daemon that must start episodes, keep running, report their status, and stop them
on request — many at once.

We want a **persistent control service** that hosts a web API to create, list,
inspect, stop, and tear down episodes, so a browser application can configure and
launch episodes and watch several at once (live or replay) without the manual
dance. Several forces shape how it should be built:

- **The wire must not change.** Episodes already have a settled NATS subject
  layout (`<app>.episode.<id>.*`, see `packages/freeagent/src/freeagent/subjects.py`)
  and a viewer that subscribes to it (ADR-0001). A control plane that altered
  subjects would break every existing observer and replay consumer.
- **The service is generic across applications.** It cannot import Twenty
  Questions — or any app — by name; it must launch an *arbitrary* installed app.
- **An operator must be able to stop an episode well.** DESIGN.md already names
  "an external operator intervenes" as a first-class shutdown trigger, but killing
  the environment process would skip the `shutdown` broadcast and the grace
  period, stranding agents mid-goodbye.
- **NATS is assumed to exist.** DESIGN.md's architecture principle is that NATS
  "is assumed to exist … not part of any FreeAgent process." A service that
  embedded or managed the NATS container would quietly overturn that principle.
- **This is a trusted local testbed.** Consistent with `SECURITY.md` and the
  no-auth local NATS config (`docker/nats/nats-server.conf`), the control plane
  is a developer convenience on `localhost`, not a multi-tenant production system.

## Decision

Build a **persistent `freeagent` control service** — a long-running process that
hosts a localhost HTTP API and owns the set of running episodes. It is assembled
from three foundations that landed first, then exposes them over REST.

### REST façade over unchanged subjects

The API is shaped as `freeagent/<app>/<episode>` — e.g.
`freeagent/twenty-questions/32`. This is a **façade mapped at the API boundary
only**: the service translates the REST path onto today's wire subjects
(`<app>.episode.<id>.*`) and the NATS wire is untouched. A viewer maps the REST
id back to the subject root and subscribes exactly as it does today; it cannot
tell a service-launched episode from a `run`-launched one.

The mapping must absorb one naming gotcha: the REST/CLI name is dashed
(`twenty-questions`) while the subject prefix is undashed (`twentyquestions`).
That mapping is defined **once**, in the app registry (`load_apps` keys each
`AppSpec` by its dashed entry-point name; each spec carries its undashed
`AppSpec.name` subject prefix — `packages/freeagent/src/freeagent/cli/apps.py`).
Episode ids are service-assigned short, subject-safe strings
(`subjects.validate_name`, `[A-Za-z0-9_-]+`).

Endpoints (the verbs, not their exact spelling, which is settled in the service
PR): **create** an episode for an app (the body carries the settable config —
NATS URL, the Host's secret, model, logging file, and a **live | replay(parquet
path)** mode); **list** running episodes (what the service "displays");
**get** one episode's status; **stop** (graceful abort) an episode; and
**teardown** to bring everything down. Replay reuses the existing
`Replayer` / `free-agent replay` path (ADR-0001) — "create" simply gains a
live|replay mode, so the viewer keeps its single code path.

### Episode registry over a non-blocking handle

The service holds an **in-memory registry** of running episodes, each an
`EpisodeHandle` (`freeagent.cli.orchestrate`). `start_episode` is the primitive
that makes this possible: it launches an episode's child processes and returns a
handle **without blocking**, running supervision (await the environment, wind
agents down, clean up stragglers) as a background task. A daemon holds many
handles in one event loop, queries each one's state, awaits completion, and
requests an abort — with no process-wide or global state, and signal handling
made opt-in so only the CLI installs SIGINT/SIGTERM. `run_episode` is now the
synchronous CLI behavior built on the same primitive, with byte-identical exit
codes and summary line.

The registry is **not persisted across service restarts** in v1 — a known
limitation. A restart loses the service's view of episodes (the episodes
themselves are independent OS processes and survive, but the handles do not).

> **Amended (#36): discovery over JetStream softens this.** The browser now
> discovers episodes by listing JetStream streams directly (`<app>_episode_<id>`,
> see `subjects.py`), not by asking the registry. Because the wire already holds
> the authoritative set of episodes, a restart no longer loses **visibility or
> watchability**: every episode whose stream still exists — live, replayed, from
> the CLI, from another instance, or from before the restart — remains
> discoverable and watchable from the browser. What a restart still loses is only
> the **control handle**: the ability to gracefully *stop* a pre-restart episode,
> which is inherent (a new process cannot reattach a supervisor to children it
> never spawned). So `EpisodeView.controllable` is true only while the service
> owns the handle; a discovered-but-unowned episode is read-only. Durable
> persistence of the *handle* across restarts remains the open follow-up; durable
> persistence of *episode existence* is, in effect, already provided by JetStream.

### Generic launch via app self-description

Because the service is generic, an app must **advertise** what `run_episode`
needs rather than have the service import it. Each app registers an `AppSpec` on
the existing `freeagent.apps` entry-point group, carrying the subject-prefix
name, the environment class, the roster (name → agent class), and a **settable
config surface** — plain, wire-safe `ConfigField` data (field name, JSON-ish type
tag, help text; no class references) describing which `config` keys an operator
may set per component. `load_app(rest_name)` returns the spec or a clean
`UnknownAppError`; the library still never imports its applications by name. The
**roster is fixed in source** for v1: config covers URLs, the Host's secret,
model, and logging file — *not* player count.

### Operator-abort: a graceful service → environment request

Stopping an episode is a **request, not a command**. The service (or any
in-process holder of an `EpisodeHandle`) sends one management message,
`{"type": "freeagent.abort"}`, on the environment's existing inbox subject
`<root>.env` (`freeagent.control.publish_operator_abort` / `abort_episode`;
`EpisodeHandle.abort()`). The environment takes its **normal** stopping path —
broadcast `shutdown`, run the grace period, agents wind down identically to any
other shutdown — but settles in `aborted` (exit code 2) rather than `ended`.
Control authority stays with the environment: the message is the request, the
`shutdown` broadcast is the decision, and the request is a no-op unless the
episode is running.

The abort rides the environment inbox rather than a dedicated out-of-band
subject, for one decisive reason: that inbox already lives under the episode's
subject root, so the JetStream stream captures the abort like every other message
and the **wire-is-the-log** invariant holds with no side channel to reason about.
Any acks (none are needed — the `shutdown` broadcast is the observable
acknowledgement) would use episode-scoped reply subjects, never NATS's `_INBOX.>`.

### NATS is verified, not managed

On startup and on create, the service **verifies NATS is reachable** and fails
with a clear error if it is not — no hang. It does **not** embed NATS, and in v1
it does **not** manage the Docker container; bringing NATS up stays the
operator's job (`docker compose -f docker/nats/docker-compose.yml up -d`). This
keeps DESIGN.md's "NATS … is assumed to exist … not part of any FreeAgent
process" principle intact: verifying a dependency is reachable is not the same as
owning its lifecycle. Optional container orchestration is **deferred** (issue #29)
and, when it lands, will be opt-in so the default still honors the principle.

### Local, no-auth, CORS-enabled

The service binds to **localhost** with **no authentication**, consistent with
the trusted-testbed posture of ADR-0001 and `SECURITY.md`. It enables **CORS** so
the viewer's Vite dev server (default `:5173`) can call the API. Whether the
service also serves the built viewer bundle (one origin) or stays API-only
(simpler dev) is a detail settled in the service PR, not a load-bearing decision
here.

### Shared client code, not an SDK

The browser app gets only as much shared JavaScript as it needs to call the API
and build subjects from the `freeagent/<app>/<episode>` mapping — **no standalone
SDK package yet**. As with the viewer SDK in ADR-0001, a shared client is a
generalization best extracted from real use, not designed up front.

## Options Considered

### Façade vs. new wire protocol

| Option | Assessment |
|--------|------------|
| **REST façade over unchanged subjects (chosen)** | The API is a translation layer; NATS subjects, schemas, viewers, and replay are all untouched. |
| New/extended wire protocol for control | Would let the service speak one protocol end to end, but breaks every existing observer and replay consumer and re-opens a contract ADR-0001 deliberately froze. |

The façade wins decisively: the wire contract is the expensive thing to change
(ADR-0001), the REST shape is cheap and human-friendly, and a translation at the
boundary costs only the dashed/undashed name mapping — which already exists in
the app registry.

### Aborting an episode: kill vs. request

| Option | Assessment |
|--------|------------|
| **Operator-abort request on `<root>.env` (chosen)** | Graceful: env broadcasts `shutdown`, grace period runs, agents say goodbye, episode settles in `aborted`. Captured by the stream. |
| Kill the environment process | Immediate but brutal: skips the broadcast and grace period, strands agents mid-goodbye, and leaves no record on the wire. |
| Dedicated out-of-band abort subject | Cleanly separated from app traffic, but escapes the episode's subject root and so the stream — a side channel to reason about separately, against wire-is-the-log. |

Kill remains available as the **hard** interrupt (`EpisodeHandle.request_abort()`,
what the CLI's Ctrl-C uses), but the service's normal "stop" is the graceful
request.

### Service vs. enhanced one-shot CLI

A persistent service is more than the CLI needs for a single run, but the whole
point is to own *many* episodes' lifecycles concurrently and answer "what's
running?" — state a run-to-completion command cannot hold. Building the service
on the non-blocking `EpisodeHandle` means the CLI and the daemon share one
orchestration primitive rather than forking two.

## Trade-off Analysis

The decisive trade is **convenience and concurrency vs. added surface and a
running process**. The control service adds a long-lived component, an HTTP API,
and CORS — but it removes the four-step manual dance, lets a browser app drive
and watch many episodes, and does it all without touching the NATS contract that
ADR-0001 froze.

On NATS lifecycle, the trade is **honesty vs. one-command convenience**. Managing
the container would be a nicer first-run experience but would silently overturn a
stated architecture principle. Verify-don't-manage keeps the principle true,
gives a clear error instead of a hang, and leaves the convenience as an explicit,
opt-in, deferred follow-up rather than a quiet default.

On persistence, an in-memory registry is the **simplest thing that proves the
design**. Durable episode tracking across restarts is real work for uncertain v1
value; naming it a known limitation lets the service ship and defers the cost
until evidence asks for it.

Security is again treated as a non-concern (per ADR-0001): localhost, no auth,
CORS for the dev server. Revisit if the service is ever exposed beyond a trusted
local machine or human-in-the-loop control becomes a goal.

## Consequences

- **Easier:** create / list / inspect / stop / teardown episodes over a simple
  REST API; a browser app that configures and watches many concurrent episodes
  (live or replay) with no hand-edited URLs; a generic launcher that runs any
  installed app via its `AppSpec`; graceful operator stop that respects goodbyes;
  one orchestration primitive shared by CLI and daemon.
- **Harder / new work:** run and maintain a persistent service and its HTTP API;
  keep the REST↔subject mapping (including dashed/undashed names) correct at the
  boundary; configure CORS; live with no cross-restart persistence in v1.
- **Watch / revisit:** episode-registry persistence across restarts; container
  orchestration (#29, opt-in); extracting a shared client SDK once a second
  consumer exists; auth and non-local binding if the testbed assumption ever
  changes.

## Action Items

1. [x] Refactor the orchestrator into a non-blocking, supervised `EpisodeHandle`
       (`start_episode`); keep `run_episode` identical on top of it. (#23)
2. [x] Let applications self-describe via `AppSpec` on the `freeagent.apps`
       entry point — name, environment, roster, settable config — discoverable by
       dashed REST name without importing the app. (#24)
3. [x] Add the operator-abort protocol: `{"type": "freeagent.abort"}` on
       `<root>.env`, graceful `stopping → aborted`, wired into
       `EpisodeHandle.abort()`. (#25)
4. [ ] Build the control service: REST API under `freeagent/<app>/<episode>`
       mapped onto wire subjects, in-memory episode registry, verify NATS
       reachable, localhost + CORS. (#26)
5. [ ] Build the Twenty Questions controller app: configure, launch, and watch
       many episodes concurrently over the API; retain history-file replay. (#27)
6. [ ] *(Deferred, not v1)* optionally orchestrate the NATS container, opt-in, so
       the default still honors "NATS is assumed to exist." (#29)
