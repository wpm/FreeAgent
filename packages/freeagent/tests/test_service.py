"""Episode-service tests: the REST API over the durable JetStream record.

Three tiers:

* **Fast, no NATS** -- request-model validation, the NATS-down 503, and the
  unknown-application 404.
* **Fast, in-memory durable record** -- the JetStream-sourcing logic
  (list/get/rename/delete and *restart durability*) driven against an injected
  :class:`MemoryTransport` seeded with episode streams + metadata, exactly as the
  real durable record looks. This is the heart of ADR-0003 and runs without NATS.
  This tier also covers the worker-pool ``create`` (ADR-0005): it *enqueues* the
  episode's manifests onto the shared work queue and holds **no** handle, and a
  ``stop`` publishes an operator-abort on the episode's ``.env`` subject.
* **Slow** (opt-in ``FREEAGENT_SLOW_TEST``, skipped without NATS) -- create
  enqueues an episode's manifests, observable on the real work queue; a fresh
  service over the same NATS still sees a recorded episode (restart loses
  nothing).

The service is pointed at the noop app through an injected ``manifest_loader``
that returns its :class:`~freeagent.cli.apps.ManifestSpec` -- the class-ref
strings, never the live classes -- so ``create`` resolves an app and enqueues
manifests **without importing any engine** (the slim worker-pool image).
"""

from __future__ import annotations

import os
import time
from datetime import UTC, datetime
from pathlib import Path

import nats
import pytest
from httpx import ASGITransport, AsyncClient
from noop_app import NoopAgent, NoopEnvironment
from pydantic import ValidationError

from freeagent import (
    AGENT_FIELDS,
    ENVIRONMENT_FIELDS,
    AppSpec,
    Envelope,
    EpisodeMetadata,
    Manifest,
    MemoryTransport,
    SettableConfig,
    default_nats_url,
)
from freeagent.cli.apps import ManifestSpec
from freeagent.cli.child import class_ref
from freeagent.environment import OPERATOR_ABORT_TYPE
from freeagent.service import (
    ControlService,
    CreateEpisodeRequest,
    NatsUnreachableError,
    create_app,
    verify_nats_reachable,
)
from freeagent.subjects import EpisodeSubjects, stream_name
from freeagent.workqueue import WORK_QUEUE_STREAM, work_subject

NATS_URL = default_nats_url()
#: A loopback port nothing listens on: a connect here is refused immediately.
DEAD_NATS_URL = "nats://127.0.0.1:14222"

#: The REST/application name the tests register the noop app under.
NOOP_APP = "noopapp"

NOOP_SPEC = AppSpec(
    name=NOOP_APP,
    environment=NoopEnvironment,
    roster={"alpha": NoopAgent, "beta": NoopAgent},
    settable_config=SettableConfig(
        environment=ENVIRONMENT_FIELDS,
        agents={"alpha": AGENT_FIELDS, "beta": AGENT_FIELDS},
    ),
)
#: The engine-free manifest spec the service resolves -- class-ref strings only.
NOOP_MANIFEST = NOOP_SPEC.manifest_spec()


def _noop_loader(name: str) -> ManifestSpec:
    """Resolve only the noop app's :class:`ManifestSpec`; else unknown application.

    Returns class-ref *strings*, mirroring ``load_manifest_spec`` -- the service
    never sees a live engine class, so ``create`` imports no engine.
    """
    from freeagent.cli.apps import UnknownAppError

    if name == NOOP_APP:
        return NOOP_MANIFEST
    raise UnknownAppError(f"unknown application {name!r}; installed applications: {NOOP_APP}")


def _nats_running() -> bool:
    import socket

    try:
        with socket.create_connection(("localhost", 4222), timeout=0.5):
            return True
    except OSError:
        return False


requires_nats = pytest.mark.skipif(
    not _nats_running(),
    reason=(
        "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
    ),
)
slow = pytest.mark.skipif(
    not os.environ.get("FREEAGENT_SLOW_TEST"),
    reason="slow test is opt-in: set FREEAGENT_SLOW_TEST to run it",
)


def _client(service: ControlService) -> AsyncClient:
    """An httpx client driving the app in-process (lifespan handled by the test)."""
    app = create_app(service=service)
    return AsyncClient(transport=ASGITransport(app=app), base_url="http://service")


