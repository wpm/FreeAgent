"""The episode service's brain: JetStream-sourced CRUD + lifecycle control.

:class:`ControlService` owns the operations the REST layer exposes -- create,
list, get, rename, delete, stop, teardown -- with no web framework in sight, so
it is testable on its own.

The atemporal change (ADR-0003): the **durable record is JetStream**, not an
in-memory registry. ``list``/``get`` read the episode streams and their metadata
(:class:`~freeagent.service.store.EpisodeStore`), so a service **restart loses
nothing** -- the limitation ADR-0002 named is resolved. The service still holds a
live :class:`~freeagent.EpisodeHandle` for each episode *it* launched, to drive
``stop`` and to report the most current status while an episode is still being
created (before its stream exists); but an episode's existence no longer depends
on the service remembering it.

``rename`` sets the friendly-name metadata; ``delete`` removes the whole stream.
``create`` accepts a model and API key and threads them into agent config. The
service is the sole JetStream client and stateless with respect to the durable
record.
"""

from __future__ import annotations

import asyncio
import contextlib
import secrets
from dataclasses import dataclass
from datetime import UTC, datetime
from functools import lru_cache
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urlsplit

import nats
from nats.errors import NoServersError

from freeagent.cli.apps import AppSpec, load_app, load_apps
from freeagent.cli.config import ComponentSpec, EpisodeConfig, make_plan
from freeagent.cli.orchestrate import EpisodeHandle, EpisodeStatus, start_episode
from freeagent.names import fallback_episode_name
from freeagent.subjects import subject_root, validate_name
from freeagent.transport import NatsTransport, Transport

from .models import EpisodeView
from .store import EpisodeRecord, EpisodeStore

if TYPE_CHECKING:
    from collections.abc import Callable

    from .models import CreateEpisodeRequest

#: How long an episode's id is, in random hex characters.
_ID_BYTES = 4

#: Bound on the startup/create NATS reachability probe.
_NATS_PROBE_TIMEOUT = 8.0

#: Bound on tearing one episode down, so a wedged child cannot hang teardown.
_TEARDOWN_TIMEOUT = 30.0


class ServiceError(Exception):
    """Base for episode-service errors the REST layer maps to status codes."""


class NatsUnreachableError(ServiceError):
    """NATS could not be reached -- a clear error instead of a hang (HTTP 503)."""


class EpisodeNotFoundError(ServiceError):
    """No episode under the requested application and id (HTTP 404)."""


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

    Two bounded stages, each guarded by ``wait_for`` so neither can hang: a TCP
    reachability gate a closed port refuses immediately, then a full nats-py
    connect (reconnection disabled) to confirm something that speaks NATS is
    listening. Deliberately not :class:`~freeagent.NatsTransport`, whose
    reconnect tuning is wrong for a one-shot liveness check.
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


@lru_cache(maxsize=1)
def _application_index() -> dict[str, str]:
    """Map every installed app's undashed subject prefix to its dashed REST name.

    Cached: the installed-app set does not change within a process. Used to label
    an episode read from JetStream (which stores the undashed ``app``) with the
    REST application name a client addresses it by.
    """
    return {spec.name: rest_name for rest_name, spec in load_apps().items()}


def _application_of(app: str) -> str:
    """The dashed REST application name for an undashed subject prefix.

    Falls back to the prefix itself for an app with no installed entry point
    (e.g. an episode imported from a foreign log), so listing never fails.
    """
    return _application_index().get(app, app)


@dataclass
class _LiveEpisode:
    """One episode the service launched: its handle plus the identity it carries.

    The handle supervises the child processes in the background, so the live
    status reflects reality without polling. This is what the service uses to
    drive ``stop`` and to answer ``get`` for an episode whose stream does not yet
    exist (the environment creates it a moment after launch).
    """

    handle: EpisodeHandle
    application: str
    app: str
    episode_id: str
    name: str
    nats_url: str
    created_at: datetime

    @property
    def status(self) -> str:
        return str(self.handle.state)

    @property
    def sealed(self) -> bool:
        return self.handle.state is not EpisodeStatus.RUNNING

    @property
    def outcome(self) -> str | None:
        if self.handle.state is EpisodeStatus.ABORTED:
            return "aborted"
        if self.handle.state is EpisodeStatus.ERROR:
            return "error"
        return None

    @property
    def detail(self) -> str | None:
        result = self.handle.outcome
        return result.summary if result is not None else None

    def view(self) -> EpisodeView:
        return EpisodeView(
            id=self.episode_id,
            application=self.application,
            app=self.app,
            episode_id=self.episode_id,
            subject_root=subject_root(self.app, self.episode_id),
            name=self.name,
            mode="live",
            status=self.status,
            sealed=self.sealed,
            outcome=self.outcome,
            detail=self.detail,
            nats_url=self.nats_url,
            created_at=self.created_at,
        )


