"""The control service's in-memory episode registry and launch logic.

:class:`ControlService` is the persistent piece's brain: it owns the set of
running episodes and the operations the REST layer exposes -- create, list, get,
stop, teardown -- with no web framework in sight, so it is testable on its own.

It launches **live** episodes through the supervised
:func:`~freeagent.start_episode` handle (one episode = one
:class:`~freeagent.EpisodeHandle` driving its own child processes) and **replay**
episodes by re-publishing a recorded Parquet log onto NATS via
:class:`~freeagent.Replayer`. Either way the result is observable over NATS
exactly like a ``run``-launched episode; the registry just remembers the mapping
from a REST id to that episode's subject root and lifecycle.

**Known limitation (v1).** The registry is in memory only: a service restart
forgets every episode it was tracking (the episodes' own child processes are
torn down with the service, so nothing is orphaned, but history is not
persisted). Persistence is deferred.
"""

from __future__ import annotations

import asyncio
import contextlib
import secrets
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urlsplit

import nats
from nats.errors import NoServersError

from freeagent.cli.apps import AppSpec, load_app
from freeagent.cli.config import ComponentSpec, EpisodeConfig, make_plan
from freeagent.cli.orchestrate import EpisodeHandle, start_episode
from freeagent.replayer import Replayer, ReplayMessage, load_episode
from freeagent.subjects import EpisodeSubjects, subject_root, validate_name
from freeagent.transport import NatsTransport, Transport

if TYPE_CHECKING:
    from collections.abc import Callable, Iterable

    from .models import CreateEpisodeRequest

#: How long an episode's id is, in random hex characters. Short enough to read
#: in a listing, wide enough that ids are not reused across restarts.
_ID_BYTES = 4

#: Bound on the startup/create NATS reachability probe, so a black-hole host
#: cannot hang a request waiting on a connection that never completes.
_NATS_PROBE_TIMEOUT = 8.0

#: Bound on tearing one episode down, so a wedged child cannot hang teardown.
_TEARDOWN_TIMEOUT = 30.0


class ServiceError(Exception):
    """Base for control-service errors the REST layer maps to status codes."""


class NatsUnreachableError(ServiceError):
    """NATS could not be reached -- a clear error instead of a hang (HTTP 503)."""


class EpisodeNotFoundError(ServiceError):
    """No episode is registered under the requested application and id (HTTP 404)."""


class EpisodeExistsError(ServiceError):
    """The requested episode id is already taken (HTTP 409)."""


async def _quiet_error_cb(_exc: Exception) -> None:
    """Swallow nats-py's per-attempt tracebacks; the raised error is the message."""


def _unreachable(url: str) -> NatsUnreachableError:
    """The clear, actionable error for a NATS server that cannot be reached."""
    return NatsUnreachableError(
        f"could not connect to NATS at {url}. Is the server running? Start it with: "
        "docker compose -f docker/nats/docker-compose.yml up -d"
    )


def _host_port(url: str) -> tuple[str, int]:
    """The host and port of a ``nats://host:port`` URL (first server, default 4222)."""
    parsed = urlsplit(url.split(",", 1)[0])
    return parsed.hostname or "localhost", parsed.port or 4222


async def verify_nats_reachable(url: str, *, timeout: float = _NATS_PROBE_TIMEOUT) -> None:  # noqa: ASYNC109 -- the timeout is this probe's whole point: the bound that makes it non-hanging
    """Confirm a NATS server answers at *url*, or raise :class:`NatsUnreachableError`.

    Two bounded stages, each guarded by ``wait_for`` so neither can hang:

    1. a TCP reachability gate that a closed port refuses *immediately* -- the
       common "NATS is down" case fails fast without waiting out nats-py's
       reconnect backoff;
    2. a full nats-py connect (reconnection disabled) to confirm something that
       actually speaks NATS is listening, not just any open socket.

    This is deliberately *not* :class:`~freeagent.NatsTransport`, whose reconnect
    tuning is right for a long-lived episode connection but wrong for a one-shot
    liveness check.
    """
    host, port = _host_port(url)
    try:
        _reader, writer = await asyncio.wait_for(
            asyncio.open_connection(host, port), timeout=timeout
        )
    except (OSError, TimeoutError) as exc:
        raise _unreachable(url) from exc
    writer.close()
    with contextlib.suppress(Exception):
        await writer.wait_closed()
    try:
        client = await asyncio.wait_for(
            nats.connect(
                url,
                allow_reconnect=False,
                max_reconnect_attempts=0,
                connect_timeout=min(timeout, 2.0),
                error_cb=_quiet_error_cb,
            ),
            timeout=timeout,
        )
    except (NoServersError, OSError, TimeoutError) as exc:
        raise _unreachable(url) from exc
    else:
        await client.close()