# ---------------------------------------------------------------------------
# Fast: request-model validation (no NATS, no processes)
# ---------------------------------------------------------------------------


def test_live_is_the_default_mode() -> None:
    assert CreateEpisodeRequest().mode == "live"


def test_create_accepts_name_model_and_key() -> None:
    request = CreateEpisodeRequest(name="my game", model="some-model", api_key="sk-test")
    assert request.name == "my game"
    assert request.model == "some-model"
    assert request.api_key == "sk-test"


def test_replay_requires_a_parquet_path() -> None:
    with pytest.raises(ValidationError, match="requires 'parquet_path'"):
        CreateEpisodeRequest(mode="replay")


def test_unknown_field_is_rejected() -> None:
    with pytest.raises(ValidationError):
        CreateEpisodeRequest(secret="hunter2")  # secret rides in agents.<name>.config


# ---------------------------------------------------------------------------
# Fast: the NATS-down path is a clean error, not a hang
# ---------------------------------------------------------------------------


async def test_verify_nats_unreachable_fails_fast() -> None:
    started = time.monotonic()
    with pytest.raises(NatsUnreachableError):
        await verify_nats_reachable(DEAD_NATS_URL, timeout=3.0)
    assert time.monotonic() - started < 3.0


async def test_create_returns_503_when_nats_is_down() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader)
    async with _client(service) as client:
        started = time.monotonic()
        response = await client.post(f"/freeagent/{NOOP_APP}/episodes", json={"mode": "live"})
        assert time.monotonic() - started < 8.0
    assert response.status_code == 503
    assert "NATS" in response.json()["detail"]


async def test_create_returns_404_for_unknown_application() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/no-such-app/episodes", json={"mode": "live"})
    assert response.status_code == 404
    assert "unknown application" in response.json()["detail"]


async def test_replay_mode_is_rejected_in_favour_of_import() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post(
            f"/freeagent/{NOOP_APP}/episodes",
            json={"mode": "replay", "parquet_path": "/data/log.parquet"},
        )
    assert response.status_code == 400
    assert "import" in response.json()["detail"]


# ---------------------------------------------------------------------------
# Fast: the durable record (JetStream) sourced from an in-memory transport
# ---------------------------------------------------------------------------


def _seed_episode(
    transport: MemoryTransport,
    *,
    episode_id: str,
    name: str,
    status: str,
    sealed: bool,
    outcome: str | None = None,
    created_at: datetime | None = None,
) -> None:
    """Write an episode stream + metadata into the in-memory durable record."""
    subjects = EpisodeSubjects(app=NOOP_APP, episode_id=episode_id)
    meta = EpisodeMetadata(
        app=NOOP_APP,
        name=name,
        status=status,
        mode="live",
        outcome=outcome,
        created_at=(created_at or datetime.now(UTC)).isoformat(),
    ).to_stream_metadata()
    transport.stream_meta[subjects.stream] = meta
    transport.streams[subjects.stream] = (subjects.all_subjects,)
    if sealed:
        transport.sealed_streams.add(subjects.stream)


def _durable_service(transport: MemoryTransport) -> ControlService:
    return ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader, transport=transport)


async def test_list_and_get_read_from_the_durable_record() -> None:
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="alpha", name="amber-otter", status="running", sealed=False)
    _seed_episode(
        transport, episode_id="bravo", name="elephant", status="ended", sealed=True, outcome="won"
    )
    service = _durable_service(transport)
    async with _client(service) as client:
        listing = (await client.get("/freeagent/episodes")).json()
        ids = {e["id"] for e in listing}
        assert ids == {"alpha", "bravo"}
        sealed = next(e for e in listing if e["id"] == "bravo")
        assert sealed["sealed"] is True
        assert sealed["status"] == "ended"
        assert sealed["outcome"] == "won"
        assert sealed["name"] == "elephant"

        got = (await client.get(f"/freeagent/{NOOP_APP}/episodes/alpha")).json()
        assert got["status"] == "running"
        assert got["sealed"] is False


async def test_get_unknown_episode_is_404() -> None:
    service = _durable_service(MemoryTransport())  # empty but reachable record
    async with _client(service) as client:
        response = await client.get(f"/freeagent/{NOOP_APP}/episodes/nope")
    assert response.status_code == 404


