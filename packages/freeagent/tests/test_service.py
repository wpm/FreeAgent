"""Episode-service tests: the REST API over the durable JetStream record.

Three tiers:

* **Fast, no NATS** -- request-model validation, the NATS-down 503, and the
  unknown-application 404.
* **Fast, in-memory durable record** -- the JetStream-sourcing logic
  (list/get/rename/delete and *restart durability*) driven against an injected
  :class:`MemoryTransport` seeded with episode streams + metadata, exactly as the
  real durable record looks. This is the heart of ADR-0003 and runs without NATS.
* **Slow** (opt-in ``FREEAGENT_SLOW_TEST``, skipped without NATS) -- create
  launches a real episode, observable over NATS; stop aborts it; a fresh service
  over the same NATS still sees it (restart loses nothing).

Live episodes run the no-LLM ``noop_app``; the service is pointed at it through an
injected ``app_loader``, and ``noop_app``'s directory is on ``PYTHONPATH`` so the
spawned children can import it.
"""

from __future__ import annotations

import asyncio
import os
import time
from datetime import UTC, datetime
from pathlib import Path

import nats
import pytest
from httpx import ASGITransport, AsyncClient
from nats.js.api import ConsumerConfig, DeliverPolicy
from noop_app import NoopAgent, NoopEnvironment
from pydantic import ValidationError

from freeagent import (
    AGENT_FIELDS,
    ENVIRONMENT_FIELDS,
    AppSpec,
    Envelope,
    EpisodeMetadata,
    MemoryTransport,
    SettableConfig,
    default_nats_url,
)
from freeagent.service import (
    ControlService,
    CreateEpisodeRequest,
    NatsUnreachableError,
    create_app,
    verify_nats_reachable,
)
from freeagent.subjects import EpisodeSubjects, stream_name

TESTS_DIR = Path(__file__).parent
NATS_URL = default_nats_url()
#: A loopback port nothing listens on: a connect here is refused immediately.
DEAD_NATS_URL = "nats://127.0.0.1:14222"

#: The REST/application name the tests register the noop app under.
NOOP_APP = "noopapp"
#: Long enough that a created episode stays RUNNING for the test to stop it.
LONG_ENV = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.2}

NOOP_SPEC = AppSpec(
    name=NOOP_APP,
    environment=NoopEnvironment,
    roster={"alpha": NoopAgent, "beta": NoopAgent},
    settable_config=SettableConfig(
        environment=ENVIRONMENT_FIELDS,
        agents={"alpha": AGENT_FIELDS, "beta": AGENT_FIELDS},
    ),
)


def _noop_loader(name: str) -> AppSpec:
    """Resolve only the noop app; anything else is an unknown application."""
    from freeagent.cli.apps import UnknownAppError

    if name == NOOP_APP:
        return NOOP_SPEC
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
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        started = time.monotonic()
        response = await client.post(f"/freeagent/{NOOP_APP}/episodes", json={"mode": "live"})
        assert time.monotonic() - started < 8.0
    assert response.status_code == 503
    assert "NATS" in response.json()["detail"]


async def test_create_returns_404_for_unknown_application() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/no-such-app/episodes", json={"mode": "live"})
    assert response.status_code == 404
    assert "unknown application" in response.json()["detail"]


async def test_replay_mode_is_rejected_in_favour_of_import() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
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
    return ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader, transport=transport)


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
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/teardown")
    assert response.status_code == 200
    assert response.json() == {"stopped": 0}


@pytest.mark.parametrize("escaping", ["../../etc/passwd", "/etc/passwd", "a/../../escape.parquet"])
async def test_export_rejects_a_parquet_path_outside_the_volume(
    tmp_path: Path, escaping: str
) -> None:
    """A parquet_path that escapes the configured Parquet directory is a 400 and
    never reaches NATS (path-injection containment)."""
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader, parquet_dir=tmp_path)
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
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader, parquet_dir=tmp_path)
    async with _client(service) as client:
        response = await client.post("/freeagent/import", json={"parquet_path": escaping})
    assert response.status_code == 400
    assert "escapes the Parquet directory" in response.json()["detail"]


# ---------------------------------------------------------------------------
# Slow: the real API against a real NATS server and real child processes
# ---------------------------------------------------------------------------


