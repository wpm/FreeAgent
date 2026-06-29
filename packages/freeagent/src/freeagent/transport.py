"""Transport abstraction over NATS.

A small async protocol with two implementations:

- :class:`NatsTransport` -- nats-py with JetStream. Consumers read each
  episode's stream from sequence 1 (``DeliverPolicy.ALL``, ordered consumers),
  so nothing can be missed, only delayed.
- :class:`MemoryTransport` -- in-process, for unit tests. No network. It keeps
  the full publish history and replays it to new subscribers, mirroring
  JetStream consumers that read from sequence 1.

Agents and environments run against the abstraction, so the fold, the outbox,
the lifecycle state machine, and request/reply are all unit-testable without
a NATS server.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
from collections.abc import Awaitable, Callable, Sequence
from typing import TYPE_CHECKING, Protocol, runtime_checkable

import nats
from nats.errors import NoServersError
from nats.errors import TimeoutError as NatsTimeoutError
from nats.js.api import (
    AckPolicy,
    ConsumerConfig,
    DeliverPolicy,
    RetentionPolicy,
    StreamConfig,
)
from nats.js.errors import BadRequestError, NotFoundError

if TYPE_CHECKING:
    from nats.aio.client import Client as NatsClient
    from nats.js import JetStreamContext

logger = logging.getLogger("freeagent.transport")

MessageHandler = Callable[[str, bytes], Awaitable[None]]
"""Async callback receiving ``(subject, data)`` for each delivered message."""


class TransportError(RuntimeError):
    """A transport-level failure with an operator-facing message.

    Raised in place of nats-py's low-level connection errors so the runner can
    print one clear line instead of a connection-refused traceback.
    """


class SealedStreamError(TransportError):
    """A publish was attempted to a sealed (complete, immutable) episode stream.

    Sealing (ADR-0003) makes a finished episode immutable: it is still readable
    and deletable as a whole, but rejects further appends. JetStream enforces
    this server-side; :class:`MemoryTransport` models the same rejection so the
    seal is unit-testable without a server.
    """


@runtime_checkable
class Subscription(Protocol):
    """Handle for one active subscription."""

    async def unsubscribe(self) -> None:
        """Stop delivery and release resources."""
        ...


@runtime_checkable
class PulledMessage(Protocol):
    """One manifest pulled off the work queue, acked by the worker on confirmed launch.

    A worker fetches a batch of these from its :class:`PullSubscription`, spawns
    the child the manifest describes, and -- only once the child has survived its
    connect phase -- :meth:`ack`-s it. A child that fails to start is
    :meth:`nak`-ed so the manifest is redelivered to another worker; a manifest
    that can never be launched is :meth:`term`-ed so it is not redelivered.
    """

    @property
    def data(self) -> bytes:
        """The raw manifest bytes (``Manifest.model_validate_json`` parses them)."""
        ...

    async def ack(self) -> None:
        """Acknowledge the message: the work is claimed and will not be redelivered."""
        ...

    async def nak(self) -> None:
        """Negatively acknowledge: redeliver the message to a worker (incl. this one)."""
        ...

    async def term(self) -> None:
        """Terminate the message: stop redelivering it (it can never be launched)."""
        ...


@runtime_checkable
class PullSubscription(Protocol):
    """A bound durable pull consumer a worker fetches manifest batches from."""

    async def fetch(self, batch: int, timeout: float) -> Sequence[PulledMessage]:  # noqa: ASYNC109
        """Pull up to *batch* messages, waiting at most *timeout* seconds.

        Returns the empty sequence when no message arrives in the window rather
        than raising, so the worker's pull loop can simply poll again. The
        ``timeout`` parameter mirrors nats-py's ``PullSubscription.fetch``.
        """
        ...


@runtime_checkable
class Transport(Protocol):
    """The async messaging surface FreeAgent runs on."""

    async def connect(self) -> None:
        """Establish the connection (no-op for in-memory)."""
        ...

    async def close(self) -> None:
        """Tear down the connection and all subscriptions."""
        ...

    async def ensure_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        """Idempotently create the stream capturing *subjects*, with *metadata*."""
        ...

    async def ensure_work_queue_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        """Idempotently create a work-queue-retention stream capturing *subjects*.

        Unlike :meth:`ensure_stream` (limits retention -- a durable log), this
        creates a stream whose messages are *removed once acknowledged*: the
        shared manifest queue of ADR-0005, where each enqueued unit of work is
        delivered to exactly one worker. Idempotent: an already-existing stream
        of this name is left in place (its metadata merged).
        """
        ...

    async def ensure_work_consumer(self, stream: str, durable: str, *, ack_wait: float) -> None:
        """Idempotently create the one shared durable pull consumer on *stream*.

        Every worker binds this same durable name (ADR-0005), so a manifest is
        delivered to exactly one of them. ``ack_wait`` covers startup only.
        """
        ...

    async def pull_subscribe(
        self, stream: str, durable: str, subjects: Sequence[str]
    ) -> PullSubscription:
        """Bind the shared durable pull consumer and return a fetchable handle."""
        ...

    async def publish(self, subject: str, data: bytes) -> None:
        """Publish raw bytes to a subject.

        Raises :class:`SealedStreamError` if the subject's stream is sealed.
        """
        ...

    async def subscribe(self, subject: str, handler: MessageHandler) -> Subscription:
        """Subscribe to a subject (wildcards allowed), delivering from sequence 1."""
        ...

    async def list_streams(self) -> Sequence[str]:
        """The names of every stream the server knows (the durable record)."""
        ...

    async def delete_stream(self, name: str) -> None:
        """Delete the whole stream (the episode and all its messages)."""
        ...

    async def stream_metadata(self, name: str) -> dict[str, str]:
        """The stream's current metadata (empty when it carries none)."""
        ...

    async def set_stream_metadata(self, name: str, metadata: dict[str, str]) -> None:
        """Merge *metadata* into the stream's existing metadata (read-modify-write)."""
        ...

    async def seal_stream(self, name: str) -> None:
        """Seal the stream: make it immutable, rejecting all further appends."""
        ...

    async def stream_sealed(self, name: str) -> bool:
        """Whether the stream is sealed (complete and immutable)."""
        ...