async def test_rename_updates_the_durable_record() -> None:
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="bravo", name="elephant", status="ended", sealed=True)
    service = _durable_service(transport)
    async with _client(service) as client:
        renamed = await client.patch(
            f"/freeagent/{NOOP_APP}/episodes/bravo", json={"name": "pachyderm"}
        )
        assert renamed.status_code == 200
        assert renamed.json()["name"] == "pachyderm"
        # Durable: re-reading the record reflects the new name.
        got = (await client.get(f"/freeagent/{NOOP_APP}/episodes/bravo")).json()
        assert got["name"] == "pachyderm"


async def test_delete_removes_the_stream() -> None:
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="bravo", name="elephant", status="ended", sealed=True)
    service = _durable_service(transport)
    async with _client(service) as client:
        deleted = await client.request("DELETE", f"/freeagent/{NOOP_APP}/episodes/bravo")
        assert deleted.status_code == 204
        assert stream_name(NOOP_APP, "bravo") not in transport.streams
        gone = await client.get(f"/freeagent/{NOOP_APP}/episodes/bravo")
        assert gone.status_code == 404


async def test_restart_loses_nothing() -> None:
    """A fresh service over the same durable record still sees every episode."""
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="alpha", name="amber-otter", status="running", sealed=False)
    _seed_episode(transport, episode_id="bravo", name="elephant", status="ended", sealed=True)

    # The durable record (the transport) survives; the service does not.
    first = _durable_service(transport)
    async with _client(first) as client:
        assert {e["id"] for e in (await client.get("/freeagent/episodes")).json()} == {
            "alpha",
            "bravo",
        }
    # A brand-new service instance, same record, no shared in-memory state.
    second = _durable_service(transport)
    async with _client(second) as client:
        assert {e["id"] for e in (await client.get("/freeagent/episodes")).json()} == {
            "alpha",
            "bravo",
        }


async def test_teardown_of_empty_service_is_zero() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/teardown")
    assert response.status_code == 200
    assert response.json() == {"stopped": 0}


# ---------------------------------------------------------------------------
# Fast: provision-only create -- enqueue manifests, hold no handle (ADR-0005)
# ---------------------------------------------------------------------------


def _enqueued_manifests(transport: MemoryTransport, episode_id: str) -> list[Manifest]:
    """The manifests published to *episode_id*'s work-queue subject, decoded."""
    subject = work_subject(NOOP_APP, episode_id)
    return [Manifest.model_validate_json(data) for s, data in transport.history if s == subject]


async def test_create_enqueues_manifests_and_holds_no_handle() -> None:
    """``create`` provisions: it enqueues the manifest set and retains no handle."""
    transport = MemoryTransport()
    await transport.connect()
    service = _durable_service(transport)
    async with _client(service) as client:
        created = await client.post(
            f"/freeagent/{NOOP_APP}/episodes",
            json={"mode": "live", "episode_id": "ep1", "name": "first"},
        )
        assert created.status_code == 201
        view = created.json()
        assert view["id"] == "ep1"
        assert view["app"] == NOOP_APP
        assert view["name"] == "first"
        # No worker has launched anything yet, so the episode is in setup.
        assert view["status"] == "setup"
        assert view["sealed"] is False
        assert view["subject_root"] == f"{NOOP_APP}.episode.ep1"

    # The manifest set landed on the shared work queue: N agents + 1 environment.
    assert WORK_QUEUE_STREAM in transport.work_queue_streams
    manifests = _enqueued_manifests(transport, "ep1")
    assert [m.role for m in manifests] == ["agent", "agent", "environment"]
    env = next(m for m in manifests if m.role == "environment")
    assert env.cls == class_ref(NoopEnvironment)
    assert env.app == NOOP_APP
    assert env.roster == ["alpha", "beta"]
    # The service holds no per-episode handle: provision-only.
    assert service._handles == {}  # type: ignore[attr-defined]


async def test_create_assigns_a_short_id_when_none_is_given() -> None:
    """Omitting ``episode_id`` mints a fresh short id and enqueues under it."""
    transport = MemoryTransport()
    await transport.connect()
    service = _durable_service(transport)
    async with _client(service) as client:
        created = await client.post(f"/freeagent/{NOOP_APP}/episodes", json={"mode": "live"})
        assert created.status_code == 201
        episode_id = created.json()["id"]
    assert episode_id  # a non-empty assigned id
    # The manifests landed under the assigned id's work-queue subject.
    assert _enqueued_manifests(transport, episode_id)


