"""Launch, watch, and stop application episodes on behalf of the REST API.

This module is the API's half of the control/data plane split. For every episode the API
provisions, it subscribes one NATS subscription to the episode's whole subject subtree and sorts
each arriving message into one of two planes:

- **Control plane:** the SDK vocabulary — :class:`~freeagent.sdk.message.StartEntity`,
  :class:`~freeagent.sdk.message.StopEntity`, :class:`~freeagent.sdk.message.StopAgent`,
  :class:`~freeagent.sdk.message.Ack`, :class:`~freeagent.sdk.message.EpisodeComplete` — decoded
  with :meth:`~freeagent.sdk.message.Message.try_decode` and folded into the episode's lifecycle
  state: which agents are alive, whether the episode is running, when it definitely ended.
- **Data plane:** everything else. Application-defined messages are opaque here by design:
  each is ``json.loads``-ed, indexed by subject, ``message_type`` tag, and arrival time, and served
  back verbatim. The API never computes on data-plane contents.

The sorting is by *type*, not by "does it decode": membership in the explicit
:data:`CONTROL_PLANE_TYPES` set decides the plane. In a pure API process the two tests agree (no
application code is imported, so app messages don't decode), but a process that happens to have
application message classes registered — a test suite, a worker sharing an interpreter — must not
have those messages silently vanish from the data-plane feed just because they decoded.

Provisioning an episode spawns a ``freeagent-worker`` subprocess to run it. The worker is a
*process* dependency, never an import: the command line is built from strings and the module is
launched with ``python -m``, so the invariant that ``freeagent-api`` may only depend on names
defined in ``freeagent-sdk`` holds (and is enforced by import-linter).

Everything held here is a cache, not a record: under core NATS the monitor's state is
best-effort by construction, good for serving viewers and never fed to an archive.
"""

from __future__ import annotations

import asyncio
import json
import subprocess
import sys
import time
import uuid
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from enum import StrEnum
from typing import Any, Protocol

import nats
from freeagent.sdk import UnknownApplication, available_applications
from freeagent.sdk import subjects as _sdk_subjects
from freeagent.sdk.entity import AGENTS
from freeagent.sdk.message import (
    Ack,
    EpisodeComplete,
    Message,
    StartEntity,
    StopAgent,
    StopEntity,
)
from freeagent.sdk.subjects import valid_subject_token
from nats.aio.msg import Msg
from pydantic import BaseModel

CONTROL_PLANE_TYPES: tuple[type[Message], ...] = (
    Ack,
    EpisodeComplete,
    StartEntity,
    StopAgent,
    StopEntity,
)
"""The SDK message types that make up the control plane.

A message of one of these concrete types updates episode lifecycle state; every other message —
including SDK :class:`~freeagent.sdk.message.Message` subclasses this process happens to have
registered — is data plane, recorded opaquely and served verbatim. Deliberately an explicit set
rather than "whatever :meth:`~freeagent.sdk.message.Message.try_decode` recognizes": the registry is
process-global and open, so what decodes depends on what happens to be imported, and the plane split
must not.
"""

_CONTROL_PLANE_TYPE_NAMES = frozenset(cls.__name__ for cls in CONTROL_PLANE_TYPES)
"""The control-plane types' envelope tags, for cheaply pre-filtering before decoding.

Sound because the SDK pins every message's ``message_type`` tag to its concrete class name (a
``Literal``), so a payload whose tag isn't in this set cannot decode to a control-plane type.
"""

WORKER_MODULE = "freeagent.worker.cli"
"""The worker's CLI module, launched as ``python -m`` to run an episode.

A string, never an import: the worker is a process boundary. Importing it (or any application
package) inside ``freeagent.api`` would break the invariant that the API depends only on
``freeagent-sdk`` names, which import-linter enforces.
"""

WORKER_STOP_TIMEOUT = 5.0
"""Seconds to wait for a terminated worker process to exit before killing it outright."""

