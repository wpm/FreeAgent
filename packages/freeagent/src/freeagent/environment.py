"""The Environment base class: lifecycle owner, same fold model as the agent.

The environment marries a roster of agents to a NATS subject root and owns
the episode lifecycle state machine::

    setup --> running --> stopping --> ended
      \\--> aborted (roster incomplete at setup timeout, or duplicate name)

It is the same fold as the agent -- a single locally ordered event stream
processed one handler at a time -- with three differences: no think queue
(but timers, which enter the fold as events), a constructor that *creates*
the episode (app name, roster, episode ID, timeouts), and lifecycle
ownership (presence confirmation by request/reply, ``start`` and
``shutdown`` broadcasts, timeout-driven aborts). A minimal environment is
this base class plus a roster, zero overrides.

**Control flows one way.** The environment broadcasts on the control subject;
agents never send on it. Agents *can* send app-defined management messages to
the environment's inbox (e.g. "the game is over") -- the inbox message is
information, the control broadcast is the decision. The same inbox carries the
framework's one service->environment message, an **operator-abort request**
(``{"type": "freeagent.abort"}``): it asks the environment to take its normal
``stopping -> aborted`` path -- ``shutdown`` broadcast, grace period, exit with
the aborted code -- rather than have its process killed (which would skip the
broadcast and strand agents). Here too the request is information; the broadcast
is the decision. Replies to environment-originated requests use episode-scoped
reply subjects (``<app>.episode.<id>.reply.<req-id>``), never NATS's
``_INBOX.>``.

Handlers here obey the same hard rule as agents: **fast and non-blocking** --
no LLM calls or slow work inside a handler; slow work is a spawned task.
"""

from __future__ import annotations

import asyncio
import enum
import logging
import uuid
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any, Literal

from pydantic import ValidationError

from .config import (
    DEFAULT_EPISODE_TIMEOUT,
    DEFAULT_GRACE_PERIOD,
    DEFAULT_LIVENESS_INTERVAL,
    DEFAULT_LIVENESS_TIMEOUT,
    DEFAULT_SETUP_TIMEOUT,
)
from .envelope import Envelope
from .manifest import resolved_version_for
from .metadata import KEY_NAME, KEY_OUTCOME, KEY_STATUS, EpisodeMetadata
from .names import fallback_episode_name
from .subjects import ENV_NAME, EpisodeSubjects, validate_name
from .transport import MessageHandler, NatsTransport, Subscription, Transport

if TYPE_CHECKING:
    from collections.abc import Coroutine, Iterable, Mapping, Sequence

logger = logging.getLogger("freeagent.environment")


class EpisodeState(enum.StrEnum):
    """The lifecycle state machine's states."""

    SETUP = "setup"
    RUNNING = "running"
    STOPPING = "stopping"
    ENDED = "ended"
    ABORTED = "aborted"


EnvChannel = Literal["public", "env", "reply"]


@dataclass(slots=True)
class EnvMessageEvent:
    """An external message entering the environment's fold."""

    channel: EnvChannel
    envelope: Envelope


@dataclass(slots=True)
class TimerEvent:
    """A timer (or lifecycle decision) entering the environment's fold."""

    name: str
    payload: Any = field(default=None)


EnvironmentEvent = EnvMessageEvent | TimerEvent

_MANAGEMENT_PREFIX = "freeagent."
_SETUP_TIMER = "freeagent.setup_timeout"
_EPISODE_TIMER = "freeagent.episode_timeout"
_SHUTDOWN_TIMER = "freeagent.shutdown"
_ABORT_SIGNAL = "freeagent.operator_abort"
_GRACE_TIMER = "freeagent.grace_period"
#: Periodic mid-episode liveness probe: re-request presence from the roster.
_LIVENESS_TIMER = "freeagent.liveness_probe"
#: The deadline after a probe by which every probed member must have answered.
_LIVENESS_DEADLINE_TIMER = "freeagent.liveness_deadline"

