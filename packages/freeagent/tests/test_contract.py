"""The episode contract (ADR-0003 / #39): mock server, feed, OpenAPI, schemas.

These are fast tests -- no NATS, no child processes. They prove the contract is
real and usable: the mock implements the full episode REST surface and the
per-episode feed over WebSocket with canned data, the FastAPI app advertises the
whole surface via OpenAPI, and the Pydantic contract models export to JSON
Schema. A UI can develop entirely against this.
"""

from __future__ import annotations

import json

from fastapi.testclient import TestClient

from freeagent.service import create_mock_app
from freeagent.service.models import (
    CreateEpisodeRequest,
    EpisodeMessage,
    EpisodeView,
    FeedConnectionEvent,
    FeedMessageEvent,
    FeedStatusEvent,
)
from freeagent.service.schema import CONTRACT_MODELS
from freeagent.service.schema import main as export_main

APPLICATION = "twenty-questions"


def _client() -> TestClient:
    return TestClient(create_mock_app())


def test_list_returns_seeded_episodes() -> None:
    with _client() as client:
        response = client.get("/freeagent/episodes")
    assert response.status_code == 200
    episodes = [EpisodeView.model_validate(item) for item in response.json()]
    names = {episode.id for episode in episodes}
    assert {"elephant", "a1b2c3d4"} <= names
    sealed = next(episode for episode in episodes if episode.id == "elephant")
    assert sealed.sealed is True
    assert sealed.outcome == "won"
    assert sealed.name  # always present (auto-generated fallback at worst)


def test_create_then_get_roundtrips() -> None:
    with _client() as client:
        body = CreateEpisodeRequest(name="my game", model="some-model").model_dump()
        created = client.post(f"/freeagent/{APPLICATION}/episodes", json=body)
        assert created.status_code == 201
        view = EpisodeView.model_validate(created.json())
        assert view.name == "my game"
        assert view.sealed is False

        fetched = client.get(f"/freeagent/{APPLICATION}/episodes/{view.id}")
        assert fetched.status_code == 200
        assert EpisodeView.model_validate(fetched.json()).id == view.id


def test_rename_and_delete() -> None:
    with _client() as client:
        renamed = client.patch(
            f"/freeagent/{APPLICATION}/episodes/elephant", json={"name": "pachyderm"}
        )
        assert renamed.status_code == 200
        assert EpisodeView.model_validate(renamed.json()).name == "pachyderm"

        deleted = client.request("DELETE", f"/freeagent/{APPLICATION}/episodes/elephant")
        assert deleted.status_code == 204
        gone = client.get(f"/freeagent/{APPLICATION}/episodes/elephant")
        assert gone.status_code == 404


def test_import_and_export() -> None:
    with _client() as client:
        imported = client.post(
            "/freeagent/import", json={"parquet_path": "/data/game.parquet", "name": "from-disk"}
        )
        assert imported.status_code == 201
        view = EpisodeView.model_validate(imported.json())
        assert view.name == "from-disk"
        assert view.sealed is True

        exported = client.post(
            f"/freeagent/{APPLICATION}/episodes/elephant/export",
            json={"parquet_path": "/data/out.parquet"},
        )
        assert exported.status_code == 200
        assert exported.json()["rows"] > 0


def test_feed_streams_status_history_then_live() -> None:
    """Open a feed and confirm the ADR-0003 shape: a status snapshot, an open
    connection event, message history, then the live boundary."""
    with (
        _client() as client,
        client.websocket_connect(f"/freeagent/{APPLICATION}/episodes/elephant/feed") as ws,
    ):
        first = json.loads(ws.receive_text())
        status_event = FeedStatusEvent.model_validate(first)
        assert status_event.type == "status"
        assert status_event.name

        opened = FeedConnectionEvent.model_validate(json.loads(ws.receive_text()))
        assert opened.phase == "open"

        messages: list[EpisodeMessage] = []
        saw_live = False
        while True:
            event = json.loads(ws.receive_text())
            if event.get("type") == "message":
                messages.append(FeedMessageEvent.model_validate(event).message)
            elif event.get("type") == "connection":
                phase = FeedConnectionEvent.model_validate(event).phase
                if phase == "live":
                    saw_live = True
                    break
        assert saw_live
        # The canned transcript: an observer reads it as ordered, channelled rows.
        assert [m.sender for m in messages][:3] == ["env", "host", "alice"]
        assert any(m.channel == "public" for m in messages)
        assert [m.stream_seq for m in messages] == sorted(m.stream_seq for m in messages)


def test_openapi_advertises_the_episode_surface() -> None:
    with _client() as client:
        spec = client.get("/openapi.json").json()
    paths = spec["paths"]
    assert "/freeagent/episodes" in paths
    assert "/freeagent/{application}/episodes" in paths
    assert "/freeagent/{application}/episodes/{episode_id}" in paths
    assert "/freeagent/{application}/episodes/{episode_id}/stop" in paths
    assert "/freeagent/{application}/episodes/{episode_id}/export" in paths
    assert "/freeagent/import" in paths


def test_schema_export_writes_every_contract_model(tmp_path) -> None:  # noqa: ANN001 -- pytest tmp_path fixture
    export_main([str(tmp_path)])
    for stem in CONTRACT_MODELS:
        artifact = tmp_path / f"{stem}.schema.json"
        assert artifact.exists()
        schema = json.loads(artifact.read_text())
        assert schema["type"] == "object"
    # The feed message event nests EpisodeMessage as a named $def for TS codegen.
    feed = json.loads((tmp_path / "feed_message_event.schema.json").read_text())
    assert "EpisodeMessage" in feed["$defs"]
