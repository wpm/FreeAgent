"""Integration tests: real orchestration, real child processes, real NATS.

Skipped when NATS is unreachable. ``run_episode`` spawns each agent and the
environment as ``python -m freeagent.cli.child`` subprocesses; those children
import the roster classes by their ``module:QualName`` reference. ``noop_app``
lives in this directory, so the test exports it on ``PYTHONPATH`` for the
children to import. Episode ids are auto-generated per run.
"""

from __future__ import annotations

import asyncio
import os
import socket
from pathlib import Path

import pytest
from noop_app import CrashingAgent, NoopAgent, NoopEnvironment

from freeagent import (
    EpisodeConfig,
    EpisodePlan,
    EpisodeStatus,
    make_plan,
    run_episode,
    start_episode,
)

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
# Long timeouts so the episode stays RUNNING for a test to abort: a generous
# setup window (the abort, not a slow join, must be what ends it) and a long
# episode timeout (it must not end on its own before the test aborts it).
LONG_ENV = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.2}


def _noop_plan(env_config: dict[str, object]) -> EpisodePlan:
    """A launchable plan for a two-NoopAgent episode under ``noopapp``."""
    config = EpisodeConfig.model_validate({"environment": {"config": env_config}})
    return make_plan(config, app="noopapp", roster=["alpha", "beta"])


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


@slow
@requires_nats
async def test_handle_starts_then_awaits_ended() -> None:
    """start_episode returns a live handle; awaiting it reaches ENDED (exit 0)."""
    agents = {"alpha": NoopAgent, "beta": NoopAgent}
    handle = await start_episode(_noop_plan(FAST_ENV), NoopEnvironment, agents)
    assert handle.app == "noopapp"
    assert handle.state is EpisodeStatus.RUNNING
    assert len(handle.processes) == 3  # two agents + the environment, no recorder

    outcome = await handle.wait()

    assert outcome.status is EpisodeStatus.ENDED
    assert outcome.exit_code == 0
    assert handle.state is EpisodeStatus.ENDED
    assert handle.outcome is outcome
    # Every child process has been cleaned up.
    assert all(proc.returncode is not None for proc in handle.processes)


@slow
@requires_nats
async def test_handle_request_abort_interrupts_a_running_episode() -> None:
    """request_abort() stops a running episode: INTERRUPTED, exit 1, all torn down."""
    agents = {"alpha": NoopAgent, "beta": NoopAgent}
    handle = await start_episode(_noop_plan(LONG_ENV), NoopEnvironment, agents)
    # Let the episode come up, then abort it well before its 30s timeout.
    await asyncio.sleep(2.0)
    assert handle.state is EpisodeStatus.RUNNING

    handle.request_abort()
    outcome = await handle.wait()

    assert outcome.status is EpisodeStatus.INTERRUPTED
    assert outcome.exit_code == 1
    assert handle.state is EpisodeStatus.INTERRUPTED
    assert all(proc.returncode is not None for proc in handle.processes)


@slow
@requires_nats
async def test_two_concurrent_episodes_do_not_interfere() -> None:
    """Two handles run in one loop, on distinct episodes, both reaching ENDED."""
    agents = {"alpha": NoopAgent, "beta": NoopAgent}
    first = await start_episode(_noop_plan(FAST_ENV), NoopEnvironment, agents)
    second = await start_episode(_noop_plan(FAST_ENV), NoopEnvironment, agents)

    # Distinct episodes that never share a process set.
    assert first.episode_id != second.episode_id
    assert first.processes.isdisjoint(second.processes)

    first_outcome, second_outcome = await asyncio.gather(first.wait(), second.wait())

    assert first_outcome.status is EpisodeStatus.ENDED
    assert second_outcome.status is EpisodeStatus.ENDED
    assert first.state is EpisodeStatus.ENDED
    assert second.state is EpisodeStatus.ENDED
