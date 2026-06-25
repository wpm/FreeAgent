"""Parquet import/export as episode edge I/O (ADR-0003).

With replay served from JetStream (#42), Parquet stops being a transport and
becomes the episode's **edge**: a way in and out of the durable record.

* **export** drains a *sealed* episode's stream to a ``.parquet`` file -- the
  same row shape the recorder writes -- for archival and interchange.
* **import** plays a ``.parquet`` into a **fresh** JetStream stream (a new
  episode id, with episode metadata, sealed when done) so it rejoins the normal
  browse / replay / delete surface like any other episode.

Both are library functions here and service endpoints (wired in
:mod:`freeagent.service.app` via :class:`~freeagent.service.ControlService`),
operating against a path on the service's mounted volume. Import is written
against the :class:`~freeagent.transport.Transport` abstraction so it is testable
without a server.
"""

from __future__ import annotations

import json
import secrets
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TYPE_CHECKING

import nats
import nats.errors
import nats.js.errors

from freeagent.envelope import Envelope
from freeagent.metadata import EpisodeMetadata
from freeagent.names import fallback_episode_name
from freeagent.recorder import make_record, write_parquet
from freeagent.replayer import ReplayerError, load_episode
from freeagent.subjects import EpisodeSubjects, validate_name

from .registry import EpisodeExistsError

if TYPE_CHECKING:
    from pathlib import Path

    from freeagent.replayer import ReplayMessage
    from freeagent.transport import Transport

#: How long export waits for the next message before deciding a sealed stream is
#: fully drained. A sealed stream's history replays at once, so this is short.
_DRAIN_IDLE = 3.0

#: Length, in random hex chars, of an import's assigned episode id.
_ID_BYTES = 4


class EdgeIOError(Exception):
    """An import/export failure with an operator-facing message (HTTP 400)."""


@dataclass(slots=True)
class ImportResult:
    """The identity of the episode an import created."""

    app: str
    episode_id: str
    name: str


def _subjects_of(messages: list[ReplayMessage]) -> EpisodeSubjects:
    """The episode identity a recorded log's subjects belong to (its first row)."""
    parts = messages[0].subject.split(".")
    if len(parts) < 4 or parts[1] != "episode":
        raise EdgeIOError(f"recorded subject {messages[0].subject!r} is not a FreeAgent subject")
    return EpisodeSubjects(app=parts[0], episode_id=parts[2])


async def export_episode(
    *,
    nats_url: str,
    app: str,
    episode_id: str,
    output: Path,
    require_sealed: bool = True,
) -> int:
    """Drain one episode's stream to a Parquet file; return the row count.

    Intended for a **sealed** episode (complete and immutable); set
    *require_sealed* false to export an open one at your own risk. The output
    path must not already exist -- a finished log is never overwritten. Raises
    :class:`EdgeIOError` on a missing stream, an unsealed episode, or an existing
    output.
    """
    if output.exists():  # noqa: ASYNC240 -- a quick stat on a local mounted volume; the parquet write below is likewise blocking
        raise EdgeIOError(f"{output} already exists; refusing to overwrite a log")
    subjects = EpisodeSubjects(app=app, episode_id=episode_id)
    client = await _connect(nats_url)
    try:
        js = client.jetstream()
        try:
            info = await js.stream_info(subjects.stream)
        except nats.js.errors.NotFoundError as exc:
            raise EdgeIOError(f"no episode {episode_id!r} for application {app!r}") from exc
        if require_sealed and not info.config.sealed:
            raise EdgeIOError(f"episode {episode_id!r} is not sealed; stop it before exporting")
        if info.state.messages == 0:
            raise EdgeIOError(f"episode {episode_id!r} has no messages to export")
        records = []
        from nats.js.api import ConsumerConfig, DeliverPolicy

        subscription = await js.subscribe(
            subjects.all_subjects,
            stream=subjects.stream,
            ordered_consumer=True,
            config=ConsumerConfig(deliver_policy=DeliverPolicy.ALL),
        )
        while True:
            try:
                message = await subscription.next_msg(timeout=_DRAIN_IDLE)
            except nats.errors.TimeoutError:
                break  # a sealed stream's history is fully drained
            meta = message.metadata
            records.append(
                make_record(
                    stream_seq=meta.sequence.stream,
                    subject=message.subject,
                    received_at=meta.timestamp,
                    data=message.data,
                    fallback_episode_id=episode_id,
                )
            )
        await subscription.unsubscribe()
    finally:
        await client.close()
    write_parquet(records, output)
    return len(records)


