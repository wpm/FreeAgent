# freeagent-api

The Free Agent FastAPI server: launches and manages application episodes and serves their traffic
to viewers over REST, updating itself by listening to NATS.

## REST surface

| Method   | Path                                                  | Purpose                                              |
| -------- | ----------------------------------------------------- | ---------------------------------------------------- |
| `GET`    | `/applications`                                       | Installed applications                               |
| `POST`   | `/applications/{app}/episodes`                        | Provision an episode (spawns a worker process)       |
| `GET`    | `/applications/{app}/episodes/{id}`                   | Lifecycle status derived from control-plane traffic  |
| `GET`    | `/applications/{app}/episodes/{id}/messages`          | Ordered pass-through feed of data-plane JSON         |
| `DELETE` | `/applications/{app}/episodes/{id}`                   | Stop the episode                                     |

The API understands SDK-defined messages only (the control plane) and relays application-defined
messages as opaque JSON (the data plane). `freeagent-api` may only depend on names defined in
`freeagent-sdk`; import-linter enforces this in CI. Episodes are run by `freeagent-worker`
subprocesses — a process dependency, never an import.

The NATS server URL comes from the `FREEAGENT_NATS_URL` environment variable (default
`nats://localhost:4222`). Everything the API holds is an in-memory cache, best-effort under core
NATS, and never feeds an archive.
