"""Tests for the Free Agent API."""

import os
import sys

from fastapi.testclient import TestClient
from freeagent.api.app import create_app


def test_health() -> None:
    client = TestClient(create_app())
    response = client.get("/health")
    assert response.status_code == 200
    body = response.json()
    # Liveness plus the serving environment's identity (issue #117): the interpreter/venv and pid
    # a launcher checks before adopting the listener as its own API.
    assert body["status"] == "ok"
    assert body["executable"] == sys.executable
    assert body["pid"] == os.getpid()
