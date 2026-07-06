"""Unit tests for the REST surface in :mod:`freeagent.api.app`.

The routes are driven through FastAPI's ``TestClient`` against an
:class:`~freeagent.api.episodes.EpisodeManager` wired to the fakes in ``api_fakes.py``, so
every HTTP
status and response shape is pinned without a NATS server or worker subprocesses. The full
over-the-wire behavior lives in ``test_integration.py``.
"""

from __future__ import annotations

import os
from unittest.mock import patch

from api_fakes import FakeNatsClient, FakeProcess
from fastapi import FastAPI
from fastapi.testclient import TestClient
from freeagent.api.app import NATS_URL_ENV, create_app
from freeagent.api.episodes import EpisodeManager, NatsClient, WorkerProcess
from freeagent.sdk.entity import DEFAULT_NATS_SERVER
from freeagent.sdk.message import EpisodeComplete, StartEntity


class AppHarness:
    """A FastAPI app over a fake-wired manager, with the fakes kept inspectable."""

    def __init__(self) -> None:
        self.client = FakeNatsClient()
        self.processes: list[FakeProcess] = []

        async def connect(url: str) -> NatsClient:
            return self.client

        def spawn(command: list[str]) -> WorkerProcess:
            process = FakeProcess()
            self.processes.append(process)
            return process

        self.manager = EpisodeManager("nats://fake:4222", connect=connect, spawn=spawn)
        self.app: FastAPI = create_app(manager=self.manager)
        self.http = TestClient(self.app)


def test_health() -> None:
    response = AppHarness().http.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


def test_applications_lists_installed_applications() -> None:
    response = AppHarness().http.get("/applications")
    assert response.status_code == 200
    applications = response.json()["applications"]
    assert "collatz" in applications
    assert applications == sorted(applications)


def test_create_episode_returns_created_status() -> None:
    harness = AppHarness()
    response = harness.http.post(
        "/applications/collatz/episodes",
        json={"episode_id": "ep1", "config": {"starts": [8]}},
    )
    assert response.status_code == 201
    body = response.json()
    assert body == {
        "application": "collatz",
        "episode_id": "ep1",
        "episode_root": "episode.collatz.ep1",
        "state": "created",
        "agents_alive": [],
        "message_count": 0,
        "worker_exit_code": None,
    }
    assert len(harness.processes) == 1


def test_create_episode_generates_an_id_when_none_is_given() -> None:
    harness = AppHarness()
    response = harness.http.post("/applications/collatz/episodes", json={})
    assert response.status_code == 201
    assert response.json()["episode_id"]


def test_create_episode_404s_for_an_unknown_application() -> None:
    response = AppHarness().http.post("/applications/no-such-app/episodes", json={})
    assert response.status_code == 404


def test_create_episode_409s_for_a_duplicate() -> None:
    harness = AppHarness()
    assert (
        harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"}).status_code
        == 201
    )
    assert (
        harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"}).status_code
        == 409
    )


def test_create_episode_422s_for_a_malformed_episode_id() -> None:
    response = AppHarness().http.post(
        "/applications/collatz/episodes", json={"episode_id": "not.a.token"}
    )
    assert response.status_code == 422


def test_status_reflects_recorded_control_plane_traffic() -> None:
    harness = AppHarness()
    harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"})
    monitor = harness.manager.get("collatz", "ep1").monitor
    monitor.record("episode.collatz.ep1.agents.agent-0", StartEntity().to_bytes(), 1.0)

    body = harness.http.get("/applications/collatz/episodes/ep1").json()
    assert body["state"] == "running"
    assert body["agents_alive"] == ["agent-0"]

    monitor.record("episode.collatz.ep1.environment", EpisodeComplete().to_bytes(), 2.0)
    assert harness.http.get("/applications/collatz/episodes/ep1").json()["state"] == "complete"


def test_status_404s_for_an_unknown_episode() -> None:
    assert AppHarness().http.get("/applications/collatz/episodes/nope").status_code == 404