# ---------------------------------------------------------------------------
# In-memory transport (unit tests)
# ---------------------------------------------------------------------------


def subject_matches(pattern: str, subject: str) -> bool:
    """NATS subject matching: ``*`` matches one token, ``>`` the rest (one or more)."""
    pattern_tokens = pattern.split(".")
    subject_tokens = subject.split(".")
    for i, token in enumerate(pattern_tokens):
        if token == ">":
            return len(subject_tokens) >= i + 1
        if i >= len(subject_tokens):
            return False
        if token not in ("*", subject_tokens[i]):
            return False
    return len(pattern_tokens) == len(subject_tokens)


class _MemorySubscription:
    """One in-memory subscription with its own delivery queue and pump task.

    Delivery is asynchronous (like NATS): ``publish`` never invokes handlers
    inline, it only enqueues.
    """

    def __init__(self, transport: MemoryTransport, pattern: str, handler: MessageHandler) -> None:
        self._transport = transport
        self.pattern = pattern
        self._handler = handler
        self._queue: asyncio.Queue[tuple[str, bytes]] = asyncio.Queue()
        self._task = asyncio.get_running_loop().create_task(self._pump())

    def deliver(self, subject: str, data: bytes) -> None:
        self._queue.put_nowait((subject, data))

    async def _pump(self) -> None:
        while True:
            subject, data = await self._queue.get()
            try:
                await self._handler(subject, data)
            except Exception:
                logger.exception("memory transport: handler failed for subject %s", subject)

    async def unsubscribe(self) -> None:
        if self in self._transport._subscriptions:
            self._transport._subscriptions.remove(self)
        self._task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await self._task