SUBJECT_TOKEN_PATTERN = _sdk_subjects.SUBJECT_TOKEN_PATTERN
"""Regex for a single NATS subject token — the one definition of "valid episode ID characters".

Re-exported from :mod:`freeagent.sdk.subjects` (the platform-wide definition, shared with the
worker's command line) for the REST request model (see :mod:`freeagent.api.app`), so the HTTP-layer
validation and :meth:`EpisodeManager.create`'s own guard cannot drift apart.

Application names and episode IDs both become tokens of the episode's root subject, so anything with
a ``.``, whitespace, or a wildcard character would corrupt the subject hierarchy (and let one
episode's subscription overlap another's).
"""


class UnknownEpisode(LookupError):
    """Raised when an episode is looked up under an (application, episode ID) pair that has none.

    A :class:`LookupError` like the SDK's :class:`~freeagent.sdk.UnknownApplication`, and for the
    same reason: the API turns it into a 404 without also swallowing unrelated errors.
    """


class DuplicateEpisode(ValueError):
    """Raised when an episode is created under an (application, episode ID) pair already in use.

    Distinct from :class:`UnknownEpisode` so the API can tell "no such episode" (404) apart from
    "that episode already exists" (409).
    """


class EpisodeState(StrEnum):
    """The lifecycle states an episode moves through, as derived from control-plane traffic.

    ``CREATED`` is the provisioned-but-silent state: the subscription is live and the worker
    spawned, but nothing has crossed the wire yet. The first recorded message moves the episode to
    ``RUNNING``. ``COMPLETE`` is set only by the environment's definite end marker
    (:class:`~freeagent.sdk.message.EpisodeComplete`); ``STOPPED`` by a caller stopping the episode
    through the API; ``FAILED`` by the worker process exiting nonzero. The last three are terminal:
    once reached, no later signal moves the episode out of them.
    """

    CREATED = "created"
    RUNNING = "running"
    COMPLETE = "complete"
    STOPPED = "stopped"
    FAILED = "failed"


TERMINAL_STATES = frozenset({EpisodeState.COMPLETE, EpisodeState.STOPPED, EpisodeState.FAILED})
"""States an episode never leaves; see :class:`EpisodeState`."""


class DataPlaneRecord(BaseModel):
    """One data-plane message, as recorded off the wire and served verbatim.

    The whole index the API keeps over application traffic: where the message appeared
    (``subject``), what its envelope claims it is (``message_type``), when it arrived (``seq``,
    ``received_at``), and the payload itself, parsed as JSON but otherwise untouched.

    :ivar seq: The message's position in this episode's feed, counting from 0 in arrival order.
    :ivar subject: The NATS subject the message arrived on.
    :ivar message_type: The payload's ``message_type`` envelope tag, or ``None`` if the payload
        isn't a JSON object carrying one. Read off the envelope only — never used to decode.
    :ivar received_at: Unix timestamp of the message's arrival at the API's subscription.
    :ivar payload: The message's JSON payload, served as parsed. The API never computes on it.
    """

    seq: int
    subject: str
    message_type: str | None
    received_at: float
    payload: Any


class EpisodeStatus(BaseModel):
    """An episode's lifecycle status, as the API serves it over REST.

    :ivar application: The application the episode belongs to.
    :ivar episode_id: The episode's identifier, unique within its application.
    :ivar episode_root: The root NATS subject the episode's traffic lives under.
    :ivar state: The lifecycle state derived from control-plane traffic; see :class:`EpisodeState`.
    :ivar agents_alive: Names of the agents started and not yet stopped, sorted for stable output.
    :ivar message_count: How many data-plane messages the feed holds so far.
    :ivar worker_exit_code: The worker process's exit code, or ``None`` while it is still running
        (or was never spawned, e.g. under a fake in tests).
    """

    application: str
    episode_id: str
    episode_root: str
    state: EpisodeState
    agents_alive: list[str]
    message_count: int
    worker_exit_code: int | None


