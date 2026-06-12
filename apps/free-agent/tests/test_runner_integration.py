"""Integration tests: the real CLI, real child processes, real NATS.

Skipped when NATS is unreachable. The episode configurations live next to
``noop_app.py`` in this directory on purpose: the runner must make app
modules beside the yml importable in the parent and in every child (via
PYTHONPATH). Episode ids are omitted from the ymls, so the runner generates
a unique uuid4 hex id per run.
"""

from __future__ import annotations

import asyncio
import socket
import sys
from pathlib import Path

import pytest

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


async def _run_cli(config: Path) -> tuple[int, str, str]:
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "freeagent_runner.cli",
        "run",
        str(config),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=60)
    assert process.returncode is not None
    return process.returncode, stdout.decode(), stderr.decode()


@requires_nats
async def test_episode_runs_to_ended() -> None:
    code, stdout, stderr = await _run_cli(TESTS_DIR / "noop_episode.yml")
    assert code == 0, f"stdout: {stdout!r}\nstderr: {stderr!r}"
    assert "app=noopapp" in stdout
    assert "state=ended" in stdout


@requires_nats
async def test_episode_aborts_when_an_agent_never_joins() -> None:
    code, stdout, stderr = await _run_cli(TESTS_DIR / "noop_abort.yml")
    assert code == 2, f"stdout: {stdout!r}\nstderr: {stderr!r}"
    assert "app=noopapp" in stdout
    assert "state=aborted" in stdout
    # The sabotaged agent's failure is streamed through on stderr.
    assert "sabotaged" in stderr