class _MemoryPulledMessage:
    """One in-memory pulled manifest, returning to its queue if not acked.

    Mirrors a JetStream work-queue message: :meth:`ack`/:meth:`term` remove it
    for good; :meth:`nak` returns it to the front of the pending queue so the
    next :meth:`~_MemoryPullQueue.fetch` redelivers it (the worker's redelivery
    path when a child fails to start).
    """

    def __init__(self, queue: _MemoryPullQueue, data: bytes) -> None:
        self._queue = queue
        self.data = data

    async def ack(self) -> None:
        self._queue.resolve(self)

    async def term(self) -> None:
        self._queue.resolve(self)

    async def nak(self) -> None:
        self._queue.redeliver(self)


class _MemoryPullQueue:
    """The pending+in-flight FIFO behind one in-memory work-queue stream."""

    def __init__(self) -> None:
        self.pending: list[bytes] = []
        self._in_flight: set[_MemoryPulledMessage] = set()

    def enqueue(self, data: bytes) -> None:
        self.pending.append(data)

    def fetch(self, batch: int) -> list[_MemoryPulledMessage]:
        taken: list[_MemoryPulledMessage] = []
        while self.pending and len(taken) < batch:
            message = _MemoryPulledMessage(self, self.pending.pop(0))
            self._in_flight.add(message)
            taken.append(message)
        return taken

    def resolve(self, message: _MemoryPulledMessage) -> None:
        self._in_flight.discard(message)  # acked/termed: gone for good

    def redeliver(self, message: _MemoryPulledMessage) -> None:
        if message in self._in_flight:
            self._in_flight.discard(message)
            self.pending.insert(0, message.data)  # naked: back to the front


class _MemoryPullSubscription:
    """A fetchable handle bound to one in-memory work-queue (stream, durable)."""

    def __init__(self, queue: _MemoryPullQueue) -> None:
        self._queue = queue

    async def fetch(self, batch: int, timeout: float) -> Sequence[PulledMessage]:  # noqa: ASYNC109
        messages = self._queue.fetch(batch)
        if not messages:
            # Nothing pending: model the real consumer's empty-window wait so a
            # polling loop yields instead of spinning. Bounded by *timeout*.
            await asyncio.sleep(min(timeout, 0.01))
        return messages


