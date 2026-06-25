"""The per-episode feed: one read path for live and replay (ADR-0003).

For each open episode the UI watches, the service runs **one consumer** over the
episode's JetStream stream, reading from sequence 1 (history) and then tailing it
(live), and relays each message to the UI as a normalized **feed event** -- a
message was appended, the status/seal changed, or the connection's liveness
changed. The UI never parses a NATS envelope or subject.

**Live and replay are one path.** A sealed episode is just the same read with no
further appends: the feed streams its full history, marks the live boundary, and
closes. There is no second NATS server and no re-publish -- the
"an observer can't tell live from replay" invariant of ADR-0001 now holds at the
**Python -> UI** boundary.

The read is abstracted behind :class:`StreamSource` so the event logic
(:class:`EpisodeFeed`) is unit-testable without a server; :class:`NatsStreamSource`
is the JetStream-backed implementation.
"""

from __future__ import annotations

import contextlib
import logging
from typing import TYPE_CHECKING, NamedTuple, Protocol

import nats
import nats.errors
import nats.js.errors
from fastapi import WebSocket, WebSocketDisconnect

from freeagent.cli.apps import UnknownAppError
from freeagent.envelope import Envelope
from freeagent.subjects import EpisodeSubjects, channel_of

from .models import (
    EpisodeMessage,
    EpisodeView,
    FeedConnectionEvent,
    FeedMessageEvent,
    FeedStatusEvent,
)
from .registry import EpisodeNotFoundError

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable
    from datetime import datetime

    from fastapi import FastAPI

    from .registry import ControlService

logger = logging.getLogger("freeagent.service.feed")

#: How long the tail waits for the next message before checking liveness again.
#: A bound, not a deadline: a live feed loops on it indefinitely.
_TAIL_POLL = 1.0


class StreamRow(NamedTuple):
    """One message read from an episode stream, with its server metadata."""

    stream_seq: int
    subject: str
    received_at: datetime
    data: bytes


class StreamSource(Protocol):
    """A readable episode stream: history from sequence 1, then live appends."""

    async def last_seq(self) -> int:
        """The stream's current last sequence (0 when empty) -- the history target."""
        ...

    async def sealed(self) -> bool:
        """Whether the stream is now sealed (complete, no further appends)."""
        ...

    async def open(self) -> None:
        """Attach the consumer (reading from sequence 1)."""
        ...

    async def next(self, timeout: float) -> StreamRow | None:  # noqa: ASYNC109 -- the timeout is the tail poll bound, the point of the call
        """The next row, or ``None`` if none arrived within *timeout* seconds."""
        ...

    async def close(self) -> None:
        """Detach the consumer and release resources."""
        ...


def to_episode_message(row: StreamRow) -> EpisodeMessage:
    """Project one stream row onto the normalized feed message.

    Mirrors the recorder: parse the envelope for ``sender`` and ``payload``,
    derive the normalized ``channel`` from the subject, and keep the
    authoritative ``stream_seq`` and server ``received_at``. A non-envelope
    message is carried with an empty sender and its raw text as payload.
    """
    try:
        envelope = Envelope.from_bytes(row.data)
    except Exception:
        return EpisodeMessage(
            stream_seq=row.stream_seq,
            subject=row.subject,
            channel=channel_of(row.subject),
            sender="",
            received_at=row.received_at,
            payload=row.data.decode("utf-8", errors="replace"),
        )
    return EpisodeMessage(
        stream_seq=row.stream_seq,
        subject=row.subject,
        channel=channel_of(row.subject),
        sender=envelope.sender,
        received_at=row.received_at,
        payload=envelope.payload,
    )


def _status_event(view: EpisodeView) -> FeedStatusEvent:
    return FeedStatusEvent(
        status=view.status,
        sealed=view.sealed,
        name=view.name,
        outcome=view.outcome,
        detail=view.detail,
    )


