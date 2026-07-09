"""Unit tests for :mod:`freeagent.sdk.launch`.

Every test mocks docker, subprocess spawning, health polling, and signalling, so the suite needs
neither a Docker daemon nor a live API. The behaviours under test are the ones the issue calls out:
idempotent ensure, the clear docker-unavailable error, the missing-``freeagent-api`` message, and
stale-pid handling.
"""

from __future__ import annotations

import signal
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from freeagent.sdk import launch


@pytest.fixture
def state_dir_in_tmp(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """Pin the pid-file location under a temp directory so tests never touch a real repo.

    Patches :func:`freeagent.sdk.launch._state_dir`. Not autouse: the dedicated ``_state_dir`` tests
    exercise the real resolution, so they must not request this fixture.
    """
    monkeypatch.setattr(launch, "_state_dir", lambda: tmp_path)
    yield tmp_path


class FakeResponse:
    """A minimal stand-in for the object ``urllib.request.urlopen`` yields as a context manager."""

    def __init__(self, status: int) -> None:
        self.status = status

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_: object) -> None:
        return None


def _patch_health(monkeypatch: pytest.MonkeyPatch, healthy_urls: set[str]) -> list[str]:
    """Patch ``urlopen`` so URLs in ``healthy_urls`` return 200 and all others raise.

    :return: A list that records every URL probed, in order.
    """
    probed: list[str] = []

    def fake_urlopen(url: str, timeout: float = 0) -> FakeResponse:
        probed.append(url)
        if url in healthy_urls:
            return FakeResponse(200)
        raise OSError("connection refused")

    monkeypatch.setattr("freeagent.sdk.launch.urllib.request.urlopen", fake_urlopen)
    return probed


# --------------------------------------------------------------------------------------------------
# _is_healthy
# --------------------------------------------------------------------------------------------------


def test_is_healthy_true_on_2xx(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, {launch.NATS_HEALTH_URL})
    assert launch._is_healthy(launch.NATS_HEALTH_URL) is True


def test_is_healthy_false_on_non_2xx(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "freeagent.sdk.launch.urllib.request.urlopen", lambda url, timeout=0: FakeResponse(503)
    )
    assert launch._is_healthy(launch.api_health_url()) is False


def test_is_healthy_false_on_connection_error(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    assert launch._is_healthy(launch.api_health_url()) is False


# --------------------------------------------------------------------------------------------------
# resolve_compose_file
# --------------------------------------------------------------------------------------------------


def test_resolve_compose_file_uses_explicit_path() -> None:
    explicit = Path("/somewhere/docker/compose.yaml")
    assert launch.resolve_compose_file(explicit) == explicit


def test_resolve_compose_file_falls_back_to_packaged_copy() -> None:
    resolved = launch.resolve_compose_file(None)
    assert resolved.name == "compose.yaml"
    assert resolved.is_file()
    # The packaged config sits beside the compose file so the relative volume mount resolves.
    assert (resolved.parent / "nats-server.conf").is_file()


# --------------------------------------------------------------------------------------------------
# ensure_nats
# --------------------------------------------------------------------------------------------------


def test_ensure_nats_already_running_is_noop(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, {launch.NATS_HEALTH_URL})

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("docker compose must not run when NATS is already healthy")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fail)
    assert launch.ensure_nats() is launch.Outcome.ALREADY_RUNNING


def test_ensure_nats_starts_when_down(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/usr/bin/docker")

    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> Any:
        calls.append(cmd)
        return type("R", (), {"returncode": 0, "stderr": ""})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)

    assert launch.ensure_nats() is launch.Outcome.STARTED
    assert calls == [
        [
            "docker",
            "compose",
            "--file",
            str(launch.resolve_compose_file(None)),
            "up",
            "--detach",
            "--wait",
        ]
    ]


def test_ensure_nats_passes_explicit_compose_file(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/usr/bin/docker")

    captured: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> Any:
        captured.append(cmd)
        return type("R", (), {"returncode": 0, "stderr": ""})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)

    explicit = Path("/repo/docker/compose.yaml")
    launch.ensure_nats(explicit)
    assert str(explicit) in captured[0]


def test_ensure_nats_raises_when_docker_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: None)

    with pytest.raises(launch.DockerUnavailableError, match="docker is required"):
        launch.ensure_nats()