@dataclass
class ManagedEpisode:
    """One registered episode: its identity plus the live/replay state behind it.

    The base carries everything the REST view needs that is the same for both
    modes; :attr:`status`, :attr:`detail`, :meth:`stop`, and :meth:`teardown`
    are mode-specific and supplied by the subclasses.
    """

    rest_id: str
    application: str
    app: str
    episode_id: str
    mode: str
    nats_url: str
    created_at: datetime

    @property
    def subject_root(self) -> str:
        """The NATS subject root a viewer subscribes under for this episode."""
        return subject_root(self.app, self.episode_id)

    @property
    def status(self) -> str:
        raise NotImplementedError

    @property
    def detail(self) -> str | None:
        return None

    @property
    def controllable(self) -> bool:
        """Whether the service can still gracefully stop this episode.

        True only while running: the service holds a live handle (live) or an
        active playback task (replay) it can wind down; a terminal episode has
        nothing left to stop. This is what lets the browser show **Stop** only
        where it can act. A pre-restart episode is not in the registry at all, so
        it has no view -- discovery finds it over JetStream but offers no Stop,
        which is inherent: a new process cannot reattach to children it never
        spawned (ADR-0002).
        """
        return self.status == "running"

    async def stop(self) -> None:
        """Gracefully bring this episode to a clean end."""
        raise NotImplementedError

    async def teardown(self) -> None:
        """Hard-stop this episode, leaving no child process behind."""
        raise NotImplementedError


@dataclass
class LiveEpisode(ManagedEpisode):
    """A live episode, backed by a supervised :class:`~freeagent.EpisodeHandle`.

    The handle supervises the episode's child processes in the background, so
    :attr:`status` reflects reality without the registry polling anything.
    """

    handle: EpisodeHandle = field(repr=False, default=None)  # type: ignore[assignment]

    @property
    def status(self) -> str:
        return str(self.handle.state)

    @property
    def detail(self) -> str | None:
        outcome = self.handle.outcome
        return outcome.summary if outcome is not None else None

    async def stop(self) -> None:
        """Graceful operator-abort: the environment broadcasts ``shutdown``,
        agents wind down, and the episode settles ``aborted``."""
        await self.handle.abort(reason="control service stop")

    async def teardown(self) -> None:
        """Hard interrupt: tear every child process down at once and await it."""
        self.handle.request_abort()
        await self.handle.wait()


@dataclass
class ReplayEpisode(ManagedEpisode):
    """A replay episode: a recorded log re-published onto NATS in the background.

    A :class:`~freeagent.Replayer` drives playback as a background task; the
    transport it publishes through is owned here and closed when playback ends
    (whether it finishes, is stopped, or errors).
    """

    _transport: Transport = field(repr=False, default=None)  # type: ignore[assignment]
    _replayer: Replayer = field(repr=False, default=None)  # type: ignore[assignment]
    _status: str = "running"
    _detail: str | None = None
    _task: asyncio.Task[None] | None = field(repr=False, default=None)

    def start(self) -> None:
        """Begin playback in the background (call once, after construction)."""
        self._task = asyncio.ensure_future(self._run())

    async def _run(self) -> None:
        try:
            count = await self._replayer.play()
        except Exception as exc:
            self._status = "error"
            self._detail = f"error ({exc})"
        else:
            # A stop() that won the race already latched a terminal status.
            if self._status == "running":
                self._status = "ended"
                self._detail = f"replayed {count} message(s)"
        finally:
            await self._transport.close()

    @property
    def status(self) -> str:
        return self._status

    @property
    def detail(self) -> str | None:
        return self._detail

    async def stop(self) -> None:
        if self._status == "running":
            self._status = "aborted"
            self._detail = "stopped"
        self._replayer.stop()
        if self._task is not None:
            with contextlib.suppress(Exception):
                await self._task

    async def teardown(self) -> None:
        await self.stop()


