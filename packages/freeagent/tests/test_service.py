"""Control-service tests: the REST API over running episodes.

Two tiers, mirroring the rest of the suite:

* **Fast** tests need neither NATS nor child processes -- request-model
  validation, the NATS-down 503 (a *closed* port, so it fails fast), and the
  unknown-application 404.
* **Slow** tests (opt-in ``FREEAGENT_SLOW_TEST``, skipped without NATS) drive the
  real thing: create launches an episode observable over NATS exactly like a
  ``run``-launched one, list/get reflect it, stop aborts it gracefully, teardown
  clears everything, and a recorded log replays onto NATS.

Live episodes run the no-LLM ``noop_app`` (a Player-free environment plus idle
agents); the service is pointed at it through an injected ``app_loader`` so the
tests need no installed entry point, and ``noop_app``'s directory is exported on
``PYTHONPATH`` so the spawned children can import it.
"""

from __future__ import annotations

import asyncio
import os
import socket
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
    EpisodeSubjects,
    SettableConfig,
    default_nats_url,
    make_record,
    write_parquet,
)
from freeagent.service import (
    ControlService,
    CreateEpisodeRequest,
    NatsUnreachableError,
    create_app,
    verify_nats_reachable,
)

TESTS_DIR = Path(__file__).parent
NATS_URL = default_nats_url()
#: A loopback port nothing listens on: a connect here is refused immediately, so
#: the NATS-down path is exercised without a hang and without a real server.
DEAD_NATS_URL = "nats://127.0.0.1:14222"

#: The REST/application name the tests register the noop app under.
NOOP_APP = "noopapp"
#: Long enough that a created episode stays RUNNING for the test to observe and
#: stop it -- the abort, never a timeout, is what ends it.
LONG_ENV = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.2}

#: The noop app the service launches: two idle agents under one base environment.
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


# ---------------------------------------------------------------------------
# Fast: request-model validation (no NATS, no processes)
# ---------------------------------------------------------------------------


def test_live_is_the_default_mode() -> None:
    request = CreateEpisodeRequest()
    assert request.mode == "live"


def test_replay_requires_a_parquet_path() -> None:
    with pytest.raises(ValidationError, match="requires 'parquet_path'"):
        CreateEpisodeRequest(mode="replay")


def test_parquet_path_is_replay_only() -> None:
    with pytest.raises(ValidationError, match="applies only to replay mode"):
        CreateEpisodeRequest(mode="live", parquet_path="/tmp/log.parquet")


def test_replay_rejects_inapplicable_live_fields() -> None:
    with pytest.raises(ValidationError, match="do not apply to replay mode"):
        CreateEpisodeRequest(
            mode="replay",
            parquet_path="/tmp/log.parquet",
            agents={"alice": {"config": {"model": "fake"}}},
        )


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
    # A refused port returns at once; the bound is the no-hang guarantee.
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
    # The app is resolved before NATS is probed, so a closed port is irrelevant.
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/no-such-app/episodes", json={"mode": "live"})
    assert response.status_code == 404
    assert "unknown application" in response.json()["detail"]


async def test_get_unknown_episode_is_404() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.get(f"/freeagent/{NOOP_APP}/episodes/nope")
    assert response.status_code == 404


async def test_teardown_of_empty_service_is_zero() -> None:
    service = ControlService(nats_url=DEAD_NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        response = await client.post("/freeagent/teardown")
    assert response.status_code == 200
    assert response.json() == {"stopped": 0}


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
async def test_create_list_get_stop_teardown() -> None:
    service = ControlService(nats_url=NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        try:
            # --- create: a live episode comes up RUNNING.
            created = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "live", "environment": {"config": LONG_ENV}},
            )
            assert created.status_code == 201
            view = created.json()
            episode_id = view["id"]
            assert view["mode"] == "live"
            assert view["status"] == "running"
            assert view["app"] == NOOP_APP
            # A running episode is controllable: the service holds a handle it can
            # gracefully stop, so the browser's discovery overlay offers Stop.
            assert view["controllable"] is True
            # The REST id IS the subject id for a live episode.
            assert view["episode_id"] == episode_id
            assert view["subject_root"] == f"{NOOP_APP}.episode.{episode_id}"

            # --- observable over NATS exactly like a run-launched episode: the
            # environment confirms presence and broadcasts 'start' on control.
            await _await_start(view["subject_root"])

            # --- list / get reflect reality.
            listing = (await client.get("/freeagent/episodes")).json()
            assert [e["id"] for e in listing] == [episode_id]
            got = (await client.get(f"/freeagent/{NOOP_APP}/episodes/{episode_id}")).json()
            assert got["status"] == "running"

            # --- stop aborts gracefully: the episode settles 'aborted'.
            stopped = await client.post(f"/freeagent/{NOOP_APP}/episodes/{episode_id}/stop")
            assert stopped.status_code == 200
            assert stopped.json()["status"] == "aborted"
            # Once terminal there is nothing left to stop, so it is no longer
            # controllable -- discovery shows it watchable, but without Stop.
            assert stopped.json()["controllable"] is False

            # --- teardown clears everything.
            torn = await client.post("/freeagent/teardown")
            assert torn.json() == {"stopped": 1}
            assert (await client.get("/freeagent/episodes")).json() == []
        finally:
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
            await service.teardown()