def test_ensure_nats_raises_when_compose_up_fails(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/usr/bin/docker")
    monkeypatch.setattr(
        "freeagent.sdk.launch.subprocess.run",
        lambda cmd, **_: type("R", (), {"returncode": 1, "stderr": "daemon down"})(),
    )

    with pytest.raises(launch.DockerUnavailableError, match="daemon down"):
        launch.ensure_nats()


# --------------------------------------------------------------------------------------------------
# ensure_api
# --------------------------------------------------------------------------------------------------


class FakePopen:
    """A stand-in for :class:`subprocess.Popen` recording its args and reporting a fixed pid."""

    instances: list[FakePopen] = []

    def __init__(self, cmd: list[str], **kwargs: Any) -> None:
        self.cmd = cmd
        self.kwargs = kwargs
        self.pid = 424242
        FakePopen.instances.append(self)


@pytest.fixture(autouse=True)
def reset_fake_popen() -> Iterator[None]:
    FakePopen.instances = []
    yield


def test_ensure_api_already_running_is_noop(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, {launch.api_health_url()})

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("the API must not be spawned when already healthy")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)
    assert launch.ensure_api() is launch.Outcome.ALREADY_RUNNING


def test_ensure_api_spawns_detached_and_writes_pid(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # Down on the first probe, healthy afterwards (the wait loop then succeeds).
    probes = iter([False, True, True])
    monkeypatch.setattr(launch, "_is_healthy", lambda _: next(probes))
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/venv/bin/freeagent-api")
    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", FakePopen)

    assert launch.ensure_api() is launch.Outcome.STARTED

    spawned = FakePopen.instances[0]
    assert spawned.cmd == [launch.API_EXECUTABLE]
    # Detached into its own session so a session's Ctrl-C never kills the shared API.
    assert spawned.kwargs["start_new_session"] is True

    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    assert pid_file.read_text() == "424242"


def test_api_health_url_defaults(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(launch.API_HOST_ENV, raising=False)
    monkeypatch.delenv(launch.API_PORT_ENV, raising=False)
    assert launch.api_health_url() == "http://127.0.0.1:8000/health"


def test_api_health_url_honors_the_api_bind_env_vars(monkeypatch: pytest.MonkeyPatch) -> None:
    # The API binds FREEAGENT_API_HOST/FREEAGENT_API_PORT; the health probe must follow it there,
    # not poll the default port while the API comes up elsewhere.
    monkeypatch.setenv(launch.API_HOST_ENV, "0.0.0.0")
    monkeypatch.setenv(launch.API_PORT_ENV, "9000")
    assert launch.api_health_url() == "http://0.0.0.0:9000/health"


def test_ensure_api_redirects_stdio_away_from_the_terminal(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # The detached API must not inherit the spawning terminal's stdio: its output would interleave
    # into that shell, and the terminal closing would make the daemon's writes start failing.
    probes = iter([False, True, True])
    monkeypatch.setattr(launch, "_is_healthy", lambda _: next(probes))
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/venv/bin/freeagent-api")
    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", FakePopen)

    launch.ensure_api()

    spawned = FakePopen.instances[0]
    assert spawned.kwargs["stdin"] is subprocess.DEVNULL
    log_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_LOG_FILE_NAME
    # stdout and stderr both append to the log file beside the pid file.
    assert spawned.kwargs["stdout"].name == str(log_file)
    assert spawned.kwargs["stderr"] is spawned.kwargs["stdout"]


def test_ensure_api_missing_executable_exits_with_guidance(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: None)

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("must not spawn when the executable is missing")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)

    with pytest.raises(SystemExit) as excinfo:
        launch.ensure_api()
    message = str(excinfo.value)
    assert "freeagent-api" in message
    assert "dev dependencies" in message


def test_ensure_api_overwrites_stale_pid_file(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # A pid file left over from a previous, now-dead API.
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("999999")

    probes = iter([False, True])
    monkeypatch.setattr(launch, "_is_healthy", lambda _: next(probes))
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/venv/bin/freeagent-api")
    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", FakePopen)

    launch.ensure_api()
    # Health is the source of truth: the stale pid is replaced with the freshly spawned one.
    assert pid_file.read_text() == "424242"


def test_ensure_api_exits_when_spawn_never_healthy(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/venv/bin/freeagent-api")
    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", FakePopen)
    # Make the wait return immediately without ever seeing health.
    monkeypatch.setattr(launch, "_wait_until_healthy", lambda url, timeout=0: False)

    with pytest.raises(SystemExit, match="did not become healthy"):
        launch.ensure_api()


# --------------------------------------------------------------------------------------------------
# stop_api
# --------------------------------------------------------------------------------------------------


def test_stop_api_noop_when_no_pid_file(state_dir_in_tmp: Path) -> None:
    assert launch.stop_api() is False


def test_stop_api_clears_stale_pid_file(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("999999")
    # The recorded process is dead.
    monkeypatch.setattr(launch, "_process_alive", lambda _: False)

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("must not signal a dead process")

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", fail)

    assert launch.stop_api() is False
    assert not pid_file.exists()


def test_stop_api_signals_live_process_and_removes_file(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")

    # Alive for the initial check, dead once signalled (so the wait loop exits promptly).
    alive = iter([True, False])
    monkeypatch.setattr(launch, "_process_alive", lambda _: next(alive))

    signalled: list[tuple[int, int]] = []
    monkeypatch.setattr(
        "freeagent.sdk.launch.os.kill", lambda pid, sig: signalled.append((pid, sig))
    )

    assert launch.stop_api() is True
    assert signalled == [(12345, signal.SIGTERM)]
    assert not pid_file.exists()


def test_stop_api_ignores_malformed_pid_file(state_dir_in_tmp: Path) -> None:
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("not-a-number")
    assert launch.stop_api() is False
    assert not pid_file.exists()


# --------------------------------------------------------------------------------------------------
# state directory resolution and process liveness (the real helpers)
# --------------------------------------------------------------------------------------------------


def test_state_dir_is_repo_root_when_in_repo(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / ".git").mkdir(parents=True)
    subdir = repo / "packages" / "thing"
    subdir.mkdir(parents=True)
    monkeypatch.chdir(subdir)
    assert launch._state_dir() == repo


def test_state_dir_is_cwd_when_not_in_repo(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    outside = tmp_path / "loose"
    outside.mkdir()
    monkeypatch.chdir(outside)
    assert launch._state_dir() == outside


def test_process_alive_reports_dead_for_missing_pid(monkeypatch: pytest.MonkeyPatch) -> None:
    def raise_lookup(pid: int, sig: int) -> None:
        raise ProcessLookupError

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", raise_lookup)
    assert launch._process_alive(1) is False


def test_process_alive_reports_alive_for_permission_denied(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def raise_perm(pid: int, sig: int) -> None:
        raise PermissionError

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", raise_perm)
    assert launch._process_alive(1) is True


def test_process_alive_reports_alive_for_live_pid(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("freeagent.sdk.launch.os.kill", lambda pid, sig: None)
    assert launch._process_alive(1) is True


def test_read_pid_none_for_missing_file(tmp_path: Path) -> None:
    assert launch._read_pid(tmp_path / "absent.pid") is None


def test_wait_until_healthy_true_immediately(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    assert launch._wait_until_healthy(launch.api_health_url(), timeout=1) is True


def test_wait_until_healthy_times_out(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(launch, "_is_healthy", lambda _: False)
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda _: None)
    # A zero timeout means one failed probe, then the deadline check fails.
    assert launch._wait_until_healthy(launch.api_health_url(), timeout=0) is False


def test_wait_until_healthy_retries_then_succeeds(monkeypatch: pytest.MonkeyPatch) -> None:
    # Unhealthy once (sleep, retry), then healthy — exercises the sleep-and-retry branch.
    probes = iter([False, True])
    monkeypatch.setattr(launch, "_is_healthy", lambda _: next(probes))
    slept: list[float] = []
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda seconds: slept.append(seconds))
    assert launch._wait_until_healthy(launch.api_health_url(), timeout=5) is True
    assert slept == [launch._HEALTH_POLL_INTERVAL_SECONDS]


def test_stop_api_waits_for_process_to_exit(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")

    # Alive for the initial check and the first wait-loop check, then gone: the loop sleeps once.
    alive = iter([True, True, False])
    monkeypatch.setattr(launch, "_process_alive", lambda _: next(alive))
    monkeypatch.setattr("freeagent.sdk.launch.os.kill", lambda pid, sig: None)
    slept: list[float] = []
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda seconds: slept.append(seconds))

    assert launch.stop_api() is True
    assert slept == [launch._STOP_POLL_INTERVAL_SECONDS]
    assert not pid_file.exists()


# --------------------------------------------------------------------------------------------------
# Session half: Service, Launcher, run()
# --------------------------------------------------------------------------------------------------


class RecordingLauncher:
    """A :class:`launch.Launcher` returning a fixed service list, for driving :func:`launch.run`."""

    def __init__(self, name: str, services: list[launch.Service]) -> None:
        self.name = name
        self._services = services

    def services(self) -> list[launch.Service]:
        return self._services


class FakeServeProcess:
    """A stand-in for a long-running :class:`subprocess.Popen` returned by ``_spawn_serves``.

    Mimics real POSIX Popen semantics, which the earlier version got wrong: a process killed by a
    signal reports a *negative* returncode of ``-signum``. So a still-running serve that we
    ``terminate()`` reports ``-SIGTERM`` from ``wait()`` (not ``0``) unless it installed a handler —
    exactly the case that made clean teardown look like a failure. Configure ``exit_code`` to model
    a serve that had already exited on its own before teardown (a crash with a positive code, or the
    terminal's group SIGINT with ``-SIGINT``); leave it ``None`` for a live serve that teardown
    terminates.

    :param name: A label, for assertions on teardown order.
    :param exit_code: If given, the serve is already exited with this code (``poll`` reports it and
        ``terminate`` is never expected). If ``None``, the serve is live; ``terminate`` kills it and
        ``wait`` then reports ``-SIGTERM``.
    :param term_code: The code a *live* serve reports after ``terminate``; defaults to ``-SIGTERM``,
        the real default for a process with no SIGTERM handler. Override to model a serve that traps
        SIGTERM and exits 0, or ignores it and dies some other way.
    """

    def __init__(
        self, name: str, exit_code: int | None = None, term_code: int = -signal.SIGTERM
    ) -> None:
        self.name = name
        self._term_code = term_code
        self.terminated = False
        # A serve already exited on its own (crashed, or group-SIGINT'd) is reaped in place.
        self._exit_code = exit_code

    def poll(self) -> int | None:
        return self._exit_code

    def terminate(self) -> None:
        self.terminated = True
        self._exit_code = self._term_code

    def wait(self) -> int:
        # A real wait() blocks until exit; a live serve here is considered terminated by teardown.
        if self._exit_code is None:
            self._exit_code = self._term_code
        return self._exit_code


def _no_platform(monkeypatch: pytest.MonkeyPatch) -> None:
    """Stub out the platform ensure calls so ``run`` tests never touch docker or a live API.

    Records nothing beyond making the calls no-ops; the "platform survives" assertion is that no
    teardown of platform processes (``stop_api``, docker down) is ever invoked, checked per test.
    """
    monkeypatch.setattr(launch, "ensure_nats", lambda: launch.Outcome.ALREADY_RUNNING)
    monkeypatch.setattr(launch, "ensure_api", lambda: launch.Outcome.ALREADY_RUNNING)


def test_service_defaults() -> None:
    service = launch.Service(name="viewer", serve=["npm", "run", "serve"])
    assert service.prepare == []
    assert service.cwd is None
    assert service.url is None


def test_launcher_protocol_is_runtime_checkable() -> None:
    launcher = RecordingLauncher("collatz", [])
    assert isinstance(launcher, launch.Launcher)
    assert not isinstance(object(), launch.Launcher)


def test_run_prepare_failure_aborts_before_any_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    _no_platform(monkeypatch)

    prepared: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> Any:
        prepared.append(cmd)
        # The second prepare command fails.
        code = 0 if cmd == ["build", "one"] else 3
        return type("R", (), {"returncode": code})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)

    def fail_spawn(*_: Any, **__: Any) -> None:
        raise AssertionError("no serve may be spawned once a prepare step has failed")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail_spawn)

    launcher = RecordingLauncher(
        "collatz",
        [
            launch.Service(name="viewer", serve=["serve", "viewer"], prepare=[["build", "one"]]),
            launch.Service(name="worker", serve=["serve", "worker"], prepare=[["build", "two"]]),
        ],
    )

    assert launch.run(launcher) == 3
    # Both prepare commands ran, the second failed, and no serve was spawned (fail_spawn is armed).
    assert prepared == [["build", "one"], ["build", "two"]]


def test_run_prepare_steps_run_in_order_with_cwd(monkeypatch: pytest.MonkeyPatch) -> None:
    _no_platform(monkeypatch)

    calls: list[tuple[list[str], object]] = []

    def fake_run(cmd: list[str], cwd: object = None, **_: Any) -> Any:
        calls.append((cmd, cwd))
        return type("R", (), {"returncode": 0})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)
    # Spawn nothing real and interrupt immediately so the test doesn't block.
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: [])
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    launcher = RecordingLauncher(
        "collatz",
        [
            launch.Service(
                name="viewer",
                serve=["serve"],
                prepare=[["npm", "ci"], ["npm", "run", "build"]],
                cwd="/apps/collatz/viewer",
            )
        ],
    )

    assert launch.run(launcher) == 0
    assert calls == [
        (["npm", "ci"], "/apps/collatz/viewer"),
        (["npm", "run", "build"], "/apps/collatz/viewer"),
    ]


def _raise_keyboard_interrupt(*_: Any) -> None:
    raise KeyboardInterrupt


def test_run_sigint_terminates_serves_in_reverse_order(monkeypatch: pytest.MonkeyPatch) -> None:
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    processes = [FakeServeProcess("first"), FakeServeProcess("second"), FakeServeProcess("third")]
    spawned = [(launch.Service(name=proc.name, serve=[proc.name]), proc) for proc in processes]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)

    terminated_order: list[str] = []
    original_terminate = FakeServeProcess.terminate

    def record_terminate(self: FakeServeProcess) -> None:
        terminated_order.append(self.name)
        original_terminate(self)

    monkeypatch.setattr(FakeServeProcess, "terminate", record_terminate)
    # Ctrl-C the moment the session blocks.
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    assert launch.run(RecordingLauncher("collatz", [])) == 0
    assert terminated_order == ["third", "second", "first"]
    assert all(proc.terminated for proc in processes)


def test_run_exit_code_propagates_from_crashed_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    # The second serve had already crashed (positive exit 7) by teardown; the first is still live
    # and gets a clean -SIGTERM. The positive crash code must propagate through the clean teardown.
    healthy = FakeServeProcess("viewer")
    crashed = FakeServeProcess("worker", exit_code=7)
    spawned = [
        (launch.Service(name="viewer", serve=["viewer"]), healthy),
        (launch.Service(name="worker", serve=["worker"]), crashed),
    ]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    assert launch.run(RecordingLauncher("collatz", [])) == 7
    # The already-exited crash is reaped, not re-terminated; the live one is terminated.
    assert crashed.terminated is False
    assert healthy.terminated is True


def test_run_ensures_platform_but_never_tears_it_down(monkeypatch: pytest.MonkeyPatch) -> None:
    ensured: list[str] = []

    def ensure_nats() -> launch.Outcome:
        ensured.append("nats")
        return launch.Outcome.STARTED

    def ensure_api() -> launch.Outcome:
        ensured.append("api")
        return launch.Outcome.STARTED

    monkeypatch.setattr(launch, "ensure_nats", ensure_nats)
    monkeypatch.setattr(launch, "ensure_api", ensure_api)

    # Any teardown of the platform would be a bug: a session owns only what it spawned.
    def fail_stop() -> bool:
        raise AssertionError("run() must never stop the API — the platform survives a session")

    monkeypatch.setattr(launch, "stop_api", fail_stop)

    def fail_docker(*_: Any, **__: Any) -> None:
        raise AssertionError("run() must never run docker (compose down) — the platform survives")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fail_docker)

    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: [])
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    assert launch.run(RecordingLauncher("collatz", [])) == 0
    # NATS then the API were ensured, in that order.
    assert ensured == ["nats", "api"]


def test_run_prints_urls_once_the_stack_is_up(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    spawned = [
        (
            launch.Service(name="viewer", serve=["viewer"], url="http://localhost:5173"),
            FakeServeProcess("viewer"),
        ),
        # A service with no URL prints nothing.
        (launch.Service(name="worker", serve=["worker"]), FakeServeProcess("worker")),
    ]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    launch.run(RecordingLauncher("collatz", []))
    out = capsys.readouterr().out
    assert "collatz is up" in out
    assert "http://localhost:5173" in out


def test_spawn_serves_uses_popen_with_cwd(monkeypatch: pytest.MonkeyPatch) -> None:
    captured: list[tuple[list[str], object]] = []

    def fake_popen(cmd: list[str], cwd: object = None, **_: Any) -> FakeServeProcess:
        captured.append((cmd, cwd))
        return FakeServeProcess("x")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fake_popen)

    services = [
        launch.Service(name="viewer", serve=["npm", "run", "serve"], cwd="/apps/collatz/viewer"),
    ]
    spawned = launch._spawn_serves(services)

    assert captured == [(["npm", "run", "serve"], "/apps/collatz/viewer")]
    assert len(spawned) == 1


def test_terminate_in_reverse_returns_zero_when_all_clean() -> None:
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="a", serve=["a"]), FakeServeProcess("a")),
        (launch.Service(name="b", serve=["b"]), FakeServeProcess("b")),
    ]
    assert launch._terminate_in_reverse(spawned) == 0