class EpisodeFeed:
    """Drive one episode's feed: snapshot, history, the live boundary, then tail.

    *source* reads the stream; *snapshot* returns the current episode view (called
    at open and again when a live episode seals); *send* delivers one serialized
    feed event to the UI. The same logic serves a live episode and a sealed
    (replay) one -- a sealed episode simply has no appends past its history.
    """

    def __init__(
        self,
        source: StreamSource,
        snapshot: Callable[[], Awaitable[EpisodeView]],
        send: Callable[[str], Awaitable[None]],
    ) -> None:
        self._source = source
        self._snapshot = snapshot
        self._send = send

    async def run(self) -> None:
        """Stream the episode to the UI until it is exhausted or the UI leaves."""
        view = await self._snapshot()
        await self._emit(_status_event(view))
        await self._emit(FeedConnectionEvent(phase="open"))
        was_sealed = view.sealed
        target = await self._source.last_seq()
        await self._source.open()
        live = False
        if target == 0:
            await self._go_live()
            live = True
            if was_sealed:
                await self._close()
                return
        while True:
            row = await self._source.next(_TAIL_POLL)
            if row is None:
                if was_sealed:
                    # A sealed stream has no more rows once history is drained.
                    await self._close()
                    return
                if await self._source.sealed():
                    # A live episode just ended and sealed: report it, then close.
                    await self._emit(_status_event(await self._snapshot()))
                    await self._close()
                    return
                continue
            await self._emit(FeedMessageEvent(message=to_episode_message(row)))
            if not live and row.stream_seq >= target:
                await self._go_live()
                live = True
                if was_sealed:
                    await self._close()
                    return

    async def _go_live(self) -> None:
        await self._emit(FeedConnectionEvent(phase="live"))

    async def _close(self) -> None:
        await self._emit(FeedConnectionEvent(phase="closed"))

    async def _emit(self, event: FeedMessageEvent | FeedStatusEvent | FeedConnectionEvent) -> None:
        await self._send(event.model_dump_json())


class NatsStreamSource:
    """A :class:`StreamSource` over a real JetStream stream.

    Like the recorder, this is service infrastructure that legitimately sees raw
    subjects and JetStream metadata. It owns its own NATS connection (one per open
    feed) and an ordered consumer reading from sequence 1.
    """

    def __init__(self, nats_url: str, app: str, episode_id: str) -> None:
        self._url = nats_url
        self._subjects = EpisodeSubjects(app=app, episode_id=episode_id)
        self._client: nats.NATS | None = None
        self._subscription: object | None = None

    async def _connect(self) -> nats.NATS:
        if self._client is None:
            self._client = await nats.connect(self._url, max_reconnect_attempts=3)
        return self._client

    async def last_seq(self) -> int:
        client = await self._connect()
        try:
            info = await client.jetstream().stream_info(self._subjects.stream)
        except nats.js.errors.NotFoundError:
            return 0
        return info.state.last_seq

    async def sealed(self) -> bool:
        client = await self._connect()
        try:
            info = await client.jetstream().stream_info(self._subjects.stream)
        except nats.js.errors.NotFoundError:
            return False
        return bool(info.config.sealed)

    async def open(self) -> None:
        client = await self._connect()
        js = client.jetstream()
        from nats.js.api import ConsumerConfig, DeliverPolicy

        self._subscription = await js.subscribe(
            self._subjects.all_subjects,
            stream=self._subjects.stream,
            ordered_consumer=True,
            config=ConsumerConfig(deliver_policy=DeliverPolicy.ALL),
        )

    async def next(self, timeout: float) -> StreamRow | None:  # noqa: ASYNC109 -- the timeout is the tail poll bound, the point of the call
        if self._subscription is None:
            raise RuntimeError("NatsStreamSource.open() must be called before next()")
        try:
            message = await self._subscription.next_msg(timeout=timeout)  # type: ignore[attr-defined]
        except nats.errors.TimeoutError:
            return None
        meta = message.metadata
        return StreamRow(
            stream_seq=meta.sequence.stream,
            subject=message.subject,
            received_at=meta.timestamp,
            data=message.data,
        )

    async def close(self) -> None:
        if self._subscription is not None:
            try:
                await self._subscription.unsubscribe()  # type: ignore[attr-defined]
            except Exception:
                logger.debug("feed: error unsubscribing", exc_info=True)
            self._subscription = None
        if self._client is not None:
            await self._client.close()
            self._client = None


def register_feed_route(app: FastAPI) -> None:
    """Register ``WS /freeagent/{application}/episodes/{episode}/feed`` on *app*.

    The browser opens this socket to watch an episode; the service streams the
    full history then live appends as normalized feed events. A missing episode
    is reported with a single ``connection``/``error`` event before the socket
    closes, rather than a bare protocol error.
    """

    @app.websocket("/freeagent/{application}/episodes/{episode_id}/feed")
    async def feed(websocket: WebSocket, application: str, episode_id: str) -> None:
        await websocket.accept()
        control: ControlService = websocket.app.state.service

        async def snapshot() -> EpisodeView:
            return await control.get(application, episode_id)

        try:
            view = await snapshot()
        except (EpisodeNotFoundError, UnknownAppError) as exc:
            await websocket.send_text(
                FeedConnectionEvent(phase="error", detail=str(exc)).model_dump_json()
            )
            await websocket.close()
            return

        source = NatsStreamSource(control.default_nats_url, view.app, episode_id)
        try:
            await EpisodeFeed(source, snapshot, websocket.send_text).run()
        except WebSocketDisconnect:
            pass
        except Exception:
            logger.warning("feed: error streaming episode %s", episode_id, exc_info=True)
        finally:
            await source.close()
            with contextlib.suppress(Exception):
                await websocket.close()
