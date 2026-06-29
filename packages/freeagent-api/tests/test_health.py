"""Tests for the Free Agent API."""

from fastapi.testclient import TestClient
from freeagent.api.app import create_app


def test_health() -> None:
    client = TestClient(create_app())
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}