class EpisodeMonitor:
    """Derives one episode's state from the messages crossing its subject subtree.

    Fed every message the API's subscription sees under the episode root, it maintains the control-
    plane view (lifecycle state, which agents are alive) and the data-plane feed (an ordered list of
    :class:`DataPlaneRecord`). Pure bookkeeping — no I/O — so the whole state machine is unit-
    testable without a server.

    Agent liveness is read from subjects, not payloads: lifecycle commands are addressed to
    ``{episode_root}.agents.{name}``, so a :class:`~freeagent.sdk.message.StartEntity` seen there
    marks ``name`` alive and a :class:`~freeagent.sdk.message.StopAgent` or
    :class:`~freeagent.sdk.message.StopEntity` there marks it stopped.

    :param application: The application the episode belongs to.
    :param episode_id: The episode's identifier.
    :param episode_root: The root NATS subject the episode's traffic lives under.
    :ivar state: The episode's current lifecycle state.
    :ivar agents_alive: Names of the agents started and not yet stopped.
    :ivar messages: The data-plane feed, in arrival order.
    """

    def __init__(self, application: str, episode_id: str, episode_root: str) -> None:
        self.application = application
        self.episode_id = episode_id
        self.episode_root = episode_root
        self.state = EpisodeState.CREATED
        self.agents_alive: set[str] = set()
        self.messages: list[DataPlaneRecord] = []
        self._agents_prefix = f"{episode_root}.{AGENTS}."

    def record(self, subject: str, data: bytes, received_at: float) -> None:
        """Fold one wire message into the episode's state.

        Non-JSON payloads are skipped entirely: the API serves JSON, and a frame it can't even
        parse has no place in either plane. A parseable message wakes a ``CREATED`` episode to
        ``RUNNING``, then lands in exactly one plane: a message whose decoded type is in
        :data:`CONTROL_PLANE_TYPES` updates lifecycle state; anything else — unknown types,
        undecodable-but-JSON payloads, and even known non-control SDK types — is appended to the
        data-plane feed verbatim. Validation failures on a *known* type are treated as "not a
        control-plane message" rather than an error: the API meets other processes' traffic by
        design and drops nothing it can serve.

        :param subject: The NATS subject the message arrived on.
        :param data: The message's raw payload bytes.
        :param received_at: Unix timestamp of the message's arrival.
        """
        try:
            payload = json.loads(data)
        except ValueError:
            return
        if self.state is EpisodeState.CREATED:
            self.state = EpisodeState.RUNNING
        tag = payload.get("message_type") if isinstance(payload, dict) else None
        # Only a payload whose envelope tag names a control-plane type is worth decoding: the tag
        # pins the concrete class (the SDK narrows it to a Literal), so anything else is data
        # plane by definition and skipping try_decode avoids parsing every data-plane message a
        # second time in this hot path.
        if tag in _CONTROL_PLANE_TYPE_NAMES:
            try:
                decoded = Message.try_decode(data)
            except ValueError:
                decoded = None
            if isinstance(decoded, CONTROL_PLANE_TYPES):
                self._record_control(subject, decoded)
                return
        self.messages.append(
            DataPlaneRecord(
                seq=len(self.messages),
                subject=subject,
                message_type=tag if isinstance(tag, str) else None,
                received_at=received_at,
                payload=payload,
            )
        )

    def _record_control(self, subject: str, message: Message) -> None:
        """Update lifecycle state for one control-plane message.

        :class:`~freeagent.sdk.message.StartEntity` and the two stop commands adjust
        :attr:`agents_alive` when addressed to an agent subject;
        :class:`~freeagent.sdk.message.EpisodeComplete` marks the episode's definite end. An
        :class:`~freeagent.sdk.message.Ack` (or a control command on a non-agent subject) changes
        nothing beyond the running transition already made by :meth:`record`.

        :param subject: The NATS subject the message arrived on.
        :param message: The decoded control-plane message.
        """
        agent = self._agent_of(subject)
        match message:
            case StartEntity() if agent is not None:
                self.agents_alive.add(agent)
            case StopAgent() | StopEntity() if agent is not None:
                self.agents_alive.discard(agent)
            case EpisodeComplete():
                if self.state not in TERMINAL_STATES:
                    self.state = EpisodeState.COMPLETE
            case _:
                pass

    def _agent_of(self, subject: str) -> str | None:
        """Extract the agent name from an agent command subject, if ``subject`` is one.

        Lifecycle commands are addressed to exactly ``{episode_root}.agents.{name}``; a subject
        outside that subtree, or deeper inside it (an agent's extra subjects), names no agent.

        :param subject: The subject a message arrived on.
        :return: The agent name, or ``None`` if the subject isn't a bare agent command subject.
        """
        if not subject.startswith(self._agents_prefix):
            return None
        rest = subject[len(self._agents_prefix) :]
        if rest and "." not in rest:
            return rest
        return None

    def mark_stopped(self) -> None:
        """Record that the episode was stopped through the API, unless already in a terminal state.

        A stop after completion changes nothing: ``COMPLETE`` is the stronger, wire-derived fact.
        """
        if self.state not in TERMINAL_STATES:
            self.state = EpisodeState.STOPPED

    def mark_failed(self) -> None:
        """Record that the worker process died before the episode ended, unless already terminal.

        Called when the worker's exit code turns up nonzero. A nonzero exit *after* the episode
        completed or was stopped changes nothing — e.g. the terminate signal a stop sends the worker
        also surfaces as a nonzero exit, and must not relabel a deliberate stop as failure.
        """
        if self.state not in TERMINAL_STATES:
            self.state = EpisodeState.FAILED