def test_run_returns_zero_when_serves_die_by_sigterm(monkeypatch: pytest.MonkeyPatch) -> None:
    # The realistic default: a serve with no SIGTERM handler (e.g. `python -m http.server`) is
    # killed by terminate() and reports -SIGTERM. A normal session must still exit 0.
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    serves = [FakeServeProcess("viewer"), FakeServeProcess("worker")]
    spawned = [(launch.Service(name=p.name, serve=[p.name]), p) for p in serves]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    assert launch.run(RecordingLauncher("collatz", [])) == 0
    assert all(p.wait() == -signal.SIGTERM for p in serves)


def test_run_returns_zero_when_serves_already_died_by_group_sigint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Terminal Ctrl-C sends SIGINT to the whole foreground group, so children may already be dead
    # with -SIGINT before teardown runs. That is a clean session exit, not a failure.
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    serves = [
        FakeServeProcess("viewer", exit_code=-signal.SIGINT),
        FakeServeProcess("worker", exit_code=-signal.SIGINT),
    ]
    spawned = [(launch.Service(name=p.name, serve=[p.name]), p) for p in serves]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    assert launch.run(RecordingLauncher("collatz", [])) == 0
    # Already dead: teardown reaps them without terminating.
    assert not any(p.terminated for p in serves)


