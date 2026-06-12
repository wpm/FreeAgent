"""Drain one episode's JetStream stream into a single Parquet file.

The recorder is infrastructure: unlike application code it legitimately sees
raw subjects and JetStream server metadata. It consumes the episode's stream
from sequence 1 and records every message as one row, per DESIGN.md's logging
section ("the wire is the log").

Termination rule: the recorder keeps consuming while the episode is live. It
stops and writes the file once BOTH hold:

(a) it has seen a control message with payload ``{"type": "shutdown"}``, and
(b) no new message has arrived for ``idle_timeout`` seconds.

This single rule covers both running concurrently with a live episode and
running after the episode ended (history replays instantly, shutdown is in
it, and the idle timer expires).
"""

from __future__ import annotations

import asyncio
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

import nats
import nats.errors
import nats.js.errors
import pyarrow as pa
import pyarrow.parquet as pq

from freeagent import Envelope, EpisodeSubjects

if TYPE_CHECKING:
    from collections.abc import Iterable
    from datetime import datetime

DEFAULT_IDLE_TIMEOUT: float = 5.0
"""Seconds of post-shutdown silence after which the episode is considered over."""

DEFAULT_STREAM_WAIT: float = 10.0
"""Seconds to wait for the episode's stream to exist before giving up."""

PARQUET_SCHEMA: pa.Schema = pa.schema(
    [
        pa.field("episode_id", pa.string()),
        pa.field("stream_seq", pa.int64()),
        pa.field("subject", pa.string()),
        pa.field("sender", pa.string()),
        pa.field("received_at", pa.timestamp("us", tz="UTC")),
        pa.field("payload", pa.string()),
    ]
)
"""The pinned Parquet schema, per DESIGN.md's logging section."""


class RecorderError(Exception):
    """A recorder failure with a message suitable for stderr."""


@dataclass(frozen=True, slots=True)
class MessageRecord:
    """One stream message, flattened into the Parquet row shape.

    ``payload`` is the JSON serialization of the envelope's ``payload`` field,
    unparsed and uninterpreted beyond extracting it from the envelope JSON.
    For messages that are not parseable envelopes, ``sender`` is empty and
    ``payload`` is the raw data decoded best-effort.
    """

    episode_id: str
    stream_seq: int
    subject: str
    sender: str
    received_at: datetime
    payload: str


def make_record(
    *,
    stream_seq: int,
    subject: str,
    received_at: datetime,
    data: bytes,
    fallback_episode_id: str,
) -> MessageRecord:
    """Build a row from one raw stream message.

    Messages that are not parseable envelopes are still recorded (sender
    empty, payload = raw bytes decoded best-effort) rather than crashing.
    """
    try:
        envelope = Envelope.from_bytes(data)
    except Exception:
        return MessageRecord(
            episode_id=fallback_episode_id,
            stream_seq=stream_seq,
            subject=subject,
            sender="",
            received_at=received_at,
            payload=data.decode("utf-8", errors="replace"),
        )
    return MessageRecord(
        episode_id=envelope.episode_id,
        stream_seq=stream_seq,
        subject=subject,
        sender=envelope.sender,
        received_at=received_at,
        payload=json.dumps(envelope.payload, ensure_ascii=False),
    )


def write_parquet(records: Iterable[MessageRecord], output: Path) -> None:
    """Write records to one Parquet file, sorted by ``stream_seq``.

    Independent of NATS so it can be unit-tested without a server.
    """
    rows = sorted(records, key=lambda record: record.stream_seq)
    table = pa.Table.from_pydict(
        {
            "episode_id": [row.episode_id for row in rows],
            "stream_seq": [row.stream_seq for row in rows],
            "subject": [row.subject for row in rows],
            "sender": [row.sender for row in rows],
            "received_at": [row.received_at for row in rows],
            "payload": [row.payload for row in rows],
        },
        schema=PARQUET_SCHEMA,
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(table, output)


def _is_shutdown(subjects: EpisodeSubjects, subject: str, data: bytes) -> bool:
    """True when this message is the environment's shutdown broadcast."""
    if subject != subjects.control:
        return False
    try:
        envelope = Envelope.from_bytes(data)
    except Exception:
        return False
    payload = envelope.payload
    return isinstance(payload, dict) and payload.get("type") == "shutdown"


async def record_episode(
    *,
    nats_url: str,
    app: str,
    episode_id: str,
    output: Path,
    idle_timeout: float = DEFAULT_IDLE_TIMEOUT,
    stream_wait: float = DEFAULT_STREAM_WAIT,
) -> int:
    """Drain one episode's stream to ``output`` and return the row count.

    May run concurrently with a live episode or after it ended; see the
    module docstring for the termination rule. Raises :class:`RecorderError`
    on failure (NATS unreachable, stream never appears).
    """
    subjects = EpisodeSubjects(app=app, episode_id=episode_id)

    async def _error_cb(error: Exception) -> None:
        # One concise line instead of nats-py's default full-traceback dump.
        print(f"freeagent-recorder: nats error: {error}", file=sys.stderr)

    try:
        client = await nats.connect(
            nats_url, connect_timeout=5, max_reconnect_attempts=3, error_cb=_error_cb
        )
    except Exception as error:
        raise RecorderError(f"cannot connect to NATS at {nats_url}: {error}") from error

    try:
        js = client.jetstream()
        await _wait_for_stream(js, subjects.stream, stream_wait)
        # Ordered push consumer: DeliverPolicy.ALL, so the stream is read
        # from sequence 1 and history replays before any live traffic.
        subscription = await js.subscribe(
            subjects.all_subjects,
            stream=subjects.stream,
            ordered_consumer=True,
        )
        records: list[MessageRecord] = []
        shutdown_seen = False
        while True:
            try:
                message = await subscription.next_msg(timeout=idle_timeout)
            except nats.errors.TimeoutError:
                # Idle. Done only if the shutdown broadcast was already seen;
                # otherwise the episode is still live and we keep waiting.
                if shutdown_seen:
                    break
                continue
            metadata = message.metadata
            records.append(
                make_record(
                    stream_seq=metadata.sequence.stream,
                    subject=message.subject,
                    received_at=metadata.timestamp,
                    data=message.data,
                    fallback_episode_id=episode_id,
                )
            )
            if _is_shutdown(subjects, message.subject, message.data):
                shutdown_seen = True
        await subscription.unsubscribe()
    finally:
        await client.close()

    write_parquet(records, output)
    return len(records)


async def _wait_for_stream(js: nats.js.JetStreamContext, stream: str, stream_wait: float) -> None:
    """Wait up to ``stream_wait`` seconds for the episode's stream to exist."""
    deadline = asyncio.get_running_loop().time() + stream_wait
    while True:
        try:
            await js.stream_info(stream)
        except nats.js.errors.NotFoundError:
            if asyncio.get_running_loop().time() >= deadline:
                raise RecorderError(
                    f"stream {stream!r} not found after waiting {stream_wait:.1f}s"
                ) from None
            await asyncio.sleep(0.25)
        else:
            return