OPERATOR_ABORT_TYPE = "freeagent.abort"
"""Management ``type`` of an operator-abort request on the environment inbox.

Part of the service->environment control protocol (see :mod:`freeagent.control`):
sent on ``<root>.env``, it asks the environment to abort gracefully -- broadcast
``shutdown``, run the grace period, and settle in ``aborted``. Lifecycle
authority stays with the environment: the message is a request, not a command.
"""


def _resolve_versions(manifest_set: Iterable[Mapping[str, Any]]) -> dict[str, str]:
    """Resolve each manifest's class ref to its installed ``distribution==version``.

    Imports every distinct ``module:QualName`` named in the set and resolves the
    installed distribution shipping it (:func:`freeagent.manifest.resolved_version_for`).
    A ref that cannot be imported, or whose package has no installed distribution
    metadata, is skipped -- the stamp is write-only provenance, never a launch
    gate, so a loose or test-only class is a no-op rather than an error. The lazy
    import of :func:`freeagent.cli.child.import_class` avoids the module cycle
    (``cli.child`` imports this module).
    """
    from .cli.child import import_class

    versions: dict[str, str] = {}
    for manifest in manifest_set:
        ref = manifest.get("class")
        if not isinstance(ref, str) or ref in versions:
            continue
        try:
            cls = import_class(ref)
        except Exception:
            logger.debug("env: could not import %r to stamp resolved version", ref)
            continue
        stamp = resolved_version_for(cls)
        if stamp is not None:
            versions[ref] = stamp
    return versions


def _management_type(payload: Any) -> str | None:
    if isinstance(payload, dict):
        mtype = payload.get("type")
        if isinstance(mtype, str) and mtype.startswith(_MANAGEMENT_PREFIX):
            return mtype
    return None


