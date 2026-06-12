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
from nats.js.api import ConsumerConfig, DeliverPolicy, StreamConfig
from nats.js.errors import BadRequestError, NotFoundError

if TYPE_CHECKING:
    from nats.aio.client import Client as NatsClient
    from nats.js import JetStreamContext

logger = logging.getLogger("freeagent.transport")

MessageHandler = Callable[[str, bytes], Awaitable[None]]
"""Async callback receiving ``(subject, data)`` for each delivered message."""


@runtime_checkable
class Subscription(Protocol):
    """Handle for one active subscription."""

    async def unsubscribe(self) -> None:
        """Stop delivery and release resources."""
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

    async def ensure_stream(self, name: str, subjects: Sequence[str]) -> None:
        """Idempotently create the stream capturing *subjects*."""
        ...

    async def publish(self, subject: str, data: bytes) -> None:
        """Publish raw bytes to a subject."""
        ...

    async def subscribe(self, subject: str, handler: MessageHandler) -> Subscription:
        """Subscribe to a subject (wildcards allowed), delivering from sequence 1."""
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

    async def connect(self) -> None:
        return None

    async def close(self) -> None:
        for sub in list(self._subscriptions):
            await sub.unsubscribe()

    async def ensure_stream(self, name: str, subjects: Sequence[str]) -> None:
        self.streams[name] = tuple(subjects)

    async def publish(self, subject: str, data: bytes) -> None:
        self.history.append((subject, data))
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


# ---------------------------------------------------------------------------
# NATS + JetStream transport
# ---------------------------------------------------------------------------


class _NatsSubscription:
    def __init__(self, inner: object) -> None:
        self._inner = inner

    async def unsubscribe(self) -> None:
        await self._inner.unsubscribe()  # type: ignore[attr-defined]


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
        self._nc = await nats.connect(self._url)
        self._js = self._nc.jetstream()

    async def close(self) -> None:
        if self._nc is not None and not self._nc.is_closed:
            await self._nc.drain()
        self._nc = None
        self._js = None

    async def ensure_stream(self, name: str, subjects: Sequence[str]) -> None:
        js = self._jetstream()
        try:
            await js.add_stream(StreamConfig(name=name, subjects=list(subjects)))
        except BadRequestError:
            # The stream already exists (possibly created concurrently with a
            # config the server reports as conflicting); confirm it is there.
            await js.stream_info(name)

    async def publish(self, subject: str, data: bytes) -> None:
        await self._jetstream().publish(subject, data)

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