def test_list_episodes_serves_each_episodes_status() -> None:
    harness = AppHarness()
    harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"})
    harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep2"})
    body = harness.http.get("/applications/collatz/episodes").json()
    assert [episode["episode_id"] for episode in body["episodes"]] == ["ep1", "ep2"]
    assert all(episode["state"] == "created" for episode in body["episodes"])


def test_list_episodes_is_empty_before_any_are_created() -> None:
    assert AppHarness().http.get("/applications/collatz/episodes").json() == {"episodes": []}


def test_list_episodes_404s_for_an_unknown_application() -> None:
    assert AppHarness().http.get("/applications/no-such-app/episodes").status_code == 404


def test_cross_origin_viewer_requests_are_allowed() -> None:
    # Viewers are static pages served from their own origin; the browser only lets
    # them call the API if it answers with permissive CORS headers.
    response = AppHarness().http.get("/applications", headers={"Origin": "http://localhost:5173"})
    assert response.status_code == 200
    assert response.headers["access-control-allow-origin"] == "*"


def test_messages_serves_the_data_plane_feed_verbatim() -> None:
    harness = AppHarness()
    harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"})
    monitor = harness.manager.get("collatz", "ep1").monitor
    monitor.record(
        "episode.collatz.ep1.agents.agent-0",
        b'{"message_type": "Widget", "numbers": [8, 4]}',
        1.5,
    )

    body = harness.http.get("/applications/collatz/episodes/ep1/messages").json()
    assert body == {
        "messages": [
            {
                "seq": 0,
                "subject": "episode.collatz.ep1.agents.agent-0",
                "message_type": "Widget",
                "received_at": 1.5,
                "payload": {"message_type": "Widget", "numbers": [8, 4]},
            }
        ]
    }


def test_messages_404s_for_an_unknown_episode() -> None:
    assert AppHarness().http.get("/applications/collatz/episodes/nope/messages").status_code == 404


def test_delete_stops_the_episode() -> None:
    harness = AppHarness()
    harness.http.post("/applications/collatz/episodes", json={"episode_id": "ep1"})
    response = harness.http.delete("/applications/collatz/episodes/ep1")
    assert response.status_code == 204
    assert harness.processes[0].terminate_calls == 1
    assert harness.http.get("/applications/collatz/episodes/ep1").json()["state"] == "stopped"
    # Stopping again is a no-op, not an error.
    assert harness.http.delete("/applications/collatz/episodes/ep1").status_code == 204


def test_delete_404s_for_an_unknown_episode() -> None:
    assert AppHarness().http.delete("/applications/collatz/episodes/nope").status_code == 404


def test_lifespan_shutdown_tears_the_manager_down() -> None:
    harness = AppHarness()
    with TestClient(harness.app) as http:
        http.post("/applications/collatz/episodes", json={"episode_id": "ep1"})
    assert harness.processes[0].poll() is not None
    assert harness.client.close_calls == 1


def test_create_app_reads_the_nats_url_from_the_environment() -> None:
    with patch.dict(os.environ, {NATS_URL_ENV: "nats://elsewhere:4222"}):
        app = create_app()
    manager: EpisodeManager = app.state.manager
    assert manager.nats_url == "nats://elsewhere:4222"


def test_create_app_falls_back_to_the_sdk_default_nats_url() -> None:
    with patch.dict(os.environ, clear=False):
        os.environ.pop(NATS_URL_ENV, None)
        app = create_app()
    manager: EpisodeManager = app.state.manager
    assert manager.nats_url == DEFAULT_NATS_SERVER


def test_create_app_prefers_an_explicit_nats_url() -> None:
    with patch.dict(os.environ, {NATS_URL_ENV: "nats://elsewhere:4222"}):
        app = create_app(nats_url="nats://explicit:4222")
    manager: EpisodeManager = app.state.manager
    assert manager.nats_url == "nats://explicit:4222"