@slow
@requires_nats
async def test_replay_round_trips_onto_nats(tmp_path: Path) -> None:
    recorded_id = f"rec-{os.getpid()}-{int(time.time())}"
    log = tmp_path / "episode.parquet"
    _write_episode_log(log, app=NOOP_APP, episode_id=recorded_id, count=3)

    service = ControlService(nats_url=NATS_URL, app_loader=_noop_loader)
    async with _client(service) as client:
        try:
            created = await client.post(
                f"/freeagent/{NOOP_APP}/episodes",
                json={"mode": "replay", "parquet_path": str(log)},
            )
            assert created.status_code == 201
            view = created.json()
            assert view["mode"] == "replay"
            # The subject identity comes from the recording, not the REST id.
            assert view["subject_root"] == f"{NOOP_APP}.episode.{recorded_id}"
            assert view["episode_id"] == recorded_id

            # Playback finishes; status reflects it.
            await _await_status(client, NOOP_APP, view["id"], "ended")

            # The replayed messages are on NATS, captured by the episode stream.
            subjects = EpisodeSubjects(app=NOOP_APP, episode_id=recorded_id)
            messages = await _read_stream(subjects, expected=3)
            assert len(messages) == 3
            assert all(env.episode_id == recorded_id for _, env in messages)
        finally:
            await service.teardown()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _client(service: ControlService) -> AsyncClient:
    """An httpx client driving the app in-process (lifespan handled by the test)."""
    app = create_app(service=service)
    return AsyncClient(transport=ASGITransport(app=app), base_url="http://service")


async def _await_status(
    client: AsyncClient, application: str, episode_id: str, want: str, *, within: float = 15.0
) -> None:
    """Poll GET until the episode reports *want* (or fail the test)."""
    deadline = asyncio.get_running_loop().time() + within
    while asyncio.get_running_loop().time() < deadline:
        body = (await client.get(f"/freeagent/{application}/episodes/{episode_id}")).json()
        if body["status"] == want:
            return
        await asyncio.sleep(0.2)
    pytest.fail(f"episode never reached status {want!r}")


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
            except Exception:  # stream not created yet; retry until the deadline
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


async def _read_stream(
    subjects: EpisodeSubjects, *, expected: int, within: float = 10.0
) -> list[tuple[str, Envelope]]:
    """Read an episode's JetStream stream from sequence 1 until *expected* messages."""
    client = await nats.connect(NATS_URL)
    try:
        js = client.jetstream()
        messages: list[tuple[str, Envelope]] = []
        complete = asyncio.Event()

        async def callback(msg: object) -> None:
            messages.append((msg.subject, Envelope.from_bytes(msg.data)))  # type: ignore[attr-defined]
            if len(messages) >= expected:
                complete.set()

        sub = await js.subscribe(
            subjects.all_subjects,
            cb=callback,
            ordered_consumer=True,
            config=ConsumerConfig(deliver_policy=DeliverPolicy.ALL),
        )
        await asyncio.wait_for(complete.wait(), timeout=within)
        await sub.unsubscribe()
        return messages
    finally:
        await client.close()


def _write_episode_log(path: Path, *, app: str, episode_id: str, count: int) -> None:
    """Write a tiny recorder-compatible Parquet log for the replayer to read."""
    subjects = EpisodeSubjects(app=app, episode_id=episode_id)
    now = datetime.now(UTC)
    records = [
        make_record(
            stream_seq=i + 1,
            subject=subjects.public,
            received_at=now,  # equal timestamps -> back-to-back replay (no waits)
            data=Envelope(
                episode_id=episode_id,
                sender="alpha",
                payload={"type": "number", "value": i},
            ).to_bytes(),
            fallback_episode_id=episode_id,
        )
        for i in range(count)
    ]
    write_parquet(records, path)