MessageHandler = Callable[[Msg], Awaitable[None]]
"""The NATS subscription callback shape, matching nats-py's ``cb`` parameter."""


class NatsSubscription(Protocol):
    """The one thing the manager needs from a NATS subscription: tearing it down."""

    async def unsubscribe(self) -> None:
        """Stop delivering messages to this subscription's callback."""
        ...


class NatsClient(Protocol):
    """The slice of a NATS client the manager uses, as a protocol so tests can fake it."""

    async def subscribe(self, subject: str, *, cb: MessageHandler) -> NatsSubscription:
        """Subscribe ``cb`` to ``subject`` and return the live subscription.

        :param subject: The subject (or wildcard) to subscribe to.
        :param cb: The coroutine callback invoked with each arriving message.
        :return: The subscription, for later unsubscribing.
        """
        ...

    async def flush(self) -> None:
        """Round-trip to the server, guaranteeing everything sent so far has been processed.

        After ``subscribe`` + ``flush``, the subscription is registered server-side: a message
        published by *any* connection afterwards is delivered to it.
        """
        ...

    async def close(self) -> None:
        """Disconnect from the server."""
        ...


class WorkerProcess(Protocol):
    """The slice of a :class:`subprocess.Popen` the manager uses, fakeable in tests."""

    def poll(self) -> int | None:
        """Return the process's exit code, or ``None`` if it is still running."""
        ...

    def terminate(self) -> None:
        """Ask the process to exit (SIGTERM)."""
        ...

    def kill(self) -> None:
        """Force the process to exit (SIGKILL)."""
        ...

    def wait(self, timeout: float | None = None) -> int:
        """Block until the process exits and return its exit code.

        :param timeout: Seconds to wait before raising :class:`subprocess.TimeoutExpired`, or
            ``None`` to wait indefinitely.
        :return: The process's exit code.
        """
        ...


ConnectFn = Callable[[str], Awaitable[NatsClient]]
"""Injectable factory for the manager's NATS connection; the default is :func:`_connect_nats`."""