async def test_create_threads_name_model_and_key_into_manifests() -> None:
    """``model``/``api_key`` ride into agent config; ``name`` into the env config."""
    transport = MemoryTransport()
    await transport.connect()
    service = _durable_service(transport)
    async with _client(service) as client:
        await client.post(
            f"/freeagent/{NOOP_APP}/episodes",
            json={
                "mode": "live",
                "episode_id": "ep1",
                "name": "titled",
                "model": "fake",
                "api_key": "sk-test",
                "agents": {"alpha": {"config": {"grace_period": 0.5}}},
            },
        )
    manifests = _enqueued_manifests(transport, "ep1")
    alpha = next(m for m in manifests if m.agent_id == "alpha")
    assert alpha.config == {"grace_period": 0.5, "model": "fake", "api_key": "sk-test"}
    env = next(m for m in manifests if m.role == "environment")
    assert env.config["name"] == "titled"


async def test_create_without_engine_installed_does_not_404() -> None:
    """The slim-image acceptance: an app whose engine is *not importable* still creates.

    ``ghost_engine`` is installed on no machine; a fat worker would import it, but
    the service only needs the class-ref strings to build manifests. ``create``
    must enqueue a well-formed manifest set and never raise the
    ``404 unknown application`` the old in-process ``load_app`` produced.
    """
    transport = MemoryTransport()
    await transport.connect()
    ghost = ManifestSpec(
        name="ghostapp",
        environment="ghost_engine.env:GhostEnvironment",
        roster={"solo": "ghost_engine.agent:GhostAgent"},
    )

    def _ghost_loader(name: str) -> ManifestSpec:
        from freeagent.cli.apps import UnknownAppError

        if name == "ghost-app":
            return ghost
        raise UnknownAppError(f"unknown application {name!r}")

    service = ControlService(
        nats_url=DEAD_NATS_URL, manifest_loader=_ghost_loader, transport=transport
    )
    async with _client(service) as client:
        created = await client.post(
            "/freeagent/ghost-app/episodes",
            json={"mode": "live", "episode_id": "g1"},
        )
        assert created.status_code == 201
        assert created.json()["app"] == "ghostapp"
    subject = work_subject("ghostapp", "g1")
    manifests = [
        Manifest.model_validate_json(data) for s, data in transport.history if s == subject
    ]
    assert [m.role for m in manifests] == ["agent", "environment"]
    env = next(m for m in manifests if m.role == "environment")
    assert env.cls == "ghost_engine.env:GhostEnvironment"


async def test_create_conflict_on_a_recorded_id() -> None:
    """A client id already present in the durable record is a 409 conflict."""
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="taken", name="x", status="running", sealed=False)
    service = _durable_service(transport)
    async with _client(service) as client:
        response = await client.post(
            f"/freeagent/{NOOP_APP}/episodes",
            json={"mode": "live", "episode_id": "taken"},
        )
    assert response.status_code == 409


async def test_stop_publishes_an_operator_abort() -> None:
    """``stop`` is the graceful operator-abort over the episode's ``.env`` subject."""
    transport = MemoryTransport()
    _seed_episode(transport, episode_id="ep1", name="x", status="running", sealed=False)
    service = _durable_service(transport)
    subjects = EpisodeSubjects(app=NOOP_APP, episode_id="ep1")
    async with _client(service) as client:
        stopped = await client.post(f"/freeagent/{NOOP_APP}/episodes/ep1/stop")
        assert stopped.status_code == 200
    abort = [Envelope.from_bytes(data) for s, data in transport.history if s == subjects.env]
    assert len(abort) == 1
    assert isinstance(abort[0].payload, dict)
    assert abort[0].payload["type"] == OPERATOR_ABORT_TYPE


async def test_stop_unknown_episode_is_404() -> None:
    """Stopping an episode the durable record does not know is a clean 404."""
    service = _durable_service(MemoryTransport())
    async with _client(service) as client:
        response = await client.post(f"/freeagent/{NOOP_APP}/episodes/nope/stop")
    assert response.status_code == 404


async def test_teardown_holds_no_children_and_closes_its_transport() -> None:
    """With no handles, teardown brings down nothing and reports zero.

    The service owns no child processes (a worker does), so teardown only closes
    a service-owned transport; an injected transport is left to its owner.
    """
    service = ControlService(nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader)
    assert await service.teardown() == 0