class MemoryTransport:
    """In-process transport for unit tests -- no network, no NATS.

    The full publish ``history`` stands in for the episode's JetStream
    stream: new subscribers replay all matching past messages first, exactly
    as a JetStream consumer reading from sequence 1 would.
    """

    def __init__(self) -> None:
        self._subscriptions: list[_MemorySubscription] = []
        self.history: list[tuple[str, bytes]] = []
        self.streams: dict[str, tuple[str, ...]] = {}
        self.stream_meta: dict[str, dict[str, str]] = {}
        self.sealed_streams: set[str] = set()
        #: Names of streams created with work-queue retention (ADR-0005). Lets a
        #: test assert the recruiter built the queue with the right retention,
        #: even though publish/subscribe semantics here do not model removal.
        self.work_queue_streams: set[str] = set()
        #: One pending+in-flight FIFO per work-queue stream, so the worker's
        #: pull -> ack/nak loop is unit-testable without a server.
        self._work_queues: dict[str, _MemoryPullQueue] = {}

    async def connect(self) -> None:
        return None

    async def close(self) -> None:
        for sub in list(self._subscriptions):
            await sub.unsubscribe()

    async def ensure_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        self.streams[name] = tuple(subjects)
        if metadata:
            self.stream_meta.setdefault(name, {}).update(metadata)

    async def ensure_work_queue_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        # Work-queue retention is a server-side delivery property (a message is
        # removed once acked); it does not change the in-memory replay model, so
        # we record the stream exactly like ensure_stream and just flag it as a
        # work queue for tests to assert against. Idempotent: re-ensuring keeps
        # the existing subjects (a real server leaves the stream in place).
        if name not in self.streams:
            self.streams[name] = tuple(subjects)
        if metadata:
            self.stream_meta.setdefault(name, {}).update(metadata)
        self.work_queue_streams.add(name)
        self._work_queues.setdefault(name, _MemoryPullQueue())

    async def ensure_work_consumer(
        self,
        stream: str,
        durable: str,  # noqa: ARG002 - server-side delivery concern, unused in-memory
        *,
        ack_wait: float,  # noqa: ARG002 - server-side delivery concern, unused in-memory
    ) -> None:
        # Consumers share the stream's one queue; binding is a no-op beyond
        # ensuring the queue exists (idempotent, like the real durable consumer).
        self._work_queues.setdefault(stream, _MemoryPullQueue())

    async def pull_subscribe(
        self,
        stream: str,
        durable: str,  # noqa: ARG002 - the durable name is bound server-side
        subjects: Sequence[str],  # noqa: ARG002 - the subject filter is bound server-side
    ) -> PullSubscription:
        # One shared queue per stream stands in for the durable consumer.
        return _MemoryPullSubscription(self._work_queues.setdefault(stream, _MemoryPullQueue()))

    async def publish(self, subject: str, data: bytes) -> None:
        if self._sealed_stream_for(subject) is not None:
            raise SealedStreamError(f"cannot publish to {subject}: episode is sealed")
        self.history.append((subject, data))
        # Route into any work-queue stream capturing this subject: a work-queue
        # message is pulled+acked, not replayed to push subscribers.
        for name, patterns in self.streams.items():
            if name in self._work_queues and any(
                subject_matches(pattern, subject) for pattern in patterns
            ):
                self._work_queues[name].enqueue(data)
        for sub in list(self._subscriptions):
            if subject_matches(sub.pattern, subject):
                sub.deliver(subject, data)

    async def subscribe(self, subject: str, handler: MessageHandler) -> Subscription:
        sub = _MemorySubscription(self, subject, handler)
        for past_subject, data in self.history:
            if subject_matches(subject, past_subject):
                sub.deliver(past_subject, data)
        self._subscriptions.append(sub)
        return sub

    async def list_streams(self) -> Sequence[str]:
        return list(self.streams)

    async def delete_stream(self, name: str) -> None:
        self.streams.pop(name, None)
        self.stream_meta.pop(name, None)
        self.sealed_streams.discard(name)
        self.work_queue_streams.discard(name)
        self._work_queues.pop(name, None)

    async def stream_metadata(self, name: str) -> dict[str, str]:
        return dict(self.stream_meta.get(name, {}))

    async def set_stream_metadata(self, name: str, metadata: dict[str, str]) -> None:
        self.stream_meta.setdefault(name, {}).update(metadata)

    async def seal_stream(self, name: str) -> None:
        self.sealed_streams.add(name)

    async def stream_sealed(self, name: str) -> bool:
        return name in self.sealed_streams

    def _sealed_stream_for(self, subject: str) -> str | None:
        """The sealed stream capturing *subject*, if any (mirrors JetStream)."""
        for name in self.sealed_streams:
            patterns = self.streams.get(name, ())
            if any(subject_matches(pattern, subject) for pattern in patterns):
                return name
        return None


# ---------------------------------------------------------------------------
# NATS + JetStream transport
# ---------------------------------------------------------------------------

# Connection retries before connect() gives up with a TransportError. A few
# attempts ride out a transient blip; we still fail fast (and quietly) when no
# server is there, rather than nats-py's unbounded reconnect loop.
_CONNECT_ATTEMPTS = 3


async def _quiet_error_cb(_exc: Exception) -> None:
    """No-op nats-py error callback: suppress its per-attempt tracebacks.

    nats-py's default callback logs every failed (re)connection attempt at
    ERROR with a full traceback. During startup against a missing server that
    is just noise; the single :class:`TransportError` from :meth:`connect` is
    the actionable message.
    """


class _NatsSubscription:
    def __init__(self, inner: object) -> None:
        self._inner = inner

    async def unsubscribe(self) -> None:
        await self._inner.unsubscribe()  # type: ignore[attr-defined]


