"""Re-publish a recorded episode's Parquet log onto a NATS server.

Replay in FreeAgent is **NATS playback, not log reading** (ADR-0001): a viewer
subscribes only to NATS and must not be able to tell a live episode from a
replay, so it has exactly one code path. The replayer reads an episode's
Parquet log and re-publishes every message -- in ``stream_seq`` order, with
original or scaled inter-message timing -- onto a NATS server, intended to be a
**separate, local** one so replayed traffic never mixes with live traffic while
the subjects (and therefore the viewer config) stay byte-identical across modes.

The replayer is **app-agnostic**: the Parquet log is uniform across every
FreeAgent application (each row is just a ``subject``, a ``payload``, and a
``stream_seq``), so this one tool replays any app's episode and never needs to
know it is, say, Twenty Questions.

Each row stores the envelope's *inner* payload as JSON plus the ``episode_id``
and ``sender`` it travelled with (see :mod:`freeagent.recorder`); replay
reconstructs the wire :class:`~freeagent.envelope.Envelope` from those so a
subscriber sees the same messages it would have seen live. The regenerated
``message_id`` is fresh -- it is not recorded, and nothing downstream depends on
a particular value.

The transport controls -- pause, resume, seek, and a mutable speed -- live here
on :class:`Replayer`, where they are reusable beyond GUIs (ADR-0001). The
library-level ``free-agent replay`` command drives start and stop; pause and
seek are exposed as :class:`Replayer` methods for an embedding GUI to call and
are not wired to interactive CLI input in v1.
"""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from typing import TYPE_CHECKING

import pyarrow as pa
import pyarrow.parquet as pq

from freeagent.envelope import Envelope
from freeagent.subjects import EpisodeSubjects
from freeagent.transport import NatsTransport

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable, Iterable
    from datetime import datetime
    from pathlib import Path

    from freeagent.transport import Transport

#: The columns every episode Parquet log must carry (see the recorder's schema).
REQUIRED_COLUMNS: frozenset[str] = frozenset(
    {"episode_id", "stream_seq", "subject", "sender", "received_at", "payload"}
)


class ReplayerError(Exception):
    """A replay failure with a message suitable for stderr."""


def _validate_speed(speed: float) -> float:
    """Return *speed* if it is a positive multiplier, else raise ``ValueError``."""
    if speed <= 0:
        raise ValueError(f"speed must be greater than 0, got {speed!r}")
    return speed


@dataclass(frozen=True, slots=True)
class ReplayMessage:
    """One recorded message, ready to be re-published.

    The fields mirror the Parquet row. ``payload`` is the JSON serialization of
    the original envelope's inner payload (or, for a message that was not a
    parseable envelope, the raw text the recorder stored).
    """

    episode_id: str
    stream_seq: int
    subject: str
    sender: str
    received_at: datetime
    payload: str | None

    def to_wire(self) -> bytes:
        """Reconstruct the wire bytes a subscriber should receive.

        The common case rebuilds the :class:`~freeagent.envelope.Envelope` from
        ``episode_id``, ``sender``, and the JSON-decoded ``payload``. A payload
        that is not valid JSON was a non-envelope message the recorder captured
        raw, so it is republished verbatim.
        """
        if self.payload is None:
            inner = None
        else:
            try:
                inner = json.loads(self.payload)
            except ValueError:
                # Not JSON: a raw, non-envelope message recorded best-effort.
                return self.payload.encode("utf-8")
        return Envelope(episode_id=self.episode_id, sender=self.sender, payload=inner).to_bytes()


def load_episode(path: Path) -> list[ReplayMessage]:
    """Read an episode Parquet log into messages sorted by ``stream_seq``.

    ``stream_seq`` is the authoritative total order, so the rows are sorted by
    it regardless of how they sit in the file. Raises :class:`ReplayerError`
    when the file cannot be read or is missing a required column.
    """
    try:
        table = pq.read_table(path)
    except (OSError, pa.ArrowInvalid) as exc:
        raise ReplayerError(f"cannot read Parquet file {path}: {exc}") from exc
    missing = REQUIRED_COLUMNS.difference(table.column_names)
    if missing:
        raise ReplayerError(f"{path} is not an episode log: missing column(s) {sorted(missing)}")
    messages = [
        ReplayMessage(
            episode_id=row["episode_id"],
            stream_seq=row["stream_seq"],
            subject=row["subject"],
            sender=row["sender"],
            received_at=row["received_at"],
            payload=row["payload"],
        )
        for row in table.to_pylist()
    ]
    messages.sort(key=lambda message: message.stream_seq)
    return messages


def _subjects_for(subject: str) -> EpisodeSubjects:
    """Parse the ``<app>.episode.<id>`` an episode subject belongs to."""
    parts = subject.split(".")
    if len(parts) < 4 or parts[1] != "episode":
        raise ReplayerError(
            f"subject {subject!r} is not a FreeAgent episode subject "
            "('<app>.episode.<id>.<channel>')"
        )
    return EpisodeSubjects(app=parts[0], episode_id=parts[2])