@pytest.mark.parametrize("escaping", ["../../etc/passwd", "/etc/passwd", "a/../../escape.parquet"])
async def test_export_rejects_a_parquet_path_outside_the_volume(
    tmp_path: Path, escaping: str
) -> None:
    """A parquet_path that escapes the configured Parquet directory is a 400 and
    never reaches NATS (path-injection containment)."""
    service = ControlService(
        nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader, parquet_dir=tmp_path
    )
    async with _client(service) as client:
        response = await client.post(
            f"/freeagent/{NOOP_APP}/episodes/whatever/export",
            json={"parquet_path": escaping},
        )
    assert response.status_code == 400
    assert "escapes the Parquet directory" in response.json()["detail"]


@pytest.mark.parametrize("escaping", ["../../etc/passwd", "/etc/passwd"])
async def test_import_rejects_a_parquet_path_outside_the_volume(
    tmp_path: Path, escaping: str
) -> None:
    """Import confines its source path the same way export confines its output."""
    service = ControlService(
        nats_url=DEAD_NATS_URL, manifest_loader=_noop_loader, parquet_dir=tmp_path
    )
    async with _client(service) as client:
        response = await client.post("/freeagent/import", json={"parquet_path": escaping})
    assert response.status_code == 400
    assert "escapes the Parquet directory" in response.json()["detail"]


# ---------------------------------------------------------------------------
# Slow: the real API against a real NATS server and the real work queue
# ---------------------------------------------------------------------------


@slow
@requires_nats
async def test_create_enqueues_onto_the_real_work_queue() -> None:
    """``create`` lands a well-formed manifest set on the real JetStream work queue.

    No worker runs here (that is #52), so the episode never reaches ``running``;
    this test asserts the provisioning side only: the right number of manifests
    are durably on the shared work-queue stream under this episode's subject.

    The work-queue stream forbids overlapping filtered consumers (one shared
    durable, by design), so the count is read from per-subject stream state
    rather than by binding a competing consumer.
    """
    service = ControlService(nats_url=NATS_URL, manifest_loader=_noop_loader)
    chosen = f"wq-{os.getpid()}-{int(time.time())}"
    subject = work_subject(NOOP_APP, chosen)
    async with _client(service) as client:
        try:
            created = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "episode_id": chosen, "name": "first"},
            )
            assert created.status_code == 201
            view = created.json()
            assert view["status"] == "setup"
            assert view["app"] == NOOP_APP

            # Read this episode's manifest count off the shared stream's per-subject
            # state, without creating a (forbidden) overlapping filtered consumer.
            client_nats = await nats.connect(NATS_URL)
            try:
                js = client_nats.jetstream()
                info = await js.stream_info(WORK_QUEUE_STREAM, subjects_filter=subject)
                per_subject = dict(info.state.subjects or {})
            finally:
                await client_nats.close()
            # Two agents + one environment for the noop app's roster.
            assert per_subject.get(subject) == 3
        finally:
            await service.teardown()


@slow
@requires_nats
async def test_client_supplied_id_conflicts_with_a_recorded_episode() -> None:
    """A second create on an id already in the durable record is a 409."""
    transport_service = ControlService(nats_url=NATS_URL, manifest_loader=_noop_loader)
    chosen = f"dup-{os.getpid()}-{int(time.time())}"
    async with _client(transport_service) as client:
        try:
            # Seed the durable record directly by writing a stream + metadata.
            subjects = EpisodeSubjects(app=NOOP_APP, episode_id=chosen)
            client_nats = await nats.connect(NATS_URL)
            try:
                js = client_nats.jetstream()
                meta = EpisodeMetadata(
                    app=NOOP_APP,
                    name="seed",
                    status="running",
                    mode="live",
                    created_at=datetime.now(UTC).isoformat(),
                ).to_stream_metadata()
                from nats.js.api import StreamConfig

                await js.add_stream(
                    StreamConfig(
                        name=subjects.stream,
                        subjects=[subjects.all_subjects],
                        metadata=meta,
                    )
                )
            finally:
                await client_nats.close()

            conflict = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "episode_id": chosen},
            )
            assert conflict.status_code == 409
        finally:
            await client.request("DELETE", f"/freeagent/{NOOP_APP}/episodes/{chosen}")
            await transport_service.teardown()