def _view_from_record(record: EpisodeRecord, *, nats_url: str) -> EpisodeView:
    """Project a durable record onto the REST view."""
    return EpisodeView(
        id=record.episode_id,
        application=record.application,
        app=record.app,
        episode_id=record.episode_id,
        subject_root=subject_root(record.app, record.episode_id),
        name=record.name,
        mode=record.mode,  # type: ignore[arg-type]
        status=record.status,
        sealed=record.sealed,
        outcome=record.outcome,
        detail=None,
        nats_url=nats_url,
        created_at=record.created_at or _now(),
    )


class ControlService:
    """Owns create/list/get/rename/delete/stop/teardown over the durable record.

    Construct one per service process. *nats_url* is the default NATS URL (the
    durable record the service reads). *app_loader* resolves a dashed REST name
    to its :class:`~freeagent.AppSpec` (injectable for tests). *transport*
    injects a ready, connected transport for the durable store (tests use an
    in-memory one); when omitted, the service lazily connects its own.
    """

    def __init__(
        self,
        *,
        nats_url: str,
        app_loader: Callable[[str], AppSpec] = load_app,
        transport: Transport | None = None,
    ) -> None:
        self._default_nats_url = nats_url
        self._app_loader = app_loader
        self._handles: dict[str, _LiveEpisode] = {}
        self._lock = asyncio.Lock()
        self._transport = transport
        self._owns_transport = transport is None

    @property
    def default_nats_url(self) -> str:
        """The NATS URL used when a create request does not supply its own."""
        return self._default_nats_url

    # ------------------------------------------------------------------
    # Durable store access (the JetStream source of truth)
    # ------------------------------------------------------------------

    async def _store(self) -> EpisodeStore:
        """The durable-record view, lazily connecting the service's transport."""
        transport = await self._ensure_transport()
        return EpisodeStore(transport, application_of=_application_of)

    async def _ensure_transport(self) -> Transport:
        if self._transport is None:
            transport = NatsTransport(self._default_nats_url)
            try:
                await transport.connect()
            except Exception as exc:
                raise _unreachable(self._default_nats_url) from exc
            self._transport = transport
        return self._transport

    # ------------------------------------------------------------------
    # Commands
    # ------------------------------------------------------------------

    async def create(self, application: str, request: CreateEpisodeRequest) -> EpisodeView:
        """Launch a live episode for *application*; register and return its view.

        Resolves the app (``UnknownAppError`` -> 404), verifies NATS is reachable
        (``NatsUnreachableError`` -> 503), allocates or accepts an id
        (``EpisodeExistsError`` -> 409), and launches with the chosen model/key.
        A ``replay`` request is rejected: replaying a recorded log is now an
        import (``POST /freeagent/import``), per ADR-0003.
        """
        if request.mode == "replay":
            raise ValueError(
                "replay mode is retired: import a recorded log with POST /freeagent/import"
            )
        spec = self._app_loader(application)  # UnknownAppError when not installed
        nats_url = request.nats_url or self._default_nats_url
        await verify_nats_reachable(nats_url)
        async with self._lock:
            episode_id = self._allocate_id(request.episode_id)
            live = await self._launch_live(application, spec, episode_id, nats_url, request)
            self._handles[episode_id] = live
            return live.view()

    async def _launch_live(
        self,
        application: str,
        spec: AppSpec,
        episode_id: str,
        nats_url: str,
        request: CreateEpisodeRequest,
    ) -> _LiveEpisode:
        """Launch a real episode of *spec* and wrap its supervised handle.

        The friendly *name* (if any) rides into the environment config so the
        engine writes it onto the stream; ``model`` and ``api_key`` ride into
        every agent's config so the operator need not spell out per-agent blocks.
        """
        env_config = dict(request.environment.config)
        if request.name:
            env_config["name"] = request.name
        agent_extra: dict[str, str] = {}
        if request.model:
            agent_extra["model"] = request.model
        if request.api_key:
            agent_extra["api_key"] = request.api_key
        agents = {
            name: ComponentSpec(
                config={
                    **dict(request.agents[name].config if name in request.agents else {}),
                    **agent_extra,
                }
            )
            for name in spec.roster
        }
        config = EpisodeConfig(
            episode_id=episode_id,
            nats_url=nats_url,
            environment=ComponentSpec(config=env_config),
            agents=agents,
        )
        plan = make_plan(config, app=spec.name, roster=spec.roster)
        parquet_log = Path(request.parquet_log) if request.parquet_log else None
        handle = await start_episode(plan, spec.environment, spec.roster, parquet_log)
        name = request.name or fallback_episode_name(episode_id)
        return _LiveEpisode(
            handle=handle,
            application=application,
            app=spec.name,
            episode_id=episode_id,
            name=name,
            nats_url=nats_url,
            created_at=_now(),
        )

    async def list(self) -> list[EpisodeView]:
        """Every episode in the durable record, overlaid with live status.

        The JetStream record is the authority for existence; a live handle the
        service holds overrides status for an episode whose stream is not yet
        written (the just-created, setup phase).
        """
        store = await self._store()
        views: dict[str, EpisodeView] = {
            record.episode_id: _view_from_record(record, nats_url=self._default_nats_url)
            for record in await store.list()
        }
        for episode_id, live in self._handles.items():
            views.setdefault(episode_id, live.view())
        return sorted(views.values(), key=lambda v: v.created_at, reverse=True)

    async def get(self, application: str, episode_id: str) -> EpisodeView:
        """The episode under *application* and *episode_id*, from the durable record.

        Falls back to a live handle for an episode whose stream is not yet
        written. Raises :class:`EpisodeNotFoundError` when neither knows it.
        """
        spec = self._app_loader(application)
        record = await (await self._store()).get(spec.name, episode_id)
        if record is not None:
            return _view_from_record(record, nats_url=self._default_nats_url)
        live = self._handles.get(episode_id)
        if live is not None and live.application == application:
            return live.view()
        raise EpisodeNotFoundError(f"no episode {episode_id!r} for application {application!r}")

    async def rename(self, application: str, episode_id: str, name: str) -> EpisodeView:
        """Set an episode's friendly name and return its updated view."""
        spec = self._app_loader(application)
        store = await self._store()
        if await store.get(spec.name, episode_id) is None and episode_id not in self._handles:
            raise EpisodeNotFoundError(f"no episode {episode_id!r} for application {application!r}")
        await store.rename(spec.name, episode_id, name)
        live = self._handles.get(episode_id)
        if live is not None:
            live.name = name
        return await self.get(application, episode_id)

    async def delete(self, application: str, episode_id: str) -> None:
        """Delete an episode: stop it if it is live, then remove its stream."""
        spec = self._app_loader(application)
        live = self._handles.pop(episode_id, None)
        if live is not None:
            await self._teardown_one(live)
        await (await self._store()).delete(spec.name, episode_id)

    async def stop(self, application: str, episode_id: str) -> EpisodeView:
        """Gracefully stop one live episode and return its (terminal) view.

        A graceful operator-abort: the environment broadcasts ``shutdown``,
        agents wind down, and the episode settles ``aborted`` and seals.
        """
        live = self._handles.get(episode_id)
        if live is None or live.application != application:
            # Not a live episode the service owns. It may already be sealed in the
            # durable record; report it if so, else it truly does not exist.
            return await self.get(application, episode_id)
        await live.handle.abort(reason="episode service stop")
        return await self.get(application, episode_id)

    async def teardown(self) -> int:
        """Bring every live episode down; return how many. Does not delete streams.

        Teardown stops the service's child processes (so nothing is orphaned) but
        leaves the durable record intact -- a stopped episode is sealed, not gone.
        """
        async with self._lock:
            live = list(self._handles.values())
            self._handles.clear()
        await asyncio.gather(
            *(self._teardown_one(episode) for episode in live), return_exceptions=True
        )
        if self._owns_transport and self._transport is not None:
            with contextlib.suppress(Exception):
                await self._transport.close()
            self._transport = None
        return len(live)

    @staticmethod
    async def _teardown_one(live: _LiveEpisode) -> None:
        """Tear one live episode down, bounded so a wedged child cannot hang it."""
        with contextlib.suppress(TimeoutError, asyncio.TimeoutError):
            live.handle.request_abort()
            await asyncio.wait_for(live.handle.wait(), timeout=_TEARDOWN_TIMEOUT)

    def _allocate_id(self, requested: str | None) -> str:
        """Validate a client id, or mint a fresh short one; ensure it is free.

        Service-assigned ids are short random hex rather than a counter: random
        ids are not reused after a restart, which matters because the JetStream
        volume is persistent and a reused id would collide with a prior episode's
        stream. A client may supply its own id; a live-handle collision is a
        conflict (a durable-record collision surfaces when the env creates the
        stream).
        """
        if requested is not None:
            episode_id = validate_name(requested, kind="episode id")
            if episode_id in self._handles:
                raise EpisodeExistsError(f"episode id {episode_id!r} is already in use")
            return episode_id
        while True:
            episode_id = secrets.token_hex(_ID_BYTES)
            if episode_id not in self._handles:
                return episode_id


def _now() -> datetime:
    """Timezone-aware creation timestamp."""
    return datetime.now(UTC)