SpawnFn = Callable[[list[str]], WorkerProcess]
"""Injectable factory for worker processes; the default is :func:`_spawn_worker`."""


async def _connect_nats(servers: str) -> NatsClient:
    """Connect to NATS with nats-py; the default :data:`ConnectFn`.

    :param servers: The NATS server URL to connect to.
    :return: The connected client.
    """
    return await nats.connect(servers)


def _spawn_worker(command: list[str]) -> WorkerProcess:
    """Spawn a worker subprocess; the default :data:`SpawnFn`.

    Output is discarded rather than piped: nothing reads the pipes, and a filled pipe buffer would
    block the worker mid-episode. The worker's observable result is its wire traffic and its exit
    code.

    :param command: The full command line, as built by :meth:`EpisodeManager._worker_command`.
    :return: The started process.
    """
    return subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


@dataclass
class Episode:
    """One provisioned episode: its derived state, its subscription, and its worker process.

    :ivar monitor: The state derived from the episode's wire traffic.
    :ivar subscription: The live NATS subscription on the episode's subtree, or ``None`` once torn
        down.
    :ivar process: The worker subprocess running the episode, or ``None`` if none was spawned.
    """

    monitor: EpisodeMonitor
    subscription: NatsSubscription | None
    process: WorkerProcess | None

    def status(self) -> EpisodeStatus:
        """Snapshot this episode's lifecycle status for serving over REST.

        :return: The current status, with agents sorted for stable output.
        """
        return EpisodeStatus(
            application=self.monitor.application,
            episode_id=self.monitor.episode_id,
            episode_root=self.monitor.episode_root,
            state=self.monitor.state,
            agents_alive=sorted(self.monitor.agents_alive),
            message_count=len(self.monitor.messages),
            worker_exit_code=self.process.poll() if self.process is not None else None,
        )


