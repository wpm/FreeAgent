# NATS + JetStream

FreeAgent's only infrastructure: a NATS server with JetStream enabled. It is assumed to exist — no FreeAgent process manages it. Start it from the repository root:

```sh
docker compose -f docker/nats/docker-compose.yml up -d
```

Clients connect at `nats://localhost:4222`, and JetStream state persists across restarts. Each episode gets its own stream, which the environment creates and the recorder drains.

## Browser viewers (websockets)

The server also runs a websocket listener so browser-based viewers can subscribe to episode traffic directly via [`nats.ws`](https://github.com/nats-io/nats.ws). Connect from the browser at:

```
ws://localhost:8080
```

It is unencrypted (`no_tls: true`) — fine for this trusted local research testbed (see [ADR-0001](../../docs/decision-history/0001-gui-viewers-over-nats.md)). The same JetStream streams are reachable over websockets as over the TCP client port, so viewers see the per-episode stream and its `stream_seq` ordering.

The server configuration lives in [`nats-server.conf`](./nats-server.conf), mounted into the container; the websocket and JetStream blocks are defined there.

## Configuring the published ports

The host ports are configurable via environment variables; the defaults reproduce the command above exactly:

| Variable            | Default | Container port | Purpose                       |
| ------------------- | ------- | -------------- | ----------------------------- |
| `NATS_CLIENT_PORT`  | `4222`  | `4222`         | Client connections            |
| `NATS_MONITOR_PORT` | `8222`  | `8222`         | HTTP monitoring               |
| `NATS_WS_PORT`      | `8080`  | `8080`         | Websocket (`nats.ws`) viewers |

Only the *published host* ports change; the container-internal ports are fixed, so a client still reaches the server at whatever host port you publish.

## Running isolated instances per worktree

Parallel test runs collide if they share one NATS server. To give each git worktree its own server with its own JetStream state, launch under a distinct Docker Compose project name and distinct host ports.

Docker Compose namespaces resources by project name (`COMPOSE_PROJECT_NAME`, or `-p`): with no explicit `container_name` or volume `name:` set, the container is named `<project>-nats-1` and the volume `<project>_nats-data`. So different project names yield fully isolated containers and volumes — JetStream state never leaks between instances.

Start an isolated server for a worktree:

```sh
COMPOSE_PROJECT_NAME=fa-wt1 NATS_CLIENT_PORT=4250 NATS_MONITOR_PORT=8250 NATS_WS_PORT=8090 \
  docker compose -f docker/nats/docker-compose.yml up -d
```

(Set `NATS_WS_PORT` too if more than one instance runs at once, or their websocket host ports collide.)

Then point that worktree's runtime and test suite at it. The runtime and the test suite both resolve their NATS URL from `FREEAGENT_NATS_URL`, falling back to `nats://localhost:4222`:

```sh
export FREEAGENT_NATS_URL=nats://localhost:4250
uv run pytest                 # or: free-agent <app> run <config>
```

Tear the instance down the same way you brought it up — by project name:

```sh
COMPOSE_PROJECT_NAME=fa-wt1 docker compose -f docker/nats/docker-compose.yml down -v
```

> **Avoid host port 4299 for an isolated instance.** The transport test suite uses
> `nats://127.0.0.1:4299` as a known-dead address to assert that connecting to an
> absent server raises. It connects there directly, regardless of
> `FREEAGENT_NATS_URL`, so a real NATS listening on 4299 makes that test fail. Pick
> any other free port (the examples here use `4250`/`8250`).

The default instance (`docker compose -f docker/nats/docker-compose.yml up -d`, serving `4222`/`8222`) is unaffected and can run concurrently alongside any number of named instances.
