# freeagent-worker

The Free Agent worker command line app.

The worker is a **dumb host**:
it resolves an application by name, builds that application's environment and agents from an
`EpisodeSpec`, runs them to episode completion over NATS, and exits. All application intelligence
lives behind the SDK's `Application` protocol — the worker knows nothing about any app's messages or
config.

```console
$ freeagent-worker run collatz --episode-id 57 --config '{"starts": [6, 7, 9]}'
Episode "57" of application "collatz" complete.
```

`run` options:

- `--episode-id` (required): the episode's identifier.
- `--nats-url`: NATS server URL the worker observes the episode on (default `nats://localhost:4222`).
- `--config`: application config as a JSON string, or a path to a JSON file. Opaque to the worker;
  the application validates it.
- `--episode-root`: root NATS subject for the episode (default `episode.{episode-id}`).
- `--timeout`: seconds to wait for the episode to complete before failing; unset waits indefinitely.

An unknown application name exits nonzero with the list of installed applications; a malformed config
or a failed episode also exits nonzero, so a completed episode is distinguishable from a failed one
by exit code alone.
