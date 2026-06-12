"""The Agent base class: a fold over a single, locally ordered event stream.

An agent's life is a fold: in-world messages, control messages, and internal
"think" messages are merged into one locally ordered sequence and processed
**one handler at a time** on one asyncio loop -- no threads. Same state plus
the same event sequence yields the same new state and the same outbox, which
makes unit testing and replay exact. This local event sequence is exactly the
agent's RL trajectory.

The hard rule that makes this work: **handlers are fast and non-blocking.**
An LLM call never happens inside a handler. Slow work is spawned as an
asyncio task (:meth:`Agent.spawn`) whose completion is posted back to the
think queue (:meth:`Agent.think`) as an ordinary internal message. All the
messy asynchrony lives between folds; the fold itself stays simple.

The outbox: :meth:`Agent.act` does not publish. It appends to an outbox that
the runtime flushes only after the handler returns; if a handler raises, the
outbox it accumulated is **discarded** (and the error logged), so a handler
that acts and then fails publishes nothing. The think queue is *not* behind
the outbox -- :meth:`Agent.think` writes the internal queue directly; it is
internal state, not an effect on the world.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Literal

from pydantic import ValidationError

from .config import DEFAULT_GRACE_PERIOD
from .envelope import Envelope
from .subjects import ENV_NAME, EpisodeSubjects, validate_name
from .transport import MessageHandler, NatsTransport, Subscription, Transport

if TYPE_CHECKING:
    from collections.abc import Coroutine, Iterable, Mapping

logger = logging.getLogger("freeagent.agent")

Channel = Literal["public", "inbox", "control"]


@dataclass(slots=True)
class MessageEvent:
    """An external message entering the agent's fold."""

    channel: Channel
    envelope: Envelope


@dataclass(slots=True)
class ThinkEvent:
    """An internal self-addressed message entering the agent's fold."""

    payload: Any = field(default=None)


@dataclass(slots=True)
class StopEvent:
    """Internal sentinel: the wind-down grace period has expired; run() returns."""


AgentEvent = MessageEvent | ThinkEvent | StopEvent

Phase = Literal["idle", "active", "stopping"]

_MANAGEMENT_PREFIX = "freeagent."


def _management_type(payload: Any) -> str | None:
    """Return the ``freeagent.*`` type of a framework management payload, if any."""
    if isinstance(payload, dict):
        mtype = payload.get("type")
        if isinstance(mtype, str) and mtype.startswith(_MANAGEMENT_PREFIX):
            return mtype
    return None