class ControlService:
    """Owns the running-episode set and the create/list/get/stop/teardown ops.

    Construct one per service process. *nats_url* is the default NATS URL used
    when a create request does not override it; *app_loader* resolves a dashed
    REST application name to its :class:`~freeagent.AppSpec` (injectable for
    tests).
    """

    def __init__(
        self,
        *,
        nats_url: str,
        app_loader: Callable[[str], AppSpec] = load_app,
    ) -> None:
        self._default_nats_url = nats_url
        self._app_loader = app_loader
        self._episodes: dict[str, ManagedEpisode] = {}
        self._lock = asyncio.Lock()

    @property
    def default_nats_url(self) -> str:
        """The NATS URL used when a create request does not supply its own."""
        return self._default_nats_url

    async def create(self, application: str, request: CreateEpisodeRequest) -> ManagedEpisode:
        """Create (live) or replay an episode for *application*; register and return it.

        Resolves the app (``UnknownAppError`` -> 404), verifies NATS is reachable
        (``NatsUnreachableError`` -> 503), allocates or accepts an id
        (``EpisodeExistsError`` -> 409), and launches. A bad config surfaces as
        ``ConfigError`` -> 400.
        """
        spec = self._app_loader(application)  # UnknownAppError when not installed
        nats_url = request.nats_url or self._default_nats_url
        await verify_nats_reachable(nats_url)
        async with self._lock:
            rest_id = self._allocate_id(request.episode_id)
            if request.mode == "replay":
                episode = await self._launch_replay(application, spec, rest_id, nats_url, request)
            else:
                episode = await self._launch_live(application, spec, rest_id, nats_url, request)
            self._episodes[rest_id] = episode
            return episode

    async def _launch_live(
        self,
        application: str,
        spec: AppSpec,
        rest_id: str,
        nats_url: str,
        request: CreateEpisodeRequest,
    ) -> LiveEpisode:
        """Launch a real episode of *spec* and wrap its supervised handle."""
        config = EpisodeConfig(
            episode_id=rest_id,
            nats_url=nats_url,
            environment=ComponentSpec(config=request.environment.config),
            agents={
                name: ComponentSpec(config=component.config)
                for name, component in request.agents.items()
            },
        )
        plan = make_plan(config, app=spec.name, roster=spec.roster)
        parquet_log = Path(request.parquet_log) if request.parquet_log else None
        handle = await start_episode(plan, spec.environment, spec.roster, parquet_log)
        return LiveEpisode(
            rest_id=rest_id,
            application=application,
            app=spec.name,
            episode_id=plan.episode_id,
            mode="live",
            nats_url=nats_url,
            created_at=_now(),
            handle=handle,
        )

    async def _launch_replay(
        self,
        application: str,
        spec: AppSpec,  # noqa: ARG002 -- accepted for symmetry; replay needs only the log
        rest_id: str,
        nats_url: str,
        request: CreateEpisodeRequest,
    ) -> ReplayEpisode:
        """Replay a recorded Parquet log onto NATS and wrap the playback task."""
        assert request.parquet_path is not None  # guaranteed by request validation
        messages = load_episode(Path(request.parquet_path))  # ReplayerError -> 400
        if not messages:
            from freeagent.replayer import ReplayerError

            raise ReplayerError(f"{request.parquet_path} contains no messages to replay")
        subjects = _subjects_of(messages)
        transport = NatsTransport(nats_url)
        await transport.connect()
        try:
            await _ensure_streams(transport, messages)
        except BaseException:
            await transport.close()
            raise
        episode = ReplayEpisode(
            rest_id=rest_id,
            application=application,
            app=subjects.app,
            episode_id=subjects.episode_id,
            mode="replay",
            nats_url=nats_url,
            created_at=_now(),
            _transport=transport,
            _replayer=Replayer(messages, transport),
        )
        episode.start()
        return episode

    def list(self) -> list[ManagedEpisode]:
        """Every registered episode, in creation order."""
        return list(self._episodes.values())

    def get(self, application: str, rest_id: str) -> ManagedEpisode:
        """The episode registered under *application* and *rest_id*.

        Raises :class:`EpisodeNotFoundError` when no such episode exists, or when
        one exists under that id but a *different* application.
        """
        episode = self._episodes.get(rest_id)
        if episode is None or episode.application != application:
            raise EpisodeNotFoundError(f"no episode {rest_id!r} for application {application!r}")
        return episode

    async def stop(self, application: str, rest_id: str) -> ManagedEpisode:
        """Gracefully stop one episode and return its (now terminal) view."""
        episode = self.get(application, rest_id)
        await episode.stop()
        return episode

    async def teardown(self) -> int:
        """Bring every episode down and clear the registry; return how many.

        The registry is cleared first so an in-flight create cannot slip an
        episode past the teardown, then each one is torn down concurrently with
        a per-episode timeout backstop.
        """
        async with self._lock:
            episodes = list(self._episodes.values())
            self._episodes.clear()
        await asyncio.gather(
            *(self._teardown_one(episode) for episode in episodes),
            return_exceptions=True,
        )
        return len(episodes)

    @staticmethod
    async def _teardown_one(episode: ManagedEpisode) -> None:
        """Tear one episode down, bounded so a wedged child cannot hang teardown."""
        with contextlib.suppress(TimeoutError, asyncio.TimeoutError):
            await asyncio.wait_for(episode.teardown(), timeout=_TEARDOWN_TIMEOUT)

    def _allocate_id(self, requested: str | None) -> str:
        """Validate a client id, or mint a fresh short one; ensure it is free.

        Service-assigned ids are short random hex rather than a counter: random
        ids are not reused after a restart, which matters because the NATS
        JetStream volume is persistent and a reused ``<app>.episode.<id>`` would
        collide with a prior episode's stream. A client may still supply its own
        id (any ``[A-Za-z0-9_-]+``); a collision is a conflict, not a silent
        overwrite.
        """
        if requested is not None:
            rest_id = validate_name(requested, kind="episode id")
            if rest_id in self._episodes:
                raise EpisodeExistsError(f"episode id {rest_id!r} is already in use")
            return rest_id
        while True:
            rest_id = secrets.token_hex(_ID_BYTES)
            if rest_id not in self._episodes:
                return rest_id