@pytest.fixture
def _children_can_import_noop_app(monkeypatch: pytest.MonkeyPatch) -> None:
    existing = os.environ.get("PYTHONPATH", "")
    joined = str(TESTS_DIR) if not existing else f"{TESTS_DIR}{os.pathsep}{existing}"
    monkeypatch.setenv("PYTHONPATH", joined)


@slow
@requires_nats
@pytest.mark.usefixtures("_children_can_import_noop_app")
async def test_create_list_get_stop_then_durable() -> None:
    service = ControlService(nats_url=NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        episode_id: str | None = None
        try:
            created = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "name": "first", "environment": {"config": LONG_ENV}},
            )
            assert created.status_code == 201
            view = created.json()
            episode_id = view["id"]
            assert view["status"] == "running"
            assert view["name"] == "first"
            assert view["app"] == NOOP_APP
            assert view["episode_id"] == episode_id
            assert view["subject_root"] == f"{NOOP_APP}.episode.{episode_id}"

            await _await_start(view["subject_root"])

            # list/get read the durable JetStream record.
            listing = (await client.get("/freeagent/episodes")).json()
            assert episode_id in [e["id"] for e in listing]
            got = (await client.get(f"/freeagent/{NOOP_APP}/episodes/{episode_id}")).json()
            assert got["status"] == "running"

            # rename writes the durable record.
            await client.patch(f"/freeagent/{NOOP_APP}/episodes/{episode_id}", json={"name": "two"})
            assert (await client.get(f"/freeagent/{NOOP_APP}/episodes/{episode_id}")).json()[
                "name"
            ] == "two"

            # stop aborts gracefully and seals.
            stopped = await client.post(f"/freeagent/{NOOP_APP}/episodes/{episode_id}/stop")
            assert stopped.status_code == 200
            assert stopped.json()["status"] == "aborted"

            # A restart-equivalent fresh service still sees it (sealed, durable).
            fresh = ControlService(nats_url=NATS_URL, app_loader=_noop_loader)
            try:
                async with _client(fresh) as other:
                    seen = (await other.get(f"/freeagent/{NOOP_APP}/episodes/{episode_id}")).json()
                    assert seen["sealed"] is True
                    assert seen["status"] == "aborted"
            finally:
                await fresh.teardown()
        finally:
            if episode_id is not None:
                # Clean up the persistent volume so the id is never reused.
                await client.request("DELETE", f"/freeagent/{NOOP_APP}/episodes/{episode_id}")
            await service.teardown()


@slow
@requires_nats
async def test_client_supplied_id_and_conflict() -> None:
    service = ControlService(nats_url=NATS_URL, app_loader=_noop_loader)
    chosen = f"chosen-{os.getpid()}-{int(time.time())}"  # unique on the persistent volume
    async with _client(service) as client:
        try:
            first = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "episode_id": chosen, "environment": {"config": LONG_ENV}},
            )
            assert first.status_code == 201
            assert first.json()["id"] == chosen

            conflict = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "episode_id": chosen, "environment": {"config": LONG_ENV}},
            )
            assert conflict.status_code == 409
        finally:
            await client.request("DELETE", f"/freeagent/{NOOP_APP}/episodes/{chosen}")
            await service.teardown()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


async def _await_start(subject_root: str, *, within: float = 15.0) -> None:
    """Subscribe to the episode's control subject and wait for the 'start' gun."""
    client = await nats.connect(NATS_URL)
    started = asyncio.Event()

    async def on_message(msg: object) -> None:
        envelope = Envelope.from_bytes(msg.data)  # type: ignore[attr-defined]
        if isinstance(envelope.payload, dict) and envelope.payload.get("type") == "start":
            started.set()

    try:
        js = client.jetstream()
        deadline = asyncio.get_running_loop().time() + within
        while asyncio.get_running_loop().time() < deadline:
            try:
                sub = await js.subscribe(
                    f"{subject_root}.control",
                    cb=on_message,
                    ordered_consumer=True,
                    config=ConsumerConfig(deliver_policy=DeliverPolicy.ALL),
                )
            except Exception:
                await asyncio.sleep(0.2)
                continue
            try:
                await asyncio.wait_for(started.wait(), timeout=within)
            finally:
                await sub.unsubscribe()
            return
        pytest.fail("episode never broadcast 'start' on NATS")
    finally:
        await client.close()
