"""Integration tests: real orchestration, real child processes, real NATS.

Skipped when NATS is unreachable. ``run_episode`` spawns each agent and the
environment as ``python -m freeagent.cli.child`` subprocesses; those children
import the roster classes by their ``module:QualName`` reference. ``noop_app``
lives in this directory, so the test exports it on ``PYTHONPATH`` for the
children to import. Episode ids are auto-generated per run.
"""

from __future__ import annotations

import os
import socket
from pathlib import Path

import pytest
from noop_app import CrashingAgent, NoopAgent, NoopEnvironment

from freeagent import EpisodeConfig, run_episode

TESTS_DIR = Path(__file__).parent
NATS_HOST = "localhost"
NATS_PORT = 4222


def _nats_running() -> bool:
    try:
        with socket.create_connection((NATS_HOST, NATS_PORT), timeout=0.5):
            return True
    except OSError:
        return False


requires_nats = pytest.mark.skipif(
    not _nats_running(),
    reason=(
        "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
    ),
)

_SLOW_TEST_VAR = "FREEAGENT_SLOW_TEST"
slow = pytest.mark.skipif(
    not os.environ.get(_SLOW_TEST_VAR),
    reason=f"slow test is opt-in: set {_SLOW_TEST_VAR} to run it",
)

FAST_ENV = {"setup_timeout": 1.0, "episode_timeout": 1.0, "grace_period": 0.2}
ABORT_ENV = {"setup_timeout": 0.5, "episode_timeout": 5.0, "grace_period": 0.2}


@pytest.fixture(autouse=True)
def _children_can_import_noop_app(monkeypatch: pytest.MonkeyPatch) -> None:
    """Export this directory so spawned children can import ``noop_app``."""
    existing = os.environ.get("PYTHONPATH", "")
    joined = str(TESTS_DIR) if not existing else f"{TESTS_DIR}{os.pathsep}{existing}"
    monkeypatch.setenv("PYTHONPATH", joined)


@slow
@requires_nats
def test_episode_runs_to_ended(capsys: pytest.CaptureFixture[str]) -> None:
    config = EpisodeConfig.model_validate({"environment": {"config": FAST_ENV}})
    code = run_episode(
        config,
        app="noopapp",
        environment=NoopEnvironment,
        agents={"alpha": NoopAgent, "beta": NoopAgent},
    )
    assert code == 0
    out = capsys.readouterr().out
    assert "app=noopapp" in out
    assert "state=ended" in out


@slow
@requires_nats
def test_episode_aborts_when_an_agent_never_joins(capsys: pytest.CaptureFixture[str]) -> None:
    config = EpisodeConfig.model_validate({"environment": {"config": ABORT_ENV}})
    code = run_episode(
        config,
        app="noopapp",
        environment=NoopEnvironment,
        agents={"alpha": NoopAgent, "saboteur": CrashingAgent},
    )
    assert code == 2
    out = capsys.readouterr().out
    assert "state=aborted" in out