class _NatsPullSubscription:
    """Wraps nats-py's ``PullSubscription`` as a :class:`PullSubscription`.

    The one behavioural shim is ``fetch``: nats-py raises a timeout when a batch
    window elapses with no message; the worker's loop wants that to be an ordinary
    empty poll, so we translate it into an empty list. nats-py raises *two*
    different timeout types here -- the ``batch == 1`` path raises
    ``nats.errors.TimeoutError`` while the batched ``_fetch_n`` path raises stdlib
    ``asyncio.TimeoutError`` -- so we must catch both, or an idle worker pulling a
    batch crashes instead of looping.
    """

    def __init__(self, inner: object) -> None:
        self._inner = inner

    async def fetch(self, batch: int, timeout: float) -> Sequence[PulledMessage]:  # noqa: ASYNC109
        try:
            msgs = await self._inner.fetch(batch, timeout=timeout)  # type: ignore[attr-defined]
        except (NatsTimeoutError, TimeoutError):
            return []
        return list(msgs)


class NatsTransport:
    """Transport over a NATS server with JetStream enabled.

    Publishes through JetStream (acked into the episode stream) and
    subscribes with ordered consumers reading from sequence 1
    (``DeliverPolicy.ALL``), per the coordinated-startup design: nothing can
    be missed, only delayed.
    """

    def __init__(self, url: str, *, stream_wait: float = 30.0) -> None:
        self._url = url
        self._stream_wait = stream_wait
        self._nc: NatsClient | None = None
        self._js: JetStreamContext | None = None

    def _jetstream(self) -> JetStreamContext:
        if self._js is None:
            raise RuntimeError("NatsTransport is not connected; call connect() first")
        return self._js

    async def connect(self) -> None:
        try:
            self._nc = await nats.connect(
                self._url,
                max_reconnect_attempts=_CONNECT_ATTEMPTS,
                # Swallow nats-py's per-attempt "encountered error" tracebacks;
                # a failed connect surfaces as one TransportError below.
                error_cb=_quiet_error_cb,
            )
        except (NoServersError, OSError) as exc:
            raise TransportError(
                f"could not connect to NATS at {self._url}. "
                "Is the server running? Start it with: "
                "docker compose -f docker/nats/docker-compose.yml up -d"
            ) from exc
        self._js = self._nc.jetstream()

    async def close(self) -> None:
        if self._nc is not None and not self._nc.is_closed:
            await self._nc.drain()
        self._nc = None
        self._js = None

    async def ensure_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        js = self._jetstream()
        try:
            await js.add_stream(
                StreamConfig(name=name, subjects=list(subjects), metadata=metadata or None)
            )
        except BadRequestError:
            # The stream already exists (possibly created concurrently with a
            # config the server reports as conflicting); confirm it is there and
            # apply any metadata the caller asked for.
            await js.stream_info(name)
            if metadata:
                await self.set_stream_metadata(name, metadata)

    async def ensure_work_queue_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        js = self._jetstream()
        try:
            await js.add_stream(
                StreamConfig(
                    name=name,
                    subjects=list(subjects),
                    retention=RetentionPolicy.WORK_QUEUE,
                    metadata=metadata or None,
                )
            )
        except BadRequestError:
            # The stream already exists (the queue is long-lived and shared by
            # every episode and every recruiter); confirm it is there and apply
            # any metadata the caller asked for. We do not re-assert retention:
            # a server reports a retention change on an existing stream as an
            # error, and the queue's retention is fixed at creation.
            await js.stream_info(name)
            if metadata:
                await self.set_stream_metadata(name, metadata)

    async def publish(self, subject: str, data: bytes) -> None:
        try:
            await self._jetstream().publish(subject, data)
        except BadRequestError as exc:
            # A sealed stream rejects appends; surface it as the typed error.
            raise SealedStreamError(f"cannot publish to {subject}: episode is sealed") from exc

    async def list_streams(self) -> Sequence[str]:
        infos = await self._jetstream().streams_info()
        return [info.config.name for info in infos if info.config.name is not None]

    async def delete_stream(self, name: str) -> None:
        try:
            await self._jetstream().delete_stream(name)
        except NotFoundError:
            return  # already gone: deletion is idempotent

    async def stream_metadata(self, name: str) -> dict[str, str]:
        info = await self._jetstream().stream_info(name)
        return dict(info.config.metadata or {})

    async def set_stream_metadata(self, name: str, metadata: dict[str, str]) -> None:
        js = self._jetstream()
        info = await js.stream_info(name)
        config = info.config
        config.metadata = {**(config.metadata or {}), **metadata}
        await js.update_stream(config)

    async def seal_stream(self, name: str) -> None:
        js = self._jetstream()
        info = await js.stream_info(name)
        config = info.config
        config.sealed = True
        await js.update_stream(config)

    async def stream_sealed(self, name: str) -> bool:
        info = await self._jetstream().stream_info(name)
        return bool(info.config.sealed)

    async def subscribe(self, subject: str, handler: MessageHandler) -> Subscription:
        js = self._jetstream()

        async def callback(msg: object) -> None:
            await handler(msg.subject, msg.data)  # type: ignore[attr-defined]

        config = ConsumerConfig(deliver_policy=DeliverPolicy.ALL)
        deadline = asyncio.get_running_loop().time() + self._stream_wait
        while True:
            try:
                inner = await js.subscribe(
                    subject, cb=callback, ordered_consumer=True, config=config
                )
            except NotFoundError:
                # The episode's stream may not exist yet (the environment
                # creates it during setup); wait for it rather than failing.
                if asyncio.get_running_loop().time() >= deadline:
                    raise
                await asyncio.sleep(0.1)
            else:
                return _NatsSubscription(inner)

    # -- work-queue pull consumer (ADR-0005) -----------------------------------
    #
    # The recruiter's ``ensure_work_queue_stream`` (above) creates the shared
    # work-queue stream; these two methods are the *consumer* side the worker
    # uses. The work queue is read through one shared durable PULL consumer (not
    # the per-episode ordered push consumers ``subscribe`` uses): every worker
    # binds the same durable name, so a manifest is delivered to exactly one of
    # them. Workers pull manifests, launch them, and ack on confirmed creation.

    async def ensure_work_consumer(self, stream: str, durable: str, *, ack_wait: float) -> None:
        """Idempotently create the ONE shared durable pull consumer on *stream*.

        Every worker binds this same durable consumer -- never a per-instance one,
        which would fan every manifest to every worker. ``ack_wait`` covers
        *startup only*: the worker acks on confirmed child creation (seconds), so
        a short window suffices; a child that dies before its ack is redelivered.
        Explicit acks make that redelivery the worker's decision, not a timeout's.
        """
        js = self._jetstream()
        config = ConsumerConfig(
            durable_name=durable,
            ack_policy=AckPolicy.EXPLICIT,
            ack_wait=ack_wait,
        )
        try:
            await js.add_consumer(stream, config)
        except BadRequestError:
            # Already exists (every worker ensures it); confirm and move on.
            await js.consumer_info(stream, durable)

    async def pull_subscribe(
        self, stream: str, durable: str, subjects: Sequence[str]
    ) -> PullSubscription:
        """Bind the shared durable pull consumer and return a fetchable handle.

        Binds (does not create) the durable consumer :meth:`ensure_work_consumer`
        made, so every worker shares it. *subjects* is the work queue's capture
        wildcard (``WORK_QUEUE_SUBJECTS``); a single shared consumer reads them
        all. The returned :class:`PullSubscription` fetches batches sized to the
        worker's spare capacity.
        """
        js = self._jetstream()
        # nats-py binds a pull subscription to one subject filter; the work queue
        # uses one wildcard, so the first (and only) subject is that wildcard.
        subject = next(iter(subjects))
        inner = await js.pull_subscribe(subject, durable=durable, stream=stream)
        return _NatsPullSubscription(inner)
