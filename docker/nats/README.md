# NATS + JetStream

FreeAgent's only infrastructure: a NATS server with JetStream enabled. It is assumed to exist — no FreeAgent process manages it. Start it from the repository root:

```sh
docker compose -f docker/nats/docker-compose.yml up -d
```

Clients connect at `nats://localhost:4222`, and JetStream state persists across restarts. Each episode gets its own stream, which the environment creates and the recorder drains.