class EpisodeManager:
    """Owns every episode the API has provisioned: creation, lookup, stopping, and teardown.

    Episodes are keyed by (application, episode ID) and each lives under its own root subject,
    ``episode.{application}.{episode_id}``, so multiple applications and multiple concurrent
    episodes of one application never share a subject subtree — one subscription per episode root is
    the isolation. All episodes share a single lazily opened NATS connection.

    Creation subscribes *before* spawning the worker, so no message of the episode can be missed by
    a late subscription — the same discipline as the worker's own observer.

    :param nats_url: The NATS server URL, used both for the API's own subscriptions and passed to
        every spawned worker so the whole episode runs on one server.
    :param connect: NATS connection factory, injectable for tests; defaults to nats-py.
    :param spawn: Worker process factory, injectable for tests; defaults to
        :class:`subprocess.Popen`.
    """

    def __init__(
        self,
        nats_url: str,
        *,
        connect: ConnectFn | None = None,
        spawn: SpawnFn | None = None,
    ) -> None:
        self.nats_url = nats_url
        self._connect = connect if connect is not None else _connect_nats
        self._spawn = spawn if spawn is not None else _spawn_worker
        self._client: NatsClient | None = None
        self._episodes: dict[tuple[str, str], Episode] = {}

    async def create(
        self, application: str, episode_id: str | None, config: dict[str, Any]
    ) -> Episode:
        """Provision a new episode: subscribe to its subtree, then spawn a worker to run it.

        The episode root is ``episode.{application}.{episode_id}``; a caller-supplied episode ID
        must be a single NATS subject token, and an omitted one is generated. ``config`` is opaque
        here — it is serialized back to JSON and handed to the worker untouched, which hands it to
        the application.

        :param application: The application to run an episode of.
        :param episode_id: The episode's identifier, or ``None`` to generate one.
        :param config: The application-defined episode config, never inspected.
        :return: The provisioned episode, in the ``CREATED`` state.
        :raises UnknownApplication: If ``application`` isn't installed (or couldn't be a subject
            token, in which case no installable application could bear the name).
        :raises ValueError: If a supplied ``episode_id`` isn't a valid NATS subject token.
        :raises DuplicateEpisode: If the (application, episode ID) pair is already in use.
        """
        self._require_installed(application)
        if episode_id is None:
            episode_id = uuid.uuid4().hex
        elif not valid_subject_token(episode_id):
            raise ValueError(
                f'Episode ID "{episode_id}" is not a single NATS subject token '
                f"(letters, digits, hyphen, underscore)"
            )
        key = (application, episode_id)
        if key in self._episodes:
            raise DuplicateEpisode(
                f'Episode "{episode_id}" of application "{application}" already exists'
            )
        episode_root = f"episode.{application}.{episode_id}"
        monitor = EpisodeMonitor(application, episode_id, episode_root)
        episode = Episode(monitor=monitor, subscription=None, process=None)
        # Reserve the key before the first await: with a suspension point between the duplicate
        # check and the insert, two concurrent creates of the same episode would both pass the
        # check, both subscribe, and both spawn workers into one subject tree — and the loser's
        # worker and subscription would leak, with no dict entry left to stop them through.
        self._episodes[key] = episode
        try:

            async def _record(msg: Msg) -> None:
                monitor.record(msg.subject, msg.data, time.time())

            client = await self._ensure_client()
            subscription = await client.subscribe(f"{episode_root}.>", cb=_record)
            # Flush so the subscription is registered server-side before the worker spawns:
            # subscribe() alone only queues the SUB on this connection, and a worker (its own
            # connection) publishing before the server processes it would go unrecorded —
            # the very missed-message window subscribing-before-spawning exists to close.
            await client.flush()
            episode.subscription = subscription
            try:
                process = self._spawn(
                    self._worker_command(application, episode_id, episode_root, config)
                )
            except BaseException:
                # The worker never started, so the episode doesn't exist; don't leave its
                # subscription behind to record traffic for a record nobody can look up.
                await subscription.unsubscribe()
                episode.subscription = None
                raise
            episode.process = process
        except BaseException:
            # Roll the reservation back so a failed create can simply be retried.
            if self._episodes.get(key) is episode:
                del self._episodes[key]
            raise
        return episode

    def get(self, application: str, episode_id: str) -> Episode:
        """Look up an episode, refreshing its state from the worker process on the way.

        The refresh (see :meth:`_refresh`) is how worker death is noticed on the lookup path.

        :param application: The application the episode belongs to.
        :param episode_id: The episode's identifier.
        :return: The episode.
        :raises UnknownEpisode: If no such episode was provisioned.
        """
        try:
            episode = self._episodes[(application, episode_id)]
        except KeyError:
            raise UnknownEpisode(
                f'No episode "{episode_id}" of application "{application}"'
            ) from None
        return self._refresh(episode)

    def list_episodes(self, application: str) -> list[Episode]:
        """List an application's episodes in creation order, refreshing each on the way.

        The same worker-death refresh as :meth:`get`, applied to every episode listed, so a viewer
        enumerating episodes sees the same states it would polling them one at a time.

        :param application: The application whose episodes to list.
        :return: The application's episodes, oldest first (creation order).
        :raises UnknownApplication: If ``application`` isn't installed.
        """
        self._require_installed(application)
        return [
            self._refresh(episode)
            for (app, _), episode in self._episodes.items()
            if app == application
        ]

    @staticmethod
    def _require_installed(application: str) -> None:
        """Reject an application name that doesn't belong to an installed application.

        The one definition of the guard :meth:`create` and :meth:`list_episodes` share. The subject-
        token pre-check isn't only an optimization: a name that can't be a subject token can't be an
        installed application's name, and rejecting it without consulting the entry points keeps the
        error uniform however malformed the input.

        :param application: The application name to check.
        :raises UnknownApplication: If ``application`` isn't installed (or couldn't be a subject
            token, in which case no installable application could bear the name).
        """
        if not valid_subject_token(application) or application not in available_applications():
            raise UnknownApplication(f'No application named "{application}" is installed')

    @staticmethod
    def _refresh(episode: Episode) -> Episode:
        """Fold the worker process's fate into an episode's state.

        This is how worker death is noticed: a worker that exited nonzero before the episode
        ended moves the episode to ``FAILED`` (a clean exit says nothing — completion is judged
        from the wire's :class:`~freeagent.sdk.message.EpisodeComplete`, not from the process).

        :param episode: The episode to refresh.
        :return: The same episode, for chaining.
        """
        process = episode.process
        if process is not None:
            returncode = process.poll()
            if returncode is not None and returncode != 0:
                episode.monitor.mark_failed()
        return episode

    async def stop(self, application: str, episode_id: str) -> Episode:
        """Stop an episode: terminate its worker, drop its subscription, and mark it stopped.

        Idempotent: stopping an already-stopped (or completed) episode terminates nothing, and the
        terminal state already reached is kept.

        :param application: The application the episode belongs to.
        :param episode_id: The episode's identifier.
        :return: The stopped episode.
        :raises UnknownEpisode: If no such episode was provisioned.
        """
        episode = self.get(application, episode_id)
        await self._halt(episode)
        return episode

    async def shutdown(self) -> None:
        """Tear down everything the manager owns: every episode, then the NATS connection.

        Called from the API's lifespan shutdown so no worker process or subscription outlives the
        server. Idempotent, like the teardowns it delegates to.
        """
        # Halts touch disjoint processes and subscriptions, so run them concurrently: N stuck
        # workers then cost one WORKER_STOP_TIMEOUT rather than N of them.
        await asyncio.gather(*(self._halt(episode) for episode in self._episodes.values()))
        if self._client is not None:
            await self._client.close()
            self._client = None

    async def _halt(self, episode: Episode) -> None:
        """Stop one episode's worker and subscription, marking it stopped unless already terminal.

        The worker is terminated and reaped off the event loop (its ``wait`` blocks), escalating to
        a kill if it ignores the terminate for :data:`WORKER_STOP_TIMEOUT` seconds. Idempotent: an
        exited worker isn't re-signaled and a dropped subscription isn't re-dropped.

        :param episode: The episode to halt.
        """
        process = episode.process
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                await asyncio.to_thread(process.wait, WORKER_STOP_TIMEOUT)
            except subprocess.TimeoutExpired:
                process.kill()
                await asyncio.to_thread(process.wait)
        if episode.subscription is not None:
            await episode.subscription.unsubscribe()
            episode.subscription = None
        episode.monitor.mark_stopped()

    async def _ensure_client(self) -> NatsClient:
        """Return the shared NATS client, connecting on first use.

        Lazy so building a manager (and the FastAPI app around it) stays synchronous and free of
        I/O; the connection happens on the first episode creation, inside the running event loop.

        :return: The connected client.
        """
        if self._client is None:
            self._client = await self._connect(self.nats_url)
        return self._client

    def _worker_command(
        self, application: str, episode_id: str, episode_root: str, config: dict[str, Any]
    ) -> list[str]:
        """Build the command line that runs one episode in a worker subprocess.

        Launches :data:`WORKER_MODULE` under the same interpreter serving the API, so the worker
        sees the same installed applications. The episode root is passed explicitly (rather than
        left to the worker's default) because the API's per-episode subscription and the worker must
        agree on it exactly.

        :param application: The application to run.
        :param episode_id: The episode's identifier.
        :param episode_root: The root subject the API subscribed to for this episode.
        :param config: The opaque application config, serialized to JSON for the worker.
        :return: The full command line for :data:`SpawnFn`.
        """
        return [
            sys.executable,
            "-m",
            WORKER_MODULE,
            "run",
            application,
            "--episode-id",
            episode_id,
            "--episode-root",
            episode_root,
            "--nats-url",
            self.nats_url,
            "--config",
            json.dumps(config),
        ]
