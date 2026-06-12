# freeagent-recorder

The FreeAgent episode recorder. There is a single logging mechanism in FreeAgent — **the wire is the log** — and this is it: the recorder drains one episode's JetStream stream (read from sequence 1, so nothing is missed) into one Parquet file, one row per message, payloads stored unparsed. It is its own application; the library contains no required logging code, and recording never slows live play.

It may run concurrently with a live episode or after it has ended. The termination rule covers both: the recorder stops and writes the file once it has seen the environment's `shutdown` control broadcast *and* no new message has arrived for `--idle-timeout` seconds (an already-ended episode replays instantly from the stream, shutdown included, and the idle timer expires). The `free-agent` runner launches it automatically when the episode config has a `recorder:` block.

## CLI

```sh
freeagent-recorder --nats-url <url> --app <app> --episode-id <id> --output <path> [--idle-timeout <secs>]
```

Exits 0 with `wrote N rows to <path>` on success; nonzero with a message on stderr on failure (NATS unreachable, the episode's stream never appears).

## Schema

Rows are sorted by `stream_seq`, the episode's authoritative total order. The envelope carries no timestamps; `received_at` is JetStream's server-assigned arrival time — one clock for the whole episode.

| Column | Type | Notes |
|--------|------|-------|
| `episode_id` | string | from the message envelope |
| `stream_seq` | int64 | JetStream stream sequence — the ordering truth |
| `subject` | string | raw NATS subject (`<app>.episode.<id>.public`, `.control`, …) |
| `sender` | string | agent name, or `env` for the environment |
| `received_at` | timestamp (µs, UTC) | JetStream server clock |
| `payload` | string | the envelope's payload as JSON, unparsed and uninterpreted |

Messages that are not parseable envelopes are still recorded (empty `sender`, raw bytes decoded best-effort) rather than dropped.
