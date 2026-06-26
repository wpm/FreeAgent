# The FreeAgent backend network

[`compose.yml`](./compose.yml) brings up the FreeAgent **backend** as **two
services on one private network** (ADR-0003/[0004](../docs/decision-history/0004-app-independent-service.md)):

- **`nats`** — the durable record (JetStream on a persistent volume). It is
  internal to the network: it publishes **no** host ports. No UI ever speaks to
  it.
- **`freeagent`** — the episode service: an **app-independent** REST/JetStream
  API plus the per-episode WebSocket feed. It serves **no UI** and bundles no
  application. It is the **only** service that publishes a port to the host.

A Parquet volume is mounted into `freeagent` for import/export edge I/O.

This network is the backend only. A UI — for example the **Twenty Questions
viewer** — is a separate process you run on the host that calls this API
cross-origin; it is **not** part of this network. See
[`apps/twentyquestions/viewer/README.md`](../apps/twentyquestions/viewer/README.md).

## Bring up the backend

```sh
docker compose -f docker/compose.yml up --build      # Ctrl-C to stop
```

The REST API answers at <http://localhost:8000> (e.g.
`curl http://localhost:8000/freeagent/episodes`). Stop with
`docker compose -f docker/compose.yml down`.

To then watch a game in the browser, start the viewer separately and point it at
the service:

```sh
pnpm install                                  # once, to fetch the JS toolchain
pnpm --filter twentyquestions-viewer run dev  # http://localhost:5173
```

## What runs where

| Service     | Image                                   | Host port            | In-network address  |
| ----------- | --------------------------------------- | -------------------- | ------------------- |
| `nats`      | `nats:2.12-alpine`                      | *(none)*             | `nats://nats:4222`  |
| `freeagent` | built from [`freeagent/Dockerfile`][df] | `${FREEAGENT_PORT:-8000}` | `http://freeagent:8000` |
| `worker`    | built from [`worker/Dockerfile`][wf]    | *(none)*             | *(no server)*       |

[df]: ./freeagent/Dockerfile
[wf]: ./worker/Dockerfile

The `freeagent` image is a single Python stage that installs **only** the
`freeagent` library from the uv workspace (`uv sync --frozen --package
freeagent --extra service`) — no UI bundle and no application engine. The web
stack (FastAPI/uvicorn) lives behind a `service` extra so the worker can omit it.
The service is started with `free-agent serve --host 0.0.0.0` and reaches NATS at
`nats://nats:4222` (`FREEAGENT_NATS_URL`).

## Workers (ADR-0005)

The `worker` service is the **fat** image: it installs the `freeagent` library
**plus** the application engines (`uv sync --frozen --package twentyquestions`),
so it can import the roles a manifest names. A worker is a long-lived,
app-agnostic supervisor — `free-agent work` — that pulls episode manifests off
the in-network NATS work queue and forks each as a child process. It publishes
**no** port and serves no UI.

Workers are stateless and horizontally scalable. Bring up *N* of them with:

```sh
docker compose -f docker/compose.yml up --build --scale worker=2
```

(There is deliberately no `container_name` on the service — it would forbid
replicas.) A worker against an empty queue connects to NATS, binds the shared
durable consumer, and idles on its pull loop.

### How the backend runs an episode

The three tiers divide the work cleanly:

1. **`create`** on the service enqueues the episode's manifests (one per role —
   the environment and each agent) onto the shared work queue and returns. The
   service launches nothing and holds no handle.
2. **Workers** pull manifests off the queue — each manifest to exactly one
   worker — fork the role as a child process, confirm it came up, ack, and then
   supervise that child at the OS level. Scaling the pool (`--scale worker=N`)
   is how you scale throughput; the workers self-balance by spare capacity, with
   no scheduler.
3. The **episode itself** runs environment-led over NATS, exactly as a
   CLI-launched episode does — presence → `start` → `shutdown`. The feed shows it
   no differently. A worker that dies mid-episode loses its child; the
   environment's liveness check notices the missing role and aborts that episode
   cleanly (no role is ever re-run into a live episode).

So the rule of thumb is **fill the queue, scale the pool**: provisioning
(`create`) and execution (the workers) are decoupled through the queue, and the
durable record in JetStream — never a process handle — is the source of truth
for what should be running and what ran.

## Just NATS (development and tests)

To run episodes from the CLI or the test suite against a local NATS without the
service, use [`nats/docker-compose.yml`](./nats/docker-compose.yml), which brings
up only NATS with its client/monitoring ports published — see
[`nats/README.md`](./nats/README.md).
