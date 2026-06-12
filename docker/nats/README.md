# NATS + JetStream

FreeAgent's only infrastructure: a NATS server with JetStream enabled. It is assumed to exist — no FreeAgent process manages it. Start it from the repository root:

```sh
docker compose -f docker/nats/docker-compose.yml up -d
```

Clients connect at `nats://localhost:4222`. JetStream state persists in the `nats-data` volume. Each episode gets one stream capturing `<app>.episode.<id>.>` — created by the environment, drained by the recorder.