def test_terminate_in_reverse_treats_other_signal_death_as_failure() -> None:
    # Death by a signal other than SIGTERM/SIGINT (here SIGKILL) is a genuine failure.
    killed = FakeServeProcess("worker", exit_code=-signal.SIGKILL)
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="worker", serve=["worker"]), killed),
    ]
    assert launch._terminate_in_reverse(spawned) == -signal.SIGKILL


def test_is_clean_exit_classifies_teardown_codes() -> None:
    assert launch._is_clean_exit(0) is True
    assert launch._is_clean_exit(-signal.SIGTERM) is True
    assert launch._is_clean_exit(-signal.SIGINT) is True
    # A crash and death by another signal are both failures.
    assert launch._is_clean_exit(7) is False
    assert launch._is_clean_exit(-signal.SIGKILL) is False


def test_spawn_serves_terminates_earlier_serves_when_a_later_spawn_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A realistic mid-spawn failure: the second serve's executable is missing. The first, already
    # spawned, must be terminated before the error propagates — no leaked child.
    first = FakeServeProcess("viewer")

    spawns: list[list[str]] = []

    def fake_popen(cmd: list[str], cwd: object = None, **_: Any) -> FakeServeProcess:
        spawns.append(cmd)
        if cmd == ["missing-exe"]:
            raise OSError("No such file or directory: 'missing-exe'")
        return first

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fake_popen)

    services = [
        launch.Service(name="viewer", serve=["viewer"]),
        launch.Service(name="worker", serve=["missing-exe"]),
    ]

    with pytest.raises(OSError, match="missing-exe"):
        launch._spawn_serves(services)
    # The earlier serve was terminated during the aborted spawn — nothing leaks.
    assert first.terminated is True


