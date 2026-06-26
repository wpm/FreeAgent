"""The episode service's brain: JetStream-sourced CRUD + provision-only launch.

:class:`ControlService` owns the operations the REST layer exposes -- create,
list, get, rename, delete, stop, teardown -- with no web framework in sight, so
it is testable on its own.

The atemporal change (ADR-0003): the **durable record is JetStream**, not an
in-memory registry. ``list``/``get`` read the episode streams and their metadata
(:class:`~freeagent.service.store.EpisodeStore`), so a service **restart loses
nothing**.

The worker-pool change (ADR-0005): ``create`` is now **provision-only**. It no
longer launches an episode's agents in-process and holds no
:class:`~freeagent.EpisodeHandle`. Instead it asks the **recruiter** to enqueue
the episode's manifest set onto the shared JetStream work queue
(:mod:`freeagent.workqueue`); a pool of generic workers (#52) later pulls each
manifest and launches it. The service therefore owns **no child processes** and
stores **no PID** -- the durable record is the only truth.

Two consequences of holding no handle:

* The service resolves an app through its **engine-free**
  :class:`~freeagent.cli.apps.ManifestSpec` (class-ref *strings*, never live
  classes), so ``create`` enqueues manifests **without importing the engine** --
  the slim worker-pool image can ``create`` for an app whose engine it lacks,
  with no ``404 unknown application`` (ADR-0005, the wart ADR-0004 accepted).
* ``create`` returns a **synthesized provisioning view** (status ``setup``):
  there is no stream yet (a worker's environment child creates it during setup),
  and the service keeps no record of what it scheduled. ``get``/``list`` read
  JetStream; once the environment writes the stream, the durable record is
  authoritative. There is a brief window after ``create`` and before the stream
  exists during which ``get`` returns 404 -- acceptable, because ADR-0005 makes
  the durable record (not a held handle) the single source of truth.
* ``stop`` is the graceful operator-abort over the episode's ``<root>.env``
  subject (ADR-0002): the environment, run by a worker's child, honors it over
  NATS exactly as before -- no handle needed.

``rename`` sets the friendly-name metadata; ``delete`` removes the whole stream.
``create`` accepts a model and API key and threads them into agent config. The
service is the sole JetStream client and stateless with respect to the durable
record.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import secrets
from datetime import UTC, datetime
from functools import lru_cache
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urlsplit

import nats
from nats.errors import NoServersError

from freeagent.cli.apps import ManifestSpec, load_manifest_spec, load_manifest_specs
from freeagent.cli.config import ComponentSpec, EpisodeConfig, make_plan
from freeagent.control import publish_operator_abort
from freeagent.names import fallback_episode_name
from freeagent.recruiter import enqueue_episode
from freeagent.subjects import subject_root, validate_name
from freeagent.transport import NatsTransport, Transport

from .models import EpisodeView, ExportEpisodeResult
from .store import EpisodeRecord, EpisodeStore

if TYPE_CHECKING:
    from collections.abc import Callable

    from .models import CreateEpisodeRequest

#: How long an episode's id is, in random hex characters.
_ID_BYTES = 4

#: Bound on the startup/create NATS reachability probe.
_NATS_PROBE_TIMEOUT = 8.0

#: Environment variable naming the directory edge I/O (import/export) is confined
#: to. A request's ``parquet_path`` is resolved *within* this directory and may
#: not escape it (see :func:`_resolve_within`).
PARQUET_DIR_ENV_VAR = "FREEAGENT_PARQUET_DIR"

#: Default Parquet directory: the volume the Docker network mounts into the
#: service (``docker/compose.yml``).
DEFAULT_PARQUET_DIR = "/data/parquet"


class ServiceError(Exception):
    """Base for episode-service errors the REST layer maps to status codes."""


class NatsUnreachableError(ServiceError):
    """NATS could not be reached -- a clear error instead of a hang (HTTP 503)."""


class EpisodeNotFoundError(ServiceError):
    """No episode under the requested application and id (HTTP 404)."""


class EpisodeExistsError(ServiceError):
    """The requested episode id is already taken (HTTP 409)."""


class EdgeIOPathError(ServiceError):
    """A Parquet path that escapes the configured Parquet directory (HTTP 400)."""


def _resolve_within(base: Path, requested: str) -> Path:
    """Resolve *requested* under *base*, refusing any path that escapes *base*.

    Edge I/O takes a client-supplied ``parquet_path``; without containment that
    is a path-injection vector (``../../etc/passwd``, an absolute path elsewhere).
    The request is treated as **relative to** the mounted Parquet directory: it is
    joined onto *base*, fully resolved (collapsing ``..`` and symlinks), and
    rejected unless it still sits inside *base*. An absolute ``requested`` joined
    onto *base* still resolves outside it and is rejected -- callers pass a path
    *within* the volume, not a host path.
    """
    base_resolved = base.resolve()
    candidate = (base_resolved / requested).resolve()
    if candidate != base_resolved and base_resolved not in candidate.parents:
        raise EdgeIOPathError(
            f"parquet_path {requested!r} escapes the Parquet directory; "
            "it must be a path within the mounted volume"
        )
    return candidate


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
    REST application name a client addresses it by. Reads the engine-free
    :class:`~freeagent.cli.apps.ManifestSpec` discovery channel (ADR-0005), so a
    slim service builds this mapping without importing any engine.
    """
    return {spec.name: rest_name for rest_name, spec in load_manifest_specs().items()}


