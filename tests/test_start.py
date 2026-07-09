"""Unit tests for :mod:`freeagent.start`, the platform on/off switch.

The switch owns no orchestration logic of its own — it delegates to :mod:`freeagent.sdk.launch` and
to ``docker compose down`` — so every test here mocks those seams. Nothing spawns docker, NATS, or
the API: the tests assert that ``start`` and ``stop`` call the launch code with the right arguments,
report the right outcomes, and propagate exit codes.
"""

from __future__ import annotations

from typing import Any

import pytest
from freeagent.sdk import launch

from freeagent import start


class FakeCompletedProcess:
    """A stand-in for :class:`subprocess.CompletedProcess` carrying just a return code."""

    def __init__(self, returncode: int) -> None:
        self.returncode = returncode


# --------------------------------------------------------------------------------------------------
# start
# --------------------------------------------------------------------------------------------------


def test_start_ensures_nats_against_the_in_repo_compose_file(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    compose_files: list[Any] = []

    def fake_ensure_nats(compose_file: Any = None) -> launch.Outcome:
        compose_files.append(compose_file)
        return launch.Outcome.STARTED

    monkeypatch.setattr(launch, "ensure_nats", fake_ensure_nats)
    monkeypatch.setattr(launch, "ensure_api", lambda: launch.Outcome.STARTED)

    start.start()

    # The in-repo compose file is the source of truth, passed explicitly (never the packaged copy).
    assert compose_files == [start.COMPOSE_FILE]
    output = capsys.readouterr().out
    assert "NATS network started." in output
    assert "freeagent-api started." in output


def test_start_reports_everything_already_running(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(
        launch, "ensure_nats", lambda compose_file=None: launch.Outcome.ALREADY_RUNNING
    )
    monkeypatch.setattr(launch, "ensure_api", lambda: launch.Outcome.ALREADY_RUNNING)

    start.start()

    output = capsys.readouterr().out
    assert "NATS network already running." in output
    assert "freeagent-api already running." in output


def test_start_reports_mixed_outcomes(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # NATS already up but the API had to be started: each line reflects its own outcome.
    monkeypatch.setattr(
        launch, "ensure_nats", lambda compose_file=None: launch.Outcome.ALREADY_RUNNING
    )
    monkeypatch.setattr(launch, "ensure_api", lambda: launch.Outcome.STARTED)

    start.start()

    output = capsys.readouterr().out
    assert "NATS network already running." in output
    assert "freeagent-api started." in output


def test_start_exits_with_guidance_when_docker_unavailable(monkeypatch: pytest.MonkeyPatch) -> None:
    def raise_docker_unavailable(compose_file: Any = None) -> launch.Outcome:
        raise launch.DockerUnavailableError("docker is required but was not found on PATH")

    monkeypatch.setattr(launch, "ensure_nats", raise_docker_unavailable)

    def fail() -> launch.Outcome:
        raise AssertionError("the API must not be ensured when NATS could not start")

    monkeypatch.setattr(launch, "ensure_api", fail)

    with pytest.raises(SystemExit) as excinfo:
        start.start()
    assert "docker is required" in str(excinfo.value)


# --------------------------------------------------------------------------------------------------
# stop
# --------------------------------------------------------------------------------------------------


@pytest.fixture
def docker_on_path(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make ``stop`` see docker as installed, so the docker-path tests hold on a docker-less runner.

    ``stop`` guards ``docker compose down`` with ``shutil.which("docker")``; CI's test runner has no
    docker, so without this the guard would fire and skip the mocked ``subprocess.run``. The
    dedicated missing-docker test deliberately does not request this fixture.
    """
    monkeypatch.setattr("freeagent.start.shutil.which", lambda _: "/usr/bin/docker")


def test_stop_stops_the_api_then_takes_nats_down(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], docker_on_path: None
) -> None:
    monkeypatch.setattr(launch, "stop_api", lambda: True)

    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> FakeCompletedProcess:
        calls.append(cmd)
        return FakeCompletedProcess(0)

    monkeypatch.setattr("freeagent.start.subprocess.run", fake_run)

    with pytest.raises(SystemExit) as excinfo:
        start.stop()
    assert excinfo.value.code == 0

    # docker compose down runs against the in-repo compose file.
    assert calls == [["docker", "compose", "--file", str(start.COMPOSE_FILE), "down"]]
    output = capsys.readouterr().out
    assert "freeagent-api stopped." in output
    assert "NATS network is down." in output


def test_stop_succeeds_when_api_was_not_running(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], docker_on_path: None
) -> None:
    # stop_api returns False when there was nothing to stop (already down or half up): stop still
    # takes the network down and succeeds.
    monkeypatch.setattr(launch, "stop_api", lambda: False)
    monkeypatch.setattr("freeagent.start.subprocess.run", lambda cmd, **_: FakeCompletedProcess(0))

    with pytest.raises(SystemExit) as excinfo:
        start.stop()
    assert excinfo.value.code == 0

    output = capsys.readouterr().out
    assert "freeagent-api was not running." in output
    assert "NATS network is down." in output


def test_stop_always_takes_nats_down_even_after_stopping_the_api(
    monkeypatch: pytest.MonkeyPatch, docker_on_path: None
) -> None:
    # Whether or not the API was running, docker compose down must run: stop leaves the platform
    # fully down from any starting state.
    monkeypatch.setattr(launch, "stop_api", lambda: True)
    ran: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> FakeCompletedProcess:
        ran.append(cmd)
        return FakeCompletedProcess(0)

    monkeypatch.setattr("freeagent.start.subprocess.run", fake_run)

    with pytest.raises(SystemExit):
        start.stop()
    assert ran and ran[0][-1] == "down"


def test_stop_propagates_docker_return_code(
    monkeypatch: pytest.MonkeyPatch, docker_on_path: None
) -> None:
    monkeypatch.setattr(launch, "stop_api", lambda: True)
    monkeypatch.setattr("freeagent.start.subprocess.run", lambda cmd, **_: FakeCompletedProcess(1))

    with pytest.raises(SystemExit) as excinfo:
        start.stop()
    assert excinfo.value.code == 1


def test_stop_exits_cleanly_when_docker_missing(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # With docker absent, stop must still stop the API, then exit with a guidance message rather
    # than letting subprocess.run raise a raw FileNotFoundError traceback.
    monkeypatch.setattr(launch, "stop_api", lambda: True)
    monkeypatch.setattr("freeagent.start.shutil.which", lambda _: None)

    def fail(*_: Any, **__: Any) -> FakeCompletedProcess:
        raise AssertionError("docker compose must not run when docker is not on PATH")

    monkeypatch.setattr("freeagent.start.subprocess.run", fail)

    with pytest.raises(SystemExit) as excinfo:
        start.stop()
    assert "docker is required" in str(excinfo.value)
    # The API teardown still happened before the docker guard tripped.
    assert "freeagent-api stopped." in capsys.readouterr().out