def test_run_terminates_serves_when_interrupt_lands_before_the_block(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A KeyboardInterrupt arriving between spawning serves and entering the block-until-interrupt
    # loop (e.g. while printing URLs) must still tear the serves down — the try/finally ensures it —
    # and end the session with the conventional interrupt exit code, never a traceback.
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    serve = FakeServeProcess("viewer")
    spawned = [(launch.Service(name="viewer", serve=["viewer"]), serve)]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)

    def interrupt_immediately() -> None:
        raise KeyboardInterrupt

    # Raise during the URL-printing phase, before _block_until_interrupt_or_exit is even reached.
    monkeypatch.setattr("builtins.print", lambda *a, **k: interrupt_immediately())

    assert launch.run(RecordingLauncher("collatz", [])) == 130
    # The finally block ran: the spawned serve was terminated though the block was never entered.
    assert serve.terminated is True


def test_run_returns_interrupt_code_on_ctrl_c_during_prepare(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Ctrl-C during a long prepare step (npm install on a cold checkout is the realistic case) must
    # end the session cleanly with code 130 — not escape run() as a traceback.
    _no_platform(monkeypatch)

    def interrupted(_services: Any) -> int:
        raise KeyboardInterrupt

    monkeypatch.setattr(launch, "_run_prepare_steps", interrupted)

    def fail_spawn(*_: Any, **__: Any) -> None:
        raise AssertionError("no serve may be spawned after an interrupt during prepare")

    monkeypatch.setattr(launch, "_spawn_serves", fail_spawn)

    assert launch.run(RecordingLauncher("collatz", [])) == 130


def test_run_returns_interrupt_code_on_ctrl_c_during_ensure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Ctrl-C while the platform is being ensured is likewise a clean end of the session.
    def interrupted() -> launch.Outcome:
        raise KeyboardInterrupt

    monkeypatch.setattr(launch, "ensure_nats", interrupted)

    assert launch.run(RecordingLauncher("collatz", [])) == 130


def test_run_ends_when_a_serve_exits_on_its_own(monkeypatch: pytest.MonkeyPatch) -> None:
    # A serve that dies (e.g. its port was already bound) must end the session and surface the
    # failure — not leave run() blocking forever while claiming the stack is up. The real
    # _block_until_interrupt_or_exit is exercised: the dead child ends the wait with no Ctrl-C.
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    live = FakeServeProcess("viewer")
    crashed = FakeServeProcess("worker", exit_code=1)
    spawned = [
        (launch.Service(name="viewer", serve=["viewer"]), live),
        (launch.Service(name="worker", serve=["worker"]), crashed),
    ]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)

    def fail_sleep(_seconds: float) -> None:
        raise AssertionError("the wait must end on the already-dead child without sleeping")

    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", fail_sleep)

    assert launch.run(RecordingLauncher("collatz", [])) == 1
    # The survivor was torn down once the crashed serve ended the session.
    assert live.terminated is True
    assert crashed.terminated is False


def test_block_until_interrupt_or_exit_returns_when_a_child_dies(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The wait polls between sleeps: a child dying mid-session ends the loop.
    proc = FakeServeProcess("viewer")
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="viewer", serve=["viewer"]), proc)
    ]

    def die_during_sleep(_seconds: float) -> None:
        proc._exit_code = 3

    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", die_during_sleep)
    launch._block_until_interrupt_or_exit(spawned)  # returns instead of blocking
    assert proc.poll() == 3


def test_block_until_interrupt_or_exit_sleeps_then_can_be_interrupted(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # With every child live, the loop sleeps; KeyboardInterrupt (Ctrl-C) still ends it — proving it
    # loops while healthy and stays interruptible.
    proc = FakeServeProcess("viewer")
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="viewer", serve=["viewer"]), proc)
    ]
    calls = iter([None])

    def fake_sleep(_seconds: float) -> None:
        try:
            next(calls)
        except StopIteration:
            raise KeyboardInterrupt from None

    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", fake_sleep)
    with pytest.raises(KeyboardInterrupt):
        launch._block_until_interrupt_or_exit(spawned)