async def import_episode(
    transport: Transport,
    *,
    parquet_path: Path,
    episode_id: str | None = None,
    name: str | None = None,
) -> ImportResult:
    """Play a Parquet log into a fresh JetStream stream; return the new identity.

    The log's messages are re-published under a **new** episode id (so the
    import never collides with the original), the stream is given episode
    metadata and **sealed**, and it then appears and replays like any other
    episode. Raises :class:`ReplayerError` on a bad log, :class:`EdgeIOError` on
    an empty one, and :class:`EpisodeExistsError` when the target id is taken.
    """
    messages = load_episode(parquet_path)  # ReplayerError -> 400
    if not messages:
        raise EdgeIOError(f"{parquet_path} contains no messages to import")
    source = _subjects_of(messages)
    target_id = validate_name(episode_id, kind="episode id") if episode_id else _fresh_id()
    target = EpisodeSubjects(app=source.app, episode_id=target_id)
    if target.stream in set(await transport.list_streams()):
        raise EpisodeExistsError(f"episode id {target_id!r} is already in use")

    friendly = name or fallback_episode_name(target_id)
    metadata = EpisodeMetadata(
        app=source.app,
        name=friendly,
        status="ended",
        mode="replay",
        created_at=datetime.now(UTC).isoformat(),
    ).to_stream_metadata()
    await transport.ensure_stream(target.stream, [target.all_subjects], metadata=metadata)
    old_root, new_root = source.root, target.root
    for message in messages:
        subject, data = _rewire(message, target_id, old_root, new_root)
        await transport.publish(subject, data)
    await transport.seal_stream(target.stream)
    return ImportResult(app=source.app, episode_id=target_id, name=friendly)


def _rewire(
    message: ReplayMessage, target_id: str, old_root: str, new_root: str
) -> tuple[str, bytes]:
    """Re-publishable (subject, bytes) for one recorded message under a new id.

    The subject's episode root and the envelope's ``episode_id`` are both
    rewritten to *target_id*, so the imported episode is wholly self-consistent.
    A payload that was not JSON (a non-envelope message recorded raw) is
    re-published verbatim on its rewritten subject.
    """
    subject = new_root + message.subject[len(old_root) :]
    if message.payload is None:
        inner: object | None = None
    else:
        try:
            inner = json.loads(message.payload)
        except ValueError:
            return subject, message.payload.encode("utf-8")
    return subject, Envelope(episode_id=target_id, sender=message.sender, payload=inner).to_bytes()


def _fresh_id() -> str:
    return secrets.token_hex(_ID_BYTES)


async def _quiet_error_cb(_exc: Exception) -> None:
    """Swallow nats-py's per-attempt tracebacks during connect."""


async def _connect(nats_url: str) -> nats.NATS:
    try:
        return await nats.connect(
            nats_url, connect_timeout=5, max_reconnect_attempts=3, error_cb=_quiet_error_cb
        )
    except Exception as exc:
        raise EdgeIOError(f"cannot connect to NATS at {nats_url}. Is the server running?") from exc


# Re-exported so callers can catch the replayer's bad-log error alongside ours.
__all__ = ["EdgeIOError", "ImportResult", "ReplayerError", "export_episode", "import_episode"]