class Agent:
    """Base class for FreeAgent agents.

    An application minimally defines :meth:`perceive`. Handlers
    (:meth:`perceive`, :meth:`control`, :meth:`handle_think`) must be **fast
    and non-blocking**: an LLM call never happens inside a handler; slow work
    is a :meth:`spawn`-ed task whose completion re-enters the fold as a think
    message.

    Agents launch idle and wait for the environment's ``start`` broadcast;
    they never publish on the control subject -- they only obey it, and the
    runtime (not user code) answers the environment's presence request.
    """

    def __init__(
        self,
        subject_root: str,
        agent_id: str,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        """Create an agent for one episode.

        ``subject_root`` is the episode's ``<app>.episode.<id>`` root;
        ``agent_id`` must match ``[A-Za-z0-9_-]+`` and may not be the
        reserved name ``"env"``. ``config`` is application-defined; the
        framework understands ``grace_period`` (seconds of wind-down after a
        ``shutdown`` control message, default 5).
        """
        validate_name(agent_id, kind="agent id")
        if agent_id == ENV_NAME:
            raise ValueError(f"agent id {ENV_NAME!r} is reserved for the environment")
        self._subjects = EpisodeSubjects.from_root(subject_root)
        self._id = agent_id
        self.config: dict[str, Any] = dict(config or {})
        # Per-process nonce: lets the environment detect two processes
        # launched under the same agent name.
        self._nonce = uuid.uuid4().hex
        self._events: asyncio.Queue[AgentEvent] = asyncio.Queue()
        self._outbox: list[tuple[Any, tuple[str, ...] | None]] = []
        self._tasks: set[asyncio.Task[Any]] = set()
        self._transport: Transport | None = None
        self._phase: Phase = "idle"

    # ------------------------------------------------------------------
    # Identity
    # ------------------------------------------------------------------

    @property
    def id(self) -> str:
        """The agent's name, unique within the episode (assigned top-down)."""
        return self._id

    @property
    def episode_id(self) -> str:
        """The episode this agent participates in."""
        return self._subjects.episode_id

    @property
    def phase(self) -> Phase:
        """Lifecycle phase: ``idle`` until ``start``, ``active``, then ``stopping``."""
        return self._phase

    # ------------------------------------------------------------------
    # Handlers (the fold) -- override these. Fast and non-blocking only.
    # ------------------------------------------------------------------

    async def perceive(self, message: Envelope) -> None:
        """Handle one in-world message (broadcast or addressed to this agent).

        Must be fast and non-blocking: never call an LLM (or do any slow
        work) here -- :meth:`spawn` a task and let its completion re-enter
        the fold via :meth:`think`. Default: ignore.
        """

    async def control(self, message: Envelope) -> None:
        """Handle one lifecycle message from the environment.

        The base implementation handles ``{"type": "start"}`` (idle ->
        active) and ``{"type": "shutdown"}`` (begin wind-down: the agent gets
        the stopping grace period to finish, then :meth:`run` returns).
        Overrides **must call** ``await super().control(message)``. Must be
        fast and non-blocking. Agents never *send* control messages.
        """
        mtype = message.payload.get("type") if isinstance(message.payload, dict) else None
        if mtype == "start":
            if self._phase == "idle":
                self._phase = "active"
        elif mtype == "shutdown":
            self._begin_wind_down()

    async def handle_think(self, payload: Any) -> None:
        """Handle one internal think message (self-addressed, via :meth:`think`).

        Must be fast and non-blocking -- think handlers are part of the same
        fold as :meth:`perceive`. Default: ignore.
        """

    # ------------------------------------------------------------------
    # Effects -- callable from handlers
    # ------------------------------------------------------------------

    def act(self, payload: Any, recipients: Iterable[str] | None = None) -> None:
        """Queue an outgoing in-world message.

        ``recipients`` are agent IDs; the default ``None`` broadcasts on the
        public channel. The reserved name ``"env"`` addresses the
        environment's inbox. Application code never sees raw subjects.

        ``act`` does not publish: it appends to an outbox the runtime flushes
        only after the current handler returns (and discards if the handler
        raises), so outgoing messages land at handler boundaries, never
        mid-thought.
        """
        if recipients is None:
            self._outbox.append((payload, None))
            return
        names = tuple(recipients)
        for name in names:
            if name != ENV_NAME:
                validate_name(name, kind="recipient agent id")
        self._outbox.append((payload, names))

    def think(self, payload: Any) -> None:
        """Post an internal message to this agent's own event stream.

        The think queue is *not* behind the outbox: this writes the internal
        queue directly and is unaffected by handler exceptions. Spawned tasks
        use it to re-enter the fold (e.g. with an LLM completion).
        """
        self._events.put_nowait(ThinkEvent(payload))

    def schedule_think(self, delay: float, payload: Any) -> asyncio.Task[None]:
        """Post *payload* to the think queue after *delay* seconds."""
        return self.spawn(self._delayed_think(delay, payload))

    def schedule_periodic_think(self, interval: float, payload: Any) -> asyncio.Task[None]:
        """Post *payload* to the think queue every *interval* seconds until cancelled."""
        return self.spawn(self._periodic_think(interval, payload))

    def spawn(self, coro: Coroutine[Any, Any, Any]) -> asyncio.Task[Any]:
        """Run slow work (e.g. an LLM call) outside the fold.

        The task is tracked (cancelled when the agent winds down) and its
        exception, if any, is surfaced as a logged error. The task should
        report its result back via :meth:`think` so it re-enters the fold as
        an ordinary event -- never mutate agent state from a spawned task.
        """
        task = asyncio.get_running_loop().create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._on_task_done)
        return task

    async def log_event(self, payload: Any) -> None:
        """Publish log-only telemetry to this agent's ``log.<id>`` subject.

        Published immediately, not via the outbox: telemetry is not an
        in-world effect (no agent subscribes to log subjects). Use it for LLM
        call records, "considered speaking, chose silence", timing marks.
        """
        if self._transport is None:
            logger.warning("agent %s: log_event before transport attached; dropped", self._id)
            return
        envelope = Envelope(episode_id=self.episode_id, sender=self._id, payload=payload)
        await self._transport.publish(self._subjects.log(self._id), envelope.to_bytes())

    # ------------------------------------------------------------------
    # Runtime
    # ------------------------------------------------------------------

    async def run(self, nats_url: str) -> None:
        """Connect to NATS, participate in the episode, and return after wind-down.

        Subscribes to the public channel, this agent's inbox, and the control
        subject; starts idle; processes the merged event stream one handler
        at a time until the post-``shutdown`` grace period expires; then
        disconnects cleanly.
        """
        transport = NatsTransport(nats_url)
        await transport.connect()
        try:
            await self.serve(transport)
        finally:
            await transport.close()

    async def serve(self, transport: Transport) -> None:
        """Run the agent's event loop against an already connected transport.

        :meth:`run` wraps this with a :class:`NatsTransport`; tests drive it
        directly with a :class:`MemoryTransport`. Does not close the
        transport (the caller owns it).
        """
        self._transport = transport
        subscriptions: list[Subscription] = [
            await transport.subscribe(self._subjects.public, self._receiver("public")),
            await transport.subscribe(self._subjects.agent(self._id), self._receiver("inbox")),
            await transport.subscribe(self._subjects.control, self._receiver("control")),
        ]
        try:
            while True:
                event = await self._events.get()
                if isinstance(event, StopEvent):
                    break
                await self._process_event(event)
        finally:
            for task in list(self._tasks):
                task.cancel()
            await asyncio.gather(*list(self._tasks), return_exceptions=True)
            for subscription in subscriptions:
                await subscription.unsubscribe()
            self._transport = None

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _receiver(self, channel: Channel) -> MessageHandler:
        async def receive(subject: str, data: bytes) -> None:
            try:
                envelope = Envelope.from_bytes(data)
            except ValidationError:
                logger.warning("agent %s: undecodable message on %s; dropped", self._id, subject)
                return
            self._events.put_nowait(MessageEvent(channel, envelope))

        return receive

    async def _process_event(self, event: AgentEvent) -> None:
        """One step of the fold: dispatch, then flush the outbox.

        The outbox is flushed only when the handler returns normally; on a
        handler exception it is discarded and the error logged. Think
        messages posted by the handler are unaffected either way.
        """
        try:
            await self._dispatch(event)
        except Exception:
            self._outbox.clear()
            logger.exception("agent %s: handler failed; outbox discarded", self._id)
        else:
            await self._flush_outbox()

    async def _dispatch(self, event: AgentEvent) -> None:
        if isinstance(event, ThinkEvent):
            await self.handle_think(event.payload)
            return
        if isinstance(event, StopEvent):  # pragma: no cover - consumed by serve()
            return
        envelope = event.envelope
        if event.channel == "control":
            await self.control(envelope)
            return
        if envelope.sender == self._id:
            return  # the agent's own broadcast echoed back; not a perception
        if _management_type(envelope.payload) is not None:
            await self._handle_management(envelope)
            return
        await self.perceive(envelope)

    async def _handle_management(self, envelope: Envelope) -> None:
        """Framework management messages; intercepted before user perceive."""
        payload = envelope.payload
        mtype = _management_type(payload)
        if mtype == "freeagent.presence_request":
            request_id = payload.get("request_id")
            if not isinstance(request_id, str):
                logger.warning("agent %s: presence request without request_id", self._id)
                return
            await self._send_presence_reply(request_id)
            return
        logger.debug("agent %s: ignoring management message %s", self._id, mtype)

    async def _send_presence_reply(self, request_id: str) -> None:
        if self._transport is None:  # pragma: no cover - serve() always sets it
            return
        reply = Envelope(
            episode_id=self.episode_id,
            sender=self._id,
            payload={
                "type": "freeagent.presence_reply",
                "request_id": request_id,
                "agent": self._id,
                "nonce": self._nonce,
            },
        )
        await self._transport.publish(self._subjects.reply(request_id), reply.to_bytes())

    async def _flush_outbox(self) -> None:
        if not self._outbox:
            return
        pending, self._outbox = self._outbox, []
        if self._transport is None:
            logger.warning("agent %s: no transport; outbox of %d dropped", self._id, len(pending))
            return
        for payload, recipients in pending:
            envelope = Envelope(episode_id=self.episode_id, sender=self._id, payload=payload)
            data = envelope.to_bytes()
            if recipients is None:
                await self._transport.publish(self._subjects.public, data)
                continue
            for name in recipients:
                subject = self._subjects.env if name == ENV_NAME else self._subjects.agent(name)
                await self._transport.publish(subject, data)

    def _begin_wind_down(self) -> None:
        if self._phase == "stopping":
            return
        self._phase = "stopping"
        grace = float(self.config.get("grace_period", DEFAULT_GRACE_PERIOD))
        self.spawn(self._stop_after(grace))

    async def _stop_after(self, delay: float) -> None:
        await asyncio.sleep(delay)
        self._events.put_nowait(StopEvent())

    async def _delayed_think(self, delay: float, payload: Any) -> None:
        await asyncio.sleep(delay)
        self.think(payload)

    async def _periodic_think(self, interval: float, payload: Any) -> None:
        while True:
            await asyncio.sleep(interval)
            self.think(payload)

    def _on_task_done(self, task: asyncio.Task[Any]) -> None:
        self._tasks.discard(task)
        if task.cancelled():
            return
        exc = task.exception()
        if exc is not None:
            logger.error("agent %s: spawned task failed", self._id, exc_info=exc)