class Environment:
    """Base class for FreeAgent environments. Owns one episode's lifecycle.

    Lifecycle management is the irreducible minimum every environment
    provides; the framework provides no built-in state mechanism -- what
    state an environment keeps and how is entirely up to the application.
    Applications override :meth:`perceive` for the few messages they care
    about and call :meth:`initiate_shutdown` when the episode is over.
    """

    def __init__(
        self,
        app: str,
        roster: Sequence[str],
        episode_id: str | None = None,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        """Create the environment for one episode.

        ``app`` is the application name (subject prefix); ``roster`` the
        fixed list of expected agent IDs (assigned top-down -- duplicates
        rejected); ``episode_id`` is auto-generated when ``None``. ``config``
        keys (seconds): ``setup_timeout`` (default 30), ``episode_timeout``
        (default 600), ``grace_period`` (default 5), ``liveness_interval``
        (default 15; 0 disables the mid-episode liveness probe), and
        ``liveness_timeout`` (default 10; the per-probe response deadline before
        a present role is declared lost and the episode aborts).
        """
        validate_name(app, kind="application name")
        names = list(roster)
        for name in names:
            validate_name(name, kind="agent id")
            if name == ENV_NAME:
                raise ValueError(f"agent id {ENV_NAME!r} is reserved for the environment")
        if len(set(names)) != len(names):
            duplicates = sorted({n for n in names if names.count(n) > 1})
            raise ValueError(f"duplicate agent ids in roster: {duplicates}")
        if episode_id is not None:
            validate_name(episode_id, kind="episode id")
        self.app = app
        self.roster: tuple[str, ...] = tuple(names)
        self.episode_id: str = episode_id if episode_id is not None else uuid.uuid4().hex
        self.config: dict[str, Any] = dict(config or {})
        self.setup_timeout = float(self.config.get("setup_timeout", DEFAULT_SETUP_TIMEOUT))
        self.episode_timeout = float(self.config.get("episode_timeout", DEFAULT_EPISODE_TIMEOUT))
        self.grace_period = float(self.config.get("grace_period", DEFAULT_GRACE_PERIOD))
        # Mid-episode liveness (ADR-0005): a role present at start that stops
        # answering a periodic presence re-probe past the deadline is *lost*, and
        # a lost role aborts the episode. ``liveness_interval`` of 0 disables the
        # probe entirely (the default is conservative so short episodes never see
        # it); ``liveness_timeout`` is the per-probe response deadline.
        self.liveness_interval = float(
            self.config.get("liveness_interval", DEFAULT_LIVENESS_INTERVAL)
        )
        self.liveness_timeout = float(self.config.get("liveness_timeout", DEFAULT_LIVENESS_TIMEOUT))
        self._subjects = EpisodeSubjects(app=app, episode_id=self.episode_id)
        self._state = EpisodeState.SETUP
        self._terminal_state = EpisodeState.ENDED
        self.state_history: list[EpisodeState] = [EpisodeState.SETUP]
        self.outcome: Any = None
        # Friendly name (ADR-0003): a user-set name (config) or the
        # auto-generated fallback, until an application contributes a more
        # specific one via set_name (e.g. Twenty Questions titles a finished
        # game by its secret). Stored on the stream's metadata.
        configured = self.config.get("name")
        self._friendly_name: str = (
            configured
            if isinstance(configured, str) and configured
            else fallback_episode_name(self.episode_id)
        )
        self._name_app_contributed = False
        self.abort_reason: str | None = None
        self.shutdown_reason: str | None = None
        self._events: asyncio.Queue[EnvironmentEvent] = asyncio.Queue()
        self._outbox: list[tuple[Any, tuple[str, ...] | None]] = []
        self._tasks: set[asyncio.Task[Any]] = set()
        self._transport: Transport | None = None
        self._nonces: dict[str, str] = {}
        self._present: set[str] = set()
        self._pending_requests: dict[str, str] = {}
        # The recruited manifest set ("what should be running") and the resolved
        # engine versions ("what ran"), written into the durable record at stream
        # creation (ADR-0005). Empty until the worker's child stamps them via
        # set_manifest_set; a hand-built (non-recruiter) launch leaves them empty.
        self._manifest_set: list[dict[str, Any]] = []
        self._resolved_versions: dict[str, str] = {}
        # Liveness probe bookkeeping (mid-episode, RUNNING only). Each probe
        # round records who it asked (``_liveness_probed``) and who answered
        # (``_liveness_seen``); the deadline timer compares the two.
        self._liveness_probed: set[str] = set()
        self._liveness_seen: set[str] = set()

    # ------------------------------------------------------------------
    # Identity / state
    # ------------------------------------------------------------------

    @property
    def state(self) -> EpisodeState:
        """Current lifecycle state."""
        return self._state

    @property
    def subject_root(self) -> str:
        """The episode's subject root -- what each agent's constructor receives."""
        return self._subjects.root

    @property
    def subjects(self) -> EpisodeSubjects:
        """The episode's subject helper (runtime use only)."""
        return self._subjects

    @property
    def name(self) -> str:
        """The episode's friendly name (ADR-0003): user-set, app-contributed, or
        the auto-generated fallback."""
        return self._friendly_name

    def set_name(self, name: str) -> None:
        """Contribute an application-specific friendly name for the episode.

        Callable from handlers (e.g. Twenty Questions titles a finished game by
        its secret). This is the most specific of the three name sources, so it
        wins over the user-set or fallback name and is written to the stream
        metadata when the episode is sealed. Fast and non-blocking.
        """
        self._friendly_name = name
        self._name_app_contributed = True

    @property
    def manifest_set(self) -> list[dict[str, Any]]:
        """The recruited manifest set written into the durable record (ADR-0005).

        *What should be running*: the manifests the recruiter scheduled for this
        episode, as plain dicts. Empty on a hand-built launch the recruiter did
        not stamp. Never carries a PID -- liveness is the gap between this set and
        presence on the bus.
        """
        return list(self._manifest_set)

    @property
    def resolved_versions(self) -> dict[str, str]:
        """The resolved engine versions written into the durable record (ADR-0005).

        *What ran*: a map from each manifest's ``module:QualName`` class ref to
        its installed ``distribution==version`` stamp, resolved when the manifest
        set was set (the child imports each class). Write-only provenance.
        """
        return dict(self._resolved_versions)

    def set_manifest_set(self, manifest_set: list[dict[str, Any]]) -> None:
        """Record the recruited manifest set and stamp its resolved versions.

        Called by the worker's environment child before :meth:`run`/:meth:`serve`
        (ADR-0005). The set is written verbatim into the durable record at stream
        creation; the resolved versions are stamped here by importing each
        manifest's class (the child is "fat" -- it has the engines installed) and
        resolving the installed distribution. A class that cannot be imported or
        has no installed distribution is simply left unstamped: provenance is
        best-effort, never load-bearing, so a missing version never blocks launch.
        """
        self._manifest_set = [dict(manifest) for manifest in manifest_set]
        self._resolved_versions = _resolve_versions(self._manifest_set)

    def outcome_label(self) -> str | None:
        """How the episode finished -- ``aborted``/``timed_out``/``None``.

        Orthogonal to whether the stream is sealed (ADR-0003): this is the
        *outcome* metadata, not the open/sealed state. Applications override it
        to report a structured result (Twenty Questions returns ``won``/``lost``)
        and may defer to ``super().outcome_label()`` for the framework cases.
        """
        if self._state is EpisodeState.ABORTED:
            return "aborted"
        if self.shutdown_reason == "episode timeout":
            return "timed_out"
        return None

    # ------------------------------------------------------------------
    # Handlers (the fold) -- override these. Fast and non-blocking only.
    # ------------------------------------------------------------------

    async def perceive(self, message: Envelope) -> None:
        """Handle in-world traffic (public channel) and environment-inbox messages.

        Applications override this for the few messages they care about
        (e.g. an agent's "the game is over" signal -> call
        :meth:`initiate_shutdown`). Must be fast and non-blocking: no LLM
        calls or slow work inside a handler -- spawn a task instead. Default:
        ignore.
        """

    async def handle_reply(self, message: Envelope) -> None:
        """Handle a reply to an application-originated request (:meth:`send_request`).

        Presence replies are consumed by the runtime and never reach this.
        Must be fast and non-blocking. Default: ignore.
        """

    async def handle_timer(self, name: str, payload: Any) -> None:
        """Handle an application timer scheduled with :meth:`schedule_timer`.

        Framework timers (``freeagent.*`` names) are consumed by the runtime
        and never reach this. Must be fast and non-blocking. Default: ignore.
        """

    # ------------------------------------------------------------------
    # Effects -- callable from handlers
    # ------------------------------------------------------------------

    def act(self, payload: Any, recipients: Iterable[str] | None = None) -> None:
        """Queue an outgoing in-world message from the environment (sender ``env``).

        Same outbox mechanism as agents: nothing is published until the
        current handler returns; a raising handler publishes nothing.
        ``recipients`` are agent IDs; default broadcasts on the public
        channel.
        """
        if recipients is None:
            self._outbox.append((payload, None))
            return
        names = tuple(recipients)
        for name in names:
            validate_name(name, kind="recipient agent id")
        self._outbox.append((payload, names))

    def schedule_timer(self, delay: float, name: str, payload: Any = None) -> asyncio.Task[None]:
        """Schedule a timer that enters the fold as an event after *delay* seconds.

        Application timer names must not start with ``freeagent.`` (reserved
        for the framework's own timers); they are delivered to
        :meth:`handle_timer`.
        """
        return self.spawn(self._timer(delay, name, payload))

    def spawn(self, coro: Coroutine[Any, Any, Any]) -> asyncio.Task[Any]:
        """Run slow work outside the fold; tracked, errors logged.

        Like the agent's ``spawn``: report results back into the fold (e.g.
        via :meth:`schedule_timer` with delay 0) rather than mutating state
        from the task.
        """
        task = asyncio.get_running_loop().create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._on_task_done)
        return task

    def initiate_shutdown(self, reason: str | None = None) -> None:
        """Decide to end the episode: broadcast ``shutdown`` and start the grace period.

        Callable from application handlers (e.g. on an agent's end-of-game
        inbox signal) and used by the episode timeout. The transition happens
        at the fold boundary: this enqueues a lifecycle event; the broadcast
        goes out when that event is processed. No-op unless running.
        """
        self._events.put_nowait(TimerEvent(_SHUTDOWN_TIMER, reason))

    def initiate_abort(self, reason: str | None = None) -> None:
        """Decide to abort the episode: graceful shutdown that ends in ``aborted``.

        The graceful sibling of :meth:`initiate_shutdown` -- same ``stopping``
        path (``shutdown`` broadcast, grace period for goodbyes) but the episode
        settles in ``aborted`` (exit code 2), not ``ended``. This is what an
        operator-abort request on the inbox triggers, and what an in-process
        supervisor calls to abort gracefully instead of killing the env process
        (which would skip the broadcast and strand agents). Like
        :meth:`initiate_shutdown`, it transitions at the fold boundary and is a
        no-op unless running.
        """
        self._events.put_nowait(TimerEvent(_ABORT_SIGNAL, reason))

    async def broadcast_control(self, payload: Any) -> None:
        """Broadcast a lifecycle message on the control subject (environment only).

        Published immediately -- control is the environment's lifecycle
        authority, not in-world traffic. The control subject is strictly
        environment -> agents; agents never send on it.
        """
        if self._transport is None:
            raise RuntimeError("environment is not attached to a transport")
        envelope = Envelope(episode_id=self.episode_id, sender=ENV_NAME, payload=payload)
        await self._transport.publish(self._subjects.control, envelope.to_bytes())

    async def send_request(self, agent_id: str, payload: dict[str, Any]) -> str:
        """Send an environment-originated request to one agent; returns the request id.

        The request is delivered to the agent's inbox with a ``request_id``
        field added; the reply arrives on the episode-scoped subject
        ``reply.<request-id>`` (never ``_INBOX.>``) and is delivered to
        :meth:`handle_reply`. Request/reply always originates from the
        environment.
        """
        if self._transport is None:
            raise RuntimeError("environment is not attached to a transport")
        request_id = uuid.uuid4().hex
        message = dict(payload)
        message["request_id"] = request_id
        envelope = Envelope(episode_id=self.episode_id, sender=ENV_NAME, payload=message)
        await self._transport.publish(self._subjects.agent(agent_id), envelope.to_bytes())
        return request_id

    # ------------------------------------------------------------------
    # Runtime
    # ------------------------------------------------------------------

    async def run(self, nats_url: str) -> EpisodeState:
        """Drive the full episode lifecycle against NATS; returns the final state."""
        transport = NatsTransport(nats_url)
        await transport.connect()
        try:
            return await self.serve(transport)
        finally:
            await transport.close()

    async def serve(self, transport: Transport) -> EpisodeState:
        """Drive the lifecycle against an already connected transport.

        Creates the episode's JetStream stream (idempotently), confirms the
        roster by presence request/reply, broadcasts ``start``, processes
        events until ``ended`` or ``aborted``, and returns the final state.
        Does not close the transport (the caller owns it).
        """
        self._transport = transport
        await transport.ensure_stream(
            self._subjects.stream,
            [self._subjects.all_subjects],
            metadata=self._initial_metadata(),
        )
        subscriptions: list[Subscription] = [
            await transport.subscribe(self._subjects.public, self._receiver("public")),
            await transport.subscribe(self._subjects.env, self._receiver("env")),
            await transport.subscribe(self._subjects.reply_wildcard, self._receiver("reply")),
        ]
        try:
            await self._begin_setup()
            while self._state not in (EpisodeState.ENDED, EpisodeState.ABORTED):
                event = await self._events.get()
                await self._process_event(event)
            # Terminal step of the stopping path: record the outcome and seal
            # the episode (immutable, complete) before releasing the transport.
            await self._seal_episode()
        finally:
            for task in list(self._tasks):
                task.cancel()
            await asyncio.gather(*list(self._tasks), return_exceptions=True)
            for subscription in subscriptions:
                await subscription.unsubscribe()
            self._transport = None
        return self._state

    def _initial_metadata(self) -> dict[str, str]:
        """The episode metadata written when the stream is created (ADR-0003)."""
        return EpisodeMetadata(
            app=self.app,
            name=self._friendly_name,
            status=str(self._state),
            mode="live",
            created_at=datetime.now(UTC).isoformat(),
            manifest_set=self._manifest_set,
            resolved_versions=self._resolved_versions,
        ).to_stream_metadata()

    async def _write_status(self, status: EpisodeState) -> None:
        """Update the episode's durable status label (best-effort, never fatal)."""
        if self._transport is None:
            return
        try:
            await self._transport.set_stream_metadata(
                self._subjects.stream, {KEY_STATUS: str(status)}
            )
        except Exception:
            logger.warning("env: could not update status metadata", exc_info=True)

    async def _seal_episode(self) -> None:
        """Write the terminal status/outcome/name, then seal the stream.

        Sealing makes a finished episode immutable -- it rejects further appends
        but stays readable and deletable as a whole. Metadata is written *before*
        the seal because a sealed stream rejects config changes too. Best-effort:
        a seal failure is logged, not raised, so it never masks the final state.
        """
        if self._transport is None:
            return
        updates = {KEY_STATUS: str(self._state)}
        outcome = self.outcome_label()
        if outcome is not None:
            updates[KEY_OUTCOME] = outcome
        # The name is rewritten only when the application contributed one, so a
        # mid-episode rename through the service is not clobbered here.
        if self._name_app_contributed:
            updates[KEY_NAME] = self._friendly_name
        try:
            await self._transport.set_stream_metadata(self._subjects.stream, updates)
            await self._transport.seal_stream(self._subjects.stream)
        except Exception:
            logger.warning("env: could not seal episode %s", self.episode_id, exc_info=True)

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _receiver(self, channel: EnvChannel) -> MessageHandler:
        async def receive(subject: str, data: bytes) -> None:
            try:
                envelope = Envelope.from_bytes(data)
            except ValidationError:
                logger.warning("env: undecodable message on %s; dropped", subject)
                return
            self._events.put_nowait(EnvMessageEvent(channel, envelope))

        return receive

    async def _begin_setup(self) -> None:
        """Send presence requests to the roster and arm the setup timeout."""
        for name in self.roster:
            request_id = uuid.uuid4().hex
            self._pending_requests[request_id] = name
            envelope = Envelope(
                episode_id=self.episode_id,
                sender=ENV_NAME,
                payload={"type": "freeagent.presence_request", "request_id": request_id},
            )
            await self._transport.publish(self._subjects.agent(name), envelope.to_bytes())  # type: ignore[union-attr]
        self.schedule_timer(self.setup_timeout, _SETUP_TIMER)
        if self._present >= set(self.roster):  # empty roster: nothing to wait for
            await self._start_episode()

    async def _process_event(self, event: EnvironmentEvent) -> None:
        """One step of the fold: dispatch, then flush the outbox.

        Same semantics as the agent: the outbox is flushed only when the
        handler returns normally and discarded (with a logged error) when it
        raises.
        """
        try:
            await self._dispatch(event)
        except Exception:
            self._outbox.clear()
            logger.exception("env: handler failed; outbox discarded")
        else:
            await self._flush_outbox()

    async def _dispatch(self, event: EnvironmentEvent) -> None:
        if isinstance(event, TimerEvent):
            await self._dispatch_timer(event)
            return
        envelope = event.envelope
        if event.channel == "reply":
            payload = envelope.payload
            if _management_type(payload) == "freeagent.presence_reply":
                await self._handle_presence_reply(payload)
                return
            await self.handle_reply(envelope)
            return
        if envelope.sender == ENV_NAME:
            return  # the environment's own publication echoed back
        mtype = _management_type(envelope.payload)
        if mtype is not None:
            if event.channel == "env" and mtype == OPERATOR_ABORT_TYPE:
                self._request_operator_abort(envelope)
                return
            logger.debug("env: ignoring management message on %s", event.channel)
            return
        await self.perceive(envelope)

    def _request_operator_abort(self, envelope: Envelope) -> None:
        """Honor an operator-abort request on the inbox: begin the graceful abort.

        The environment keeps control authority -- this only *initiates* the
        abort (the broadcast remains the decision); :meth:`initiate_abort` is a
        no-op unless the episode is running.
        """
        payload = envelope.payload
        reason = payload.get("reason") if isinstance(payload, dict) else None
        detail = reason if isinstance(reason, str) else "operator abort"
        logger.info("env: operator abort requested by %r: %s", envelope.sender, detail)
        self.initiate_abort(detail)

    async def _dispatch_timer(self, event: TimerEvent) -> None:
        if event.name == _SETUP_TIMER:
            if self._state is EpisodeState.SETUP:
                missing = sorted(set(self.roster) - self._present)
                await self._abort(f"setup timeout: roster incomplete, missing {missing}")
        elif event.name == _EPISODE_TIMER:
            if self._state is EpisodeState.RUNNING:
                self.initiate_shutdown("episode timeout")
        elif event.name == _SHUTDOWN_TIMER:
            await self._enter_stopping(event.payload)
        elif event.name == _ABORT_SIGNAL:
            await self._enter_stopping(event.payload, terminal=EpisodeState.ABORTED)
        elif event.name == _GRACE_TIMER:
            if self._state is EpisodeState.STOPPING:
                self._set_state(self._terminal_state)
        elif event.name == _LIVENESS_TIMER:
            await self._run_liveness_probe()
        elif event.name == _LIVENESS_DEADLINE_TIMER:
            self._check_liveness_deadline()
        elif event.name.startswith(_MANAGEMENT_PREFIX):  # pragma: no cover - defensive
            logger.warning("env: unknown framework timer %s", event.name)
        else:
            await self.handle_timer(event.name, event.payload)

    async def _handle_presence_reply(self, payload: dict[str, Any]) -> None:
        name = payload.get("agent")
        nonce = payload.get("nonce")
        if not isinstance(name, str) or not isinstance(nonce, str) or name not in self.roster:
            logger.warning("env: malformed or unexpected presence reply: %r", payload)
            return
        known = self._nonces.get(name)
        if known is not None and known != nonce:
            # Two processes launched under the same agent name. Never accepted as
            # a liveness reply (a re-run role must not silence the loss it would
            # otherwise trigger) -- it is a duplicate join, not a heartbeat.
            if self._state is EpisodeState.SETUP:
                await self._abort(f"duplicate agent name {name!r}: presence nonce mismatch")
            else:
                logger.error("env: duplicate agent name %r detected after start", name)
            return
        self._nonces[name] = nonce
        self._present.add(name)
        # A matching-nonce reply during RUNNING is the answer to a liveness probe:
        # the same process that joined is still alive. Record it so the deadline
        # timer does not declare it lost.
        if self._state is EpisodeState.RUNNING:
            self._liveness_seen.add(name)
        if self._state is EpisodeState.SETUP and self._present >= set(self.roster):
            await self._start_episode()

    async def _start_episode(self) -> None:
        """All expected agents replied: fire the gun."""
        self._set_state(EpisodeState.RUNNING)
        await self._write_status(EpisodeState.RUNNING)
        await self.broadcast_control({"type": "start"})
        self.schedule_timer(self.episode_timeout, _EPISODE_TIMER)
        # Arm the first mid-episode liveness probe (ADR-0005). Skipped for an
        # empty roster (nobody to lose) or when the probe is disabled.
        if self.liveness_interval > 0 and self.roster:
            self.schedule_timer(self.liveness_interval, _LIVENESS_TIMER)

    async def _run_liveness_probe(self) -> None:
        """Re-request presence from every still-present role, then arm the deadline.

        The mid-episode liveness check (ADR-0005): only roles that were present at
        start (in :attr:`_present`) are probed -- a role that never joined was
        already handled by the setup timeout, and we never expect a reply from
        someone who is not in the episode. Each round records who it asked
        (``_liveness_probed``) and resets who has answered (``_liveness_seen``);
        the deadline timer compares them. The next probe is re-armed here so the
        check repeats for the life of the episode. A no-op outside RUNNING (a probe
        timer can fire just as the episode enters stopping).
        """
        if self._state is not EpisodeState.RUNNING:
            return
        self._liveness_probed = set(self._present)
        self._liveness_seen = set()
        for name in self._liveness_probed:
            request_id = uuid.uuid4().hex
            self._pending_requests[request_id] = name
            envelope = Envelope(
                episode_id=self.episode_id,
                sender=ENV_NAME,
                payload={"type": "freeagent.presence_request", "request_id": request_id},
            )
            await self._transport.publish(self._subjects.agent(name), envelope.to_bytes())  # type: ignore[union-attr]
        self.schedule_timer(self.liveness_timeout, _LIVENESS_DEADLINE_TIMER)
        self.schedule_timer(self.liveness_interval, _LIVENESS_TIMER)

    def _check_liveness_deadline(self) -> None:
        """A role probed last round that did not answer in time is lost -> abort.

        Reached the response deadline: any member that was probed but is absent
        from ``_liveness_seen`` has gone silent past the timeout -- its child
        vanished from a live episode -- so the episode aborts gracefully. We
        never re-run or re-recruit the role (that is the whole point); the
        duplicate-join nonce check already rejects a process re-joining under the
        same name. A no-op outside RUNNING.
        """
        if self._state is not EpisodeState.RUNNING:
            return
        lost = sorted(self._liveness_probed - self._liveness_seen)
        if lost:
            self.initiate_abort(f"lost role(s) {lost}: no presence reply within liveness timeout")

    async def _enter_stopping(
        self, reason: Any, terminal: EpisodeState = EpisodeState.ENDED
    ) -> None:
        """Begin the grace period; settle in *terminal* (``ended`` or ``aborted``).

        Both shutdown triggers share this path -- the only difference is the
        terminal state the grace timer lands on: a normal shutdown ends in
        ``ended``, an operator abort in ``aborted``. The ``shutdown`` broadcast
        and grace period are identical, so agents wind down the same way either
        way.
        """
        if self._state is not EpisodeState.RUNNING:
            return
        self.shutdown_reason = reason if isinstance(reason, str) else None
        self._terminal_state = terminal
        if terminal is EpisodeState.ABORTED:
            self.abort_reason = self.shutdown_reason
        self._set_state(EpisodeState.STOPPING)
        await self._write_status(EpisodeState.STOPPING)
        await self.broadcast_control({"type": "shutdown"})
        self.schedule_timer(self.grace_period, _GRACE_TIMER)

    async def _abort(self, reason: str) -> None:
        self.abort_reason = reason
        self._set_state(EpisodeState.ABORTED)
        logger.error("env: episode %s aborted: %s", self.episode_id, reason)
        # Cooperative cleanup: tell whoever did show up to wind down.
        await self.broadcast_control({"type": "shutdown"})

    def _set_state(self, state: EpisodeState) -> None:
        self._state = state
        self.state_history.append(state)

    async def _flush_outbox(self) -> None:
        if not self._outbox:
            return
        pending, self._outbox = self._outbox, []
        if self._transport is None:  # pragma: no cover - defensive
            logger.warning("env: no transport; outbox of %d dropped", len(pending))
            return
        for payload, recipients in pending:
            envelope = Envelope(episode_id=self.episode_id, sender=ENV_NAME, payload=payload)
            data = envelope.to_bytes()
            if recipients is None:
                await self._transport.publish(self._subjects.public, data)
                continue
            for name in recipients:
                await self._transport.publish(self._subjects.agent(name), data)

    async def _timer(self, delay: float, name: str, payload: Any) -> None:
        await asyncio.sleep(delay)
        self._events.put_nowait(TimerEvent(name, payload))

    def _on_task_done(self, task: asyncio.Task[Any]) -> None:
        self._tasks.discard(task)
        if task.cancelled():
            return
        exc = task.exception()
        if exc is not None:
            logger.error("env: spawned task failed", exc_info=exc)