async def _ensure_streams(transport: Transport, messages: Iterable[ReplayMessage]) -> None:
    """Create the JetStream stream for every episode root present in *messages*.

    Live play captures ``<app>.episode.<id>.>`` in one stream per episode, and
    publishing through JetStream is what lets a viewer subscribe at any moment
    and read from sequence 1 -- exactly as it does live. A log is normally one
    episode, but this handles any number of roots so the tool stays
    app-agnostic and total.
    """
    roots: dict[str, str] = {}
    for message in messages:
        subjects = _subjects_for(message.subject)
        roots.setdefault(subjects.stream, subjects.all_subjects)
    for stream, all_subjects in roots.items():
        await transport.ensure_stream(stream, [all_subjects])


class Replayer:
    """Drive playback of a loaded episode onto a transport, with controls.

    Construct with the messages (sorted by ``stream_seq``) and a connected
    transport, then :meth:`play`. Inter-message timing is derived from
    ``received_at`` and divided by :attr:`speed`; ``as_fast_as_possible``
    ignores timing entirely and publishes back-to-back.

    The controls -- :meth:`pause`/:meth:`resume`, :meth:`seek`, :meth:`stop`,
    and the mutable :attr:`speed` -- take effect at message boundaries. They are
    intended to be called from a GUI (or another coroutine) while :meth:`play`
    runs, or driven directly between calls. *sleep* is injectable so timing is
    unit-testable without real waits.
    """

    def __init__(
        self,
        messages: list[ReplayMessage],
        transport: Transport,
        *,
        speed: float = 1.0,
        as_fast_as_possible: bool = False,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ) -> None:
        self._messages = messages
        self._transport = transport
        self._speed = _validate_speed(speed)
        self._as_fast_as_possible = as_fast_as_possible
        self._sleep = sleep
        self._index = 0
        self._published = 0
        self._stopped = False
        # An asyncio.Event in the "set" state means "running"; clearing it pauses.
        self._unpaused = asyncio.Event()
        self._unpaused.set()

    @property
    def speed(self) -> float:
        """The playback speed multiplier (``2.0`` is twice as fast)."""
        return self._speed

    @speed.setter
    def speed(self, value: float) -> None:
        self._speed = _validate_speed(value)

    @property
    def index(self) -> int:
        """The index of the next message to publish."""
        return self._index

    @property
    def published(self) -> int:
        """How many messages have been published so far."""
        return self._published

    def pause(self) -> None:
        """Hold playback at the next message boundary until :meth:`resume`."""
        self._unpaused.clear()

    def resume(self) -> None:
        """Resume playback after a :meth:`pause`."""
        self._unpaused.set()

    def seek(self, index: int) -> None:
        """Move the playback position to *index* (clamped to the episode)."""
        self._index = max(0, min(index, len(self._messages)))

    def stop(self) -> None:
        """Stop playback for good; :meth:`play` returns at the next boundary."""
        self._stopped = True
        # Wake a paused play loop so it can observe the stop and return.
        self._unpaused.set()

    def _delay_before(self, index: int) -> float:
        """Seconds to wait before publishing message *index*, scaled by speed.

        Derived from the gap between this message's ``received_at`` and the
        previous one's. A non-positive gap (clock ties) yields no wait.
        """
        gap = (
            self._messages[index].received_at - self._messages[index - 1].received_at
        ).total_seconds()
        return max(0.0, gap) / self._speed

    async def play(self) -> int:
        """Publish from the current position to the end; return the count published.

        Honors pause/seek/stop at message boundaries. Returns when the episode
        is exhausted or :meth:`stop` is called.
        """
        while True:
            await self._unpaused.wait()
            if self._stopped:
                break
            index = self._index
            if index >= len(self._messages):
                break
            if not self._as_fast_as_possible and index > 0:
                delay = self._delay_before(index)
                if delay > 0:
                    await self._sleep(delay)
                if self._stopped:
                    break
                # A seek during the wait redirects us; re-evaluate from the top.
                if self._index != index:
                    continue
            message = self._messages[index]
            await self._transport.publish(message.subject, message.to_wire())
            self._published += 1
            self._index = index + 1
        return self._published


async def replay_episode(
    *,
    parquet_path: Path,
    nats_url: str,
    speed: float = 1.0,
    as_fast_as_possible: bool = False,
) -> int:
    """Read an episode log and re-publish it onto NATS; return the count published.

    Connects to *nats_url* (intended to be a separate, local nats-server),
    creates the episode's stream, and replays every message in ``stream_seq``
    order with timing scaled by *speed* (or ignored when *as_fast_as_possible*).
    Raises :class:`ReplayerError` on a bad log and
    :class:`~freeagent.transport.TransportError` when NATS is unreachable.
    """
    messages = load_episode(parquet_path)
    if not messages:
        return 0
    _validate_speed(speed)
    transport = NatsTransport(nats_url)
    await transport.connect()
    try:
        await _ensure_streams(transport, messages)
        replayer = Replayer(
            messages,
            transport,
            speed=speed,
            as_fast_as_possible=as_fast_as_possible,
        )
        return await replayer.play()
    finally:
        await transport.close()
