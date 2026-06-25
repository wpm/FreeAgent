# The FreeAgent application network

[`compose.yml`](./compose.yml) brings up the whole Twenty Questions application
as **two services on one private network** (ADR-0003):

- **`nats`** — the durable record (JetStream on a persistent volume). It is
  internal to the network: it publishes **no** host ports. The browser never
  speaks to it.
- **`freeagent`** — the episode service: the REST API, the per-episode feed, and
  the built UI bundle, all served from **one origin**. It is the **only** service
  that publishes a port to the host.

A Parquet volume is mounted into `freeagent` for import/export edge I/O.

## One command

```sh
./docker/twenty-questions.sh
```

This builds the images, brings the network up, waits for the service, opens the
**Twenty Questions UI** at <http://localhost:8000>, and tails the logs. Stop with
`docker compose -f docker/compose.yml down`.

Equivalently, by hand:

```sh
docker compose -f docker/compose.yml up --build      # Ctrl-C to stop
# then open http://localhost:8000
```

In the browser: open **Settings** to set your model and provider API key (kept in
the browser, never on the bus), then start a game from the Twenty Questions
composer and watch the transcript stream live. Sealed games replay through the
exact same view.

## What runs where

| Service     | Image                                   | Host port            | In-network address  |
| ----------- | --------------------------------------- | -------------------- | ------------------- |
| `nats`      | `nats:2.12-alpine`                      | *(none)*             | `nats://nats:4222`  |
| `freeagent` | built from [`freeagent/Dockerfile`][df] | `${FREEAGENT_PORT:-8000}` | `http://freeagent:8000` |

[df]: ./freeagent/Dockerfile

The `freeagent` image is built in two stages: a Node stage builds the UI bundle
from the committed JSON-Schema-generated types, then a Python stage installs the
uv workspace (the `freeagent` library + the `twentyquestions` engine) and copies
the bundle in. The service is started with `free-agent serve --host 0.0.0.0`, and
`FREEAGENT_UI_DIR` points it at the bundle so the UI and API share one origin.
Episodes launch as child processes inside the `freeagent` container and reach
NATS at `nats://nats:4222` (`FREEAGENT_NATS_URL`).

## Just NATS (development and tests)

To run episodes from the CLI or the test suite against a local NATS without the
service or UI, use [`nats/docker-compose.yml`](./nats/docker-compose.yml), which
brings up only NATS with its client/monitoring ports published — see
[`nats/README.md`](./nats/README.md).