def _application_of(app: str) -> str:
    """The dashed REST application name for an undashed subject prefix.

    Falls back to the prefix itself for an app with no installed entry point
    (e.g. an episode imported from a foreign log), so listing never fails.
    """
    return _application_index().get(app, app)


def _provisioning_view(
    *,
    application: str,
    app: str,
    episode_id: str,
    name: str,
    nats_url: str,
    created_at: datetime,
) -> EpisodeView:
    """Synthesize the view ``create`` returns once it has enqueued the manifests.

    The service holds no handle and the durable record does not exist yet (a
    worker's environment child creates the stream during setup, ADR-0005), so
    ``create`` returns what it just *scheduled*: the identity plus a ``setup``
    status. As soon as the environment writes the stream, ``get``/``list`` read
    the durable record, which is then authoritative.
    """
    return EpisodeView(
        id=episode_id,
        application=application,
        app=app,
        episode_id=episode_id,
        subject_root=subject_root(app, episode_id),
        name=name,
        mode="live",
        status="setup",
        sealed=False,
        outcome=None,
        detail=None,
        nats_url=nats_url,
        created_at=created_at,
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
        manifest_set=record.manifest_set,
        resolved_versions=record.resolved_versions,
    )


class ControlService:
    """Owns create/list/get/rename/delete/stop/teardown over the durable record.

    Construct one per service process. *nats_url* is the default NATS URL (the
    durable record the service reads). *manifest_loader* resolves a dashed REST
    name to its engine-free :class:`~freeagent.cli.apps.ManifestSpec` (class-ref
    strings, injectable for tests); the default reads the app's discovery entry
    point **without importing the engine**, so a slim image can ``create``.
    *transport* injects a ready, connected transport for the durable store and
    the work-queue publish (tests use an in-memory one); when omitted, the
    service lazily connects its own. *parquet_dir* (or
    ``$FREEAGENT_PARQUET_DIR``) is the directory edge I/O is confined to; a
    request's ``parquet_path`` is resolved within it and may not escape it.

    The service holds **no** episode handles (ADR-0005): launching is the worker
    pool's job. ``self._handles`` is retained only as an always-empty marker for
    that invariant -- a created episode lives in JetStream, never in the service.
    """

    def __init__(
        self,
        *,
        nats_url: str,
        manifest_loader: Callable[[str], ManifestSpec] = load_manifest_spec,
        transport: Transport | None = None,
        parquet_dir: str | Path | None = None,
    ) -> None:
        self._default_nats_url = nats_url
        self._manifest_loader = manifest_loader
        #: Always empty: the service provisions and holds no handle (ADR-0005).
        self._handles: dict[str, object] = {}
        self._lock = asyncio.Lock()
        self._transport = transport
        self._owns_transport = transport is None
        resolved_dir = (
            parquet_dir
            if parquet_dir is not None
            else os.environ.get(PARQUET_DIR_ENV_VAR, DEFAULT_PARQUET_DIR)
        )
        self._parquet_dir = Path(resolved_dir)

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
        """Provision a live episode for *application*: enqueue its manifests.

        Provision-only (ADR-0005): the service resolves the app's engine-free
        manifest spec (``UnknownAppError`` -> 404), verifies NATS is reachable
        (``NatsUnreachableError`` -> 503), allocates or accepts an id
        (``EpisodeExistsError`` -> 409), and **enqueues** the episode's manifest
        set onto the shared work queue via the recruiter. A pool of workers (#52)
        launches it. The service holds **no** handle; it returns a synthesized
        ``setup`` view (the stream does not exist yet -- a worker's environment
        child creates it). A ``replay`` request is rejected: replaying a recorded
        log is now an import (``POST /freeagent/import``), per ADR-0003.
        """
        if request.mode == "replay":
            raise ValueError(
                "replay mode is retired: import a recorded log with POST /freeagent/import"
            )
        spec = self._manifest_loader(application)  # UnknownAppError when not installed
        nats_url = request.nats_url or self._default_nats_url
        # Probe only when we would connect our own transport: an already-connected
        # injected transport (tests; a warm service) proves NATS is reachable, and
        # the recruiter publishes over it -- a redundant probe would force the
        # default URL even when a request overrides it.
        if self._transport is None:
            await verify_nats_reachable(nats_url)
        async with self._lock:
            episode_id = await self._allocate_id(spec, request.episode_id)
            return await self._enqueue(application, spec, episode_id, nats_url, request)

    async def _enqueue(
        self,
        application: str,
        spec: ManifestSpec,
        episode_id: str,
        nats_url: str,
        request: CreateEpisodeRequest,
    ) -> EpisodeView:
        """Build the episode's plan, enqueue its manifests, return a setup view.

        The friendly *name* (if any) rides into the environment config so the
        worker's environment child writes it onto the stream; ``model`` and
        ``api_key`` ride into every agent's config so the operator need not spell
        out per-agent blocks. Recording (``parquet_log``) is no longer started
        here -- the worker owns launching; the field is ignored on this path.
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
        transport = await self._ensure_transport()
        await enqueue_episode(transport, spec=spec, plan=plan)
        name = request.name or fallback_episode_name(episode_id)
        return _provisioning_view(
            application=application,
            app=spec.name,
            episode_id=episode_id,
            name=name,
            nats_url=nats_url,
            created_at=_now(),
        )

    async def list(self) -> list[EpisodeView]:
        """Every episode in the durable record, newest first.

        The JetStream record is the sole authority for existence (ADR-0005): the
        service holds no handle, so a just-provisioned episode appears here only
        once a worker's environment child has written its stream.
        """
        store = await self._store()
        views = [
            _view_from_record(record, nats_url=self._default_nats_url)
            for record in await store.list()
        ]
        return sorted(views, key=lambda v: v.created_at, reverse=True)

    async def get(self, application: str, episode_id: str) -> EpisodeView:
        """The episode under *application* and *episode_id*, from the durable record.

        Reads only JetStream (ADR-0005). Raises :class:`EpisodeNotFoundError`
        when the durable record does not know it -- including the brief window
        after ``create`` and before a worker's environment child has written the
        stream.
        """
        spec = self._manifest_loader(application)
        record = await (await self._store()).get(spec.name, episode_id)
        if record is not None:
            return _view_from_record(record, nats_url=self._default_nats_url)
        raise EpisodeNotFoundError(f"no episode {episode_id!r} for application {application!r}")

    async def rename(self, application: str, episode_id: str, name: str) -> EpisodeView:
        """Set an episode's friendly name and return its updated view."""
        spec = self._manifest_loader(application)
        store = await self._store()
        if await store.get(spec.name, episode_id) is None:
            raise EpisodeNotFoundError(f"no episode {episode_id!r} for application {application!r}")
        await store.rename(spec.name, episode_id, name)
        return await self.get(application, episode_id)

    async def delete(self, application: str, episode_id: str) -> None:
        """Delete an episode: remove its stream from the durable record.

        The service owns no child processes (a worker does), so there is nothing
        local to bring down -- deleting the stream is the whole operation. A
        still-running episode's worker children notice the vanished stream and
        wind down (ADR-0005's "a lost room aborts the episode").
        """
        spec = self._manifest_loader(application)
        await (await self._store()).delete(spec.name, episode_id)

    async def export(
        self, application: str, episode_id: str, parquet_path: str
    ) -> ExportEpisodeResult:
        """Drain a sealed episode to a Parquet file on the mounted volume.

        *parquet_path* is resolved within the configured Parquet directory and may
        not escape it (:class:`EdgeIOPathError` -> 400).
        """
        from .edgeio import export_episode  # local import: avoids an import cycle

        spec = self._manifest_loader(application)
        output = _resolve_within(self._parquet_dir, parquet_path)
        rows = await export_episode(
            nats_url=self._default_nats_url,
            app=spec.name,
            episode_id=episode_id,
            output=output,
        )
        return ExportEpisodeResult(parquet_path=parquet_path, rows=rows)

    async def import_(
        self, parquet_path: str, *, episode_id: str | None = None, name: str | None = None
    ) -> EpisodeView:
        """Play a Parquet log into a fresh sealed episode; return its view.

        *parquet_path* is resolved within the configured Parquet directory and may
        not escape it (:class:`EdgeIOPathError` -> 400).
        """
        from .edgeio import import_episode  # local import: avoids an import cycle

        source = _resolve_within(self._parquet_dir, parquet_path)
        transport = await self._ensure_transport()
        result = await import_episode(
            transport, parquet_path=source, episode_id=episode_id, name=name
        )
        record = await (await self._store()).get(result.app, result.episode_id)
        if record is None:  # pragma: no cover - the stream was just created
            raise EpisodeNotFoundError(f"imported episode {result.episode_id!r} not found")
        return _view_from_record(record, nats_url=self._default_nats_url)

    async def stop(self, application: str, episode_id: str) -> EpisodeView:
        """Gracefully stop one episode and return its current view.

        A graceful **operator-abort** over the episode's ``<root>.env`` subject
        (ADR-0002/0005): the service holds no handle, so it publishes the request
        (:func:`~freeagent.control.publish_operator_abort`) and the environment --
        run by a worker's child -- broadcasts ``shutdown``, agents wind down, and
        the episode settles ``aborted`` and seals. The episode must exist in the
        durable record; otherwise this is a clean 404 (the same window-before-the-
        stream caveat as ``get``).
        """
        spec = self._manifest_loader(application)
        record = await (await self._store()).get(spec.name, episode_id)
        if record is None:
            raise EpisodeNotFoundError(f"no episode {episode_id!r} for application {application!r}")
        if not record.sealed:
            transport = await self._ensure_transport()
            await publish_operator_abort(
                transport, spec.name, episode_id, reason="episode service stop"
            )
        return await self.get(application, episode_id)

    async def teardown(self) -> int:
        """A no-op over episodes; closes a service-owned transport. Returns 0.

        The service owns no child processes (ADR-0005: a worker forks and reaps
        them), so there is nothing to bring down -- teardown only releases a
        transport the service connected itself, leaving the durable record intact.
        Always returns ``0`` (no episodes were stopped here), preserving the
        ``{"stopped": N}`` shape the REST layer reports.
        """
        if self._owns_transport and self._transport is not None:
            with contextlib.suppress(Exception):
                await self._transport.close()
            self._transport = None
        return 0

    async def _allocate_id(self, spec: ManifestSpec, requested: str | None) -> str:
        """Validate a client id, or mint a fresh short one; ensure it is free.

        Service-assigned ids are short random hex rather than a counter: random
        ids are not reused after a restart, which matters because the JetStream
        volume is persistent and a reused id would collide with a prior episode's
        stream. A client may supply its own id; a collision with an existing
        durable record is a conflict. With no held handles (ADR-0005), the
        durable record is the only place a collision can be detected here -- a
        same-instant double-create of an unwritten id is resolved when the
        environment creates the stream.
        """
        store = await self._store()
        if requested is not None:
            episode_id = validate_name(requested, kind="episode id")
            if await store.get(spec.name, episode_id) is not None:
                raise EpisodeExistsError(f"episode id {episode_id!r} is already in use")
            return episode_id
        while True:
            episode_id = secrets.token_hex(_ID_BYTES)
            if await store.get(spec.name, episode_id) is None:
                return episode_id


def _now() -> datetime:
    """Timezone-aware creation timestamp."""
    return datetime.now(UTC)
