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

[df]: ./freeagent/Dockerfile

The `freeagent` image is a single Python stage that installs **only** the
`freeagent` library from the uv workspace (`uv sync --frozen --package
freeagent`) — no UI bundle and no application engine. The service is started with
`free-agent serve --host 0.0.0.0` and reaches NATS at `nats://nats:4222`
(`FREEAGENT_NATS_URL`).

## Just NATS (development and tests)

To run episodes from the CLI or the test suite against a local NATS without the
service, use [`nats/docker-compose.yml`](./nats/docker-compose.yml), which brings
up only NATS with its client/monitoring ports published — see
[`nats/README.md`](./nats/README.md).