def _now() -> datetime:
    """Timezone-aware creation timestamp."""
    return datetime.now(UTC)


def _subjects_of(messages: Iterable[ReplayMessage]) -> EpisodeSubjects:
    """The episode identity a recorded log's subjects belong to.

    Every row of a single episode log shares one ``<app>.episode.<id>`` root;
    the first message is enough to recover it.
    """
    first = next(iter(messages))
    parts = first.subject.split(".")
    if len(parts) < 4 or parts[1] != "episode":
        raise ValueError(f"recorded subject {first.subject!r} is not a FreeAgent episode subject")
    return EpisodeSubjects(app=parts[0], episode_id=parts[2])


async def _ensure_streams(transport: Transport, messages: Iterable[ReplayMessage]) -> None:
    """Create the JetStream stream for every episode root present in *messages*.

    Publishing replayed traffic through JetStream is what lets a viewer attach at
    any moment and read from sequence 1, exactly as it does live.
    """
    roots: dict[str, str] = {}
    for message in messages:
        subjects = EpisodeSubjects.from_root(_root_of(message.subject))
        roots.setdefault(subjects.stream, subjects.all_subjects)
    for stream, all_subjects in roots.items():
        await transport.ensure_stream(stream, [all_subjects])


def _root_of(subject: str) -> str:
    """The ``<app>.episode.<id>`` root of one episode subject."""
    return ".".join(subject.split(".")[:3])
