"""Unit tests for :mod:`freeagent.sdk.launch`.

Every test mocks docker, subprocess spawning, health polling, and signalling, so the suite needs
neither a Docker daemon nor a live API. The behaviours under test are the ones the issue calls out:
idempotent ensure, the clear docker-unavailable error, the missing-``freeagent-api`` message, and
stale-pid handling.
"""

from __future__ import annotations

import json
import signal
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from freeagent.sdk import launch


@pytest.fixture
def state_dir_in_tmp(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """Pin the pid-file location under a temp directory so tests never touch a real repo.

    Patches :func:`freeagent.sdk.launch._state_dir` to return a ``.freeagent`` directory under a
    temp path, mirroring the real layout (the pid and log files live directly inside the state
    directory). Not autouse: the dedicated ``_state_dir`` tests exercise the real resolution, so
    they must not request this fixture.

    :return: The temp base directory; the state directory is ``<base>/.freeagent``.
    """
    monkeypatch.setattr(launch, "_state_dir", lambda: tmp_path / launch.STATE_DIR_NAME)
    yield tmp_path


class FakeResponse:
    """A minimal stand-in for the object ``urllib.request.urlopen`` yields as a context manager.

    Carries a ``status`` for the liveness check and an optional ``body`` for the identity probe:
    :func:`launch._probe_listener` calls ``read()`` and parses the bytes as JSON. The default body
    is a bare ``{"status": "ok"}`` (no identity), so an unspecified probe reads as an unidentified
    listener rather than a Free Agent API.
    """

    def __init__(self, status: int, body: bytes = b'{"status": "ok"}') -> None:
        self.status = status
        self._body = body

    def read(self) -> bytes:
        return self._body

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_: object) -> None:
        return None


def _identity_body(executable: str, pid: int = 4242) -> bytes:
    """Serialize a ``/health`` identity payload the way the API does, for driving the probe.

    :param executable: The serving interpreter path to report.
    :param pid: The serving pid to report.
    :return: The JSON body bytes a healthy identity-revealing ``/health`` returns.
    """
    return json.dumps({"status": "ok", "executable": executable, "pid": pid}).encode()


def _patch_health(
    monkeypatch: pytest.MonkeyPatch, healthy_urls: set[str], body: bytes | None = None
) -> list[str]:
    """Patch ``urlopen`` so URLs in ``healthy_urls`` return 200 and all others raise.

    :param healthy_urls: The URLs that answer 200; every other URL raises a connection error.
    :param body: The response body healthy URLs return; defaults to a bare ``{"status": "ok"}``
        liveness payload (no identity), so ``ensure_api`` probes read as an unidentified listener
        unless a caller passes an identity payload via :func:`_identity_body`.
    :return: A list that records every URL probed, in order.
    """
    probed: list[str] = []

    def fake_urlopen(url: str, timeout: float = 0) -> FakeResponse:
        probed.append(url)
        if url in healthy_urls:
            return FakeResponse(200) if body is None else FakeResponse(200, body)
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


def test_ensure_nats_reconcile_runs_compose_up_even_when_healthy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The platform owner (`start`) reconciles: NATS answering healthz is not enough — a stale
    # container from an old config must be brought in line with the compose file, so `compose up
    # --detach --wait` runs unconditionally. The outcome reports the reconciliation honestly: NATS
    # was already up, so it was RECONCILED, not "started".
    _patch_health(monkeypatch, {launch.NATS_HEALTH_URL})
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/usr/bin/docker")

    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_: Any) -> Any:
        calls.append(cmd)
        return type("R", (), {"returncode": 0, "stderr": ""})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)

    assert launch.ensure_nats(reconcile=True) is launch.Outcome.RECONCILED
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


def test_ensure_nats_reconcile_reports_started_when_nats_was_down(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Reconciling a platform that was not up at all is a plain start, and reports as one.
    _patch_health(monkeypatch, set())
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/usr/bin/docker")

    def fake_run(cmd: list[str], **_: Any) -> Any:
        return type("R", (), {"returncode": 0, "stderr": ""})()

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fake_run)

    assert launch.ensure_nats(reconcile=True) is launch.Outcome.STARTED


def test_ensure_nats_reconcile_still_needs_docker(monkeypatch: pytest.MonkeyPatch) -> None:
    # Reconciliation runs docker unconditionally, so a missing docker must still raise clearly even
    # when NATS happens to be answering healthz.
    _patch_health(monkeypatch, {launch.NATS_HEALTH_URL})
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: None)

    with pytest.raises(launch.DockerUnavailableError, match="docker is required"):
        launch.ensure_nats(reconcile=True)


def test_ensure_nats_without_reconcile_short_circuits_when_healthy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A session (the default) must never disrupt a NATS other sessions share: answering healthz is
    # enough, and docker is never touched.
    _patch_health(monkeypatch, {launch.NATS_HEALTH_URL})

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("a session must not run docker compose when NATS is already healthy")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.run", fail)
    assert launch.ensure_nats(reconcile=False) is launch.Outcome.ALREADY_RUNNING


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


def test_ensure_api_already_running_adopts_our_own_api(monkeypatch: pytest.MonkeyPatch) -> None:
    # The listener at the port reports *our* interpreter: it is this environment's API, so ensure is
    # a no-op and nothing is spawned.
    _patch_health(
        monkeypatch, {launch.api_health_url()}, body=_identity_body(sys.executable, pid=4242)
    )

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("the API must not be spawned when our own API is already healthy")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)
    assert launch.ensure_api() is launch.Outcome.ALREADY_RUNNING


def test_ensure_api_adopts_our_own_api_reported_under_a_different_alias(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # The listener reports the same interpreter through a different alias (venvs symlink `python`
    # and `python3` to one binary; the API's console-script shebang may name either). Aliases of one
    # interpreter are the same environment, so ensure must adopt, not reject a legitimate API.
    binary = tmp_path / "python3.12"
    binary.write_bytes(b"")
    ours = tmp_path / "python"
    ours.symlink_to(binary)
    theirs = tmp_path / "python3"
    theirs.symlink_to(binary)
    monkeypatch.setattr("freeagent.sdk.launch.sys.executable", str(ours))
    _patch_health(monkeypatch, {launch.api_health_url()}, body=_identity_body(str(theirs)))

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("the API must not be spawned when our own API is already healthy")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)
    assert launch.ensure_api() is launch.Outcome.ALREADY_RUNNING


def test_ensure_api_rejects_wrong_environment_api(monkeypatch: pytest.MonkeyPatch) -> None:
    # A healthy API answers, but from a *different* venv (the deleted-worktree incident): adopting
    # it would 404 every request, so ensure aborts with the impostor's venv, pid, and a remedy —
    # never silently returning ALREADY_RUNNING.
    impostor = "/deleted/worktree/.venv/bin/python"
    _patch_health(monkeypatch, {launch.api_health_url()}, body=_identity_body(impostor, pid=9999))

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("a wrong-environment API must never be spawned over or adopted")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)

    with pytest.raises(SystemExit) as excinfo:
        launch.ensure_api()
    message = str(excinfo.value)
    assert impostor in message
    assert "9999" in message
    assert sys.executable in message
    # The remedy is actionable: it names how to clear the impostor.
    assert "kill 9999" in message


def test_ensure_api_rejects_identity_less_listener(monkeypatch: pytest.MonkeyPatch) -> None:
    # A process squats the port and answers health checks but reports no Free Agent identity (a
    # non-JSON body here). It is not our API: ensure refuses rather than adopting it or colliding.
    _patch_health(monkeypatch, {launch.api_health_url()}, body=b"not json at all")

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("a non-API listener must never be adopted or spawned over")

    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", fail)

    with pytest.raises(SystemExit) as excinfo:
        launch.ensure_api()
    message = str(excinfo.value)
    assert "not the Free Agent API" in message
    assert launch.api_health_url() in message
    # The lsof remedy names the actual port from the URL, not a misparse of the scheme colon.
    assert "lsof -nP -iTCP:8000 " in message


def test_reject_unidentified_listener_defaults_the_port_from_the_scheme() -> None:
    # A health URL with no explicit port must still yield a usable lsof hint: the scheme's default
    # port, never an empty string misparsed from the scheme colon.
    with pytest.raises(SystemExit) as excinfo:
        launch._reject_unidentified_listener("http://localhost/health")
    assert "lsof -nP -iTCP:80 " in str(excinfo.value)

    with pytest.raises(SystemExit) as excinfo:
        launch._reject_unidentified_listener("https://localhost/health")
    assert "lsof -nP -iTCP:443 " in str(excinfo.value)


def _patch_port_absent(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make the initial probe find the port free, so ``ensure_api`` proceeds to spawn.

    Patches :func:`launch._probe_listener` to report :attr:`launch._ListenerState.ABSENT` — nothing
    is listening — the precondition for the spawn path.
    """
    monkeypatch.setattr(launch, "_probe_listener", lambda _: (launch._ListenerState.ABSENT, None))


def test_ensure_api_spawns_detached_and_writes_pid(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # The port is free on the initial probe; the spawned API then becomes healthy so the wait loop
    # succeeds.
    _patch_port_absent(monkeypatch)
    monkeypatch.setattr(launch, "_wait_until_healthy", lambda url, timeout=0: True)
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
    _patch_port_absent(monkeypatch)
    monkeypatch.setattr(launch, "_wait_until_healthy", lambda url, timeout=0: True)
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

    _patch_port_absent(monkeypatch)
    monkeypatch.setattr(launch, "_wait_until_healthy", lambda url, timeout=0: True)
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
# _probe_listener (the health-identity classifier)
# --------------------------------------------------------------------------------------------------


def test_probe_listener_absent_on_connection_error(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, set())
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.ABSENT
    assert identity is None


def test_probe_listener_absent_on_non_2xx(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "freeagent.sdk.launch.urllib.request.urlopen", lambda url, timeout=0: FakeResponse(503)
    )
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.ABSENT
    assert identity is None


def test_probe_listener_identified_on_full_identity(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(
        monkeypatch,
        {launch.api_health_url()},
        body=_identity_body("/venv/bin/python", pid=4242),
    )
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.IDENTIFIED
    assert identity == launch._ListenerIdentity(executable="/venv/bin/python", pid=4242)


def test_probe_listener_unidentified_on_non_json_body(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_health(monkeypatch, {launch.api_health_url()}, body=b"<html>hello</html>")
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.UNIDENTIFIED
    assert identity is None


def test_probe_listener_unidentified_on_json_missing_fields(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Valid JSON, healthy, but with no Free Agent identity (the pre-#117 ``{"status": "ok"}``).
    _patch_health(monkeypatch, {launch.api_health_url()}, body=b'{"status": "ok"}')
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.UNIDENTIFIED
    assert identity is None


def test_probe_listener_unidentified_on_non_object_json(monkeypatch: pytest.MonkeyPatch) -> None:
    # A 2xx JSON body that is not an object (a bare list) carries no identity fields.
    _patch_health(monkeypatch, {launch.api_health_url()}, body=b"[1, 2, 3]")
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.UNIDENTIFIED
    assert identity is None


def test_probe_listener_unidentified_on_non_utf8_body(monkeypatch: pytest.MonkeyPatch) -> None:
    # A squatter serving binary content (an image, a compressed stream): the body is not even
    # decodable text, let alone JSON. That must classify as an unidentified listener — reported with
    # the friendly "free that port" error — not escape as a UnicodeDecodeError traceback.
    _patch_health(monkeypatch, {launch.api_health_url()}, body=b"\xff\xfe\x00\x01binary")
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.UNIDENTIFIED
    assert identity is None


def test_probe_listener_unidentified_when_pid_not_int(monkeypatch: pytest.MonkeyPatch) -> None:
    # An identity whose pid is the wrong type is not a Free Agent API's response shape.
    body = json.dumps({"status": "ok", "executable": "/venv/bin/python", "pid": "nope"}).encode()
    _patch_health(monkeypatch, {launch.api_health_url()}, body=body)
    state, identity = launch._probe_listener(launch.api_health_url())
    assert state is launch._ListenerState.UNIDENTIFIED
    assert identity is None


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
    # Health is the source of truth, and the API is not answering: the pid file is stale.
    monkeypatch.setattr(launch, "_is_healthy", lambda _: False)

    def fail(*_: Any, **__: Any) -> None:
        raise AssertionError("must not signal when the API is not answering health")

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", fail)

    assert launch.stop_api() is False
    assert not pid_file.exists()


def test_stop_api_signals_live_process_and_removes_file(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")

    # The API answers health, so the recorded pid is trusted and signalled.
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    # Dead once signalled, so the wait loop exits promptly.
    monkeypatch.setattr(launch, "_process_alive", lambda _: False)

    signalled: list[tuple[int, int]] = []
    monkeypatch.setattr(
        "freeagent.sdk.launch.os.kill", lambda pid, sig: signalled.append((pid, sig))
    )

    assert launch.stop_api() is True
    assert signalled == [(12345, signal.SIGTERM)]
    assert not pid_file.exists()


def test_stop_api_clears_pid_file_when_signal_finds_recycled_dead_pid(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # Health answers, but the recorded pid vanished between the check and the kill (recycled onto a
    # process that has since exited): os.kill raises ProcessLookupError.
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)

    def raise_lookup(pid: int, sig: int) -> None:
        raise ProcessLookupError

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", raise_lookup)

    # No traceback: the stale file is cleared and "nothing to do" is reported.
    assert launch.stop_api() is False
    assert not pid_file.exists()


def test_stop_api_clears_pid_file_when_signal_is_permission_denied(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # Health answers, but the recorded pid now belongs to another user's process (a stale pid file
    # after a reboot recycled the pid): os.kill raises PermissionError.
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)

    def raise_perm(pid: int, sig: int) -> None:
        raise PermissionError

    monkeypatch.setattr("freeagent.sdk.launch.os.kill", raise_perm)

    # No traceback, and the stale file is cleared so later stops do not fail the same way.
    assert launch.stop_api() is False
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


def test_state_dir_is_repo_state_dir_when_in_repo(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # In a repo the state directory is <repo-root>/.freeagent, shared by every session in the repo
    # regardless of the subdirectory it is launched from.
    repo = tmp_path / "repo"
    (repo / ".git").mkdir(parents=True)
    subdir = repo / "packages" / "thing"
    subdir.mkdir(parents=True)
    monkeypatch.chdir(subdir)
    assert launch._state_dir() == repo / launch.STATE_DIR_NAME


def test_state_dir_is_user_dir_when_not_in_repo_ignoring_cwd(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # Out of a repo the state directory does NOT depend on cwd: two different loose directories
    # resolve to the same user-level path, so ensure-here / stop-there agree (issue #116).
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.delenv(launch.XDG_STATE_HOME_ENV, raising=False)

    first = tmp_path / "loose-one"
    first.mkdir()
    monkeypatch.chdir(first)
    from_first = launch._state_dir()

    second = tmp_path / "loose-two"
    second.mkdir()
    monkeypatch.chdir(second)
    from_second = launch._state_dir()

    assert from_first == from_second == home / launch.STATE_DIR_NAME


def test_state_dir_honors_xdg_state_home_when_not_in_repo(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # When XDG_STATE_HOME is set, the user-level state lives under its freeagent subdirectory.
    xdg = tmp_path / "xdg-state"
    xdg.mkdir()
    monkeypatch.setenv(launch.XDG_STATE_HOME_ENV, str(xdg))
    outside = tmp_path / "loose"
    outside.mkdir()
    monkeypatch.chdir(outside)
    assert launch._state_dir() == xdg / launch._USER_STATE_SUBDIR


def test_state_dir_falls_back_to_home_when_xdg_state_home_empty(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # An empty XDG_STATE_HOME (set but blank) is treated as unset, per the XDG spec.
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv(launch.XDG_STATE_HOME_ENV, "")
    outside = tmp_path / "loose"
    outside.mkdir()
    monkeypatch.chdir(outside)
    assert launch._state_dir() == home / launch.STATE_DIR_NAME


def test_state_dir_ignores_relative_xdg_state_home(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # A relative XDG_STATE_HOME must be ignored (XDG spec): honoring it would resolve against the
    # cwd and reopen the cwd-dependence this fix closes. Fall back to the home directory instead.
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv(launch.XDG_STATE_HOME_ENV, "relative/state")
    outside = tmp_path / "loose"
    outside.mkdir()
    monkeypatch.chdir(outside)
    assert launch._state_dir() == home / launch.STATE_DIR_NAME


def test_ensure_from_one_dir_and_stop_from_another_agree_out_of_repo(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # The exact out-of-repo scenario the issue calls out: an app session ensures the API from one
    # directory and the switch stops it from another. Both must resolve the same pid file so stop
    # actually finds and kills the API, instead of orphaning it while the port stays held.
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.delenv(launch.XDG_STATE_HOME_ENV, raising=False)

    # No enclosing repo from either directory.
    monkeypatch.setattr(launch, "_repo_root", lambda: None)
    monkeypatch.setattr("freeagent.sdk.launch.shutil.which", lambda _: "/venv/bin/freeagent-api")
    monkeypatch.setattr("freeagent.sdk.launch.subprocess.Popen", FakePopen)

    # ensure_api from directory A: the port is free on the initial identity probe, so the API is
    # spawned; it then becomes healthy so the wait loop succeeds. The probe is intercepted at
    # _probe_listener (ensure gates on identity, not bare liveness — issue #117), so this stays
    # hermetic and never reaches a real listener on the port.
    ensure_dir = tmp_path / "app-a"
    ensure_dir.mkdir()
    monkeypatch.chdir(ensure_dir)
    _patch_port_absent(monkeypatch)
    monkeypatch.setattr(launch, "_wait_until_healthy", lambda url, timeout=0: True)
    assert launch.ensure_api() is launch.Outcome.STARTED
    pid_path = launch._api_pid_file()
    assert pid_path.read_text() == str(FakePopen.instances[0].pid)

    # stop_api from directory B must resolve that same pid file and signal the recorded pid. stop
    # gates on health (issue #114), so the API answers healthy; the recorded pid is then alive for
    # the initial post-signal check and gone once signalled.
    stop_dir = tmp_path / "switch-b"
    stop_dir.mkdir()
    monkeypatch.chdir(stop_dir)
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    alive = iter([True, False])
    monkeypatch.setattr(launch, "_process_alive", lambda _: next(alive))
    signalled: list[tuple[int, int]] = []
    monkeypatch.setattr(
        "freeagent.sdk.launch.os.kill", lambda pid, sig: signalled.append((pid, sig))
    )

    assert launch.stop_api() is True
    assert signalled == [(FakePopen.instances[0].pid, signal.SIGTERM)]
    assert not pid_path.exists()


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

    # The API answers health, so the pid is signalled; alive for the first wait-loop check, then
    # gone: the loop sleeps once.
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    alive = iter([True, False])
    monkeypatch.setattr(launch, "_process_alive", lambda _: next(alive))
    monkeypatch.setattr("freeagent.sdk.launch.os.kill", lambda pid, sig: None)
    slept: list[float] = []
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda seconds: slept.append(seconds))

    assert launch.stop_api() is True
    assert slept == [launch._STOP_POLL_INTERVAL_SECONDS]
    assert not pid_file.exists()


def test_stop_api_escalates_to_sigkill_when_sigterm_ignored(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # A SIGTERM-ignoring API (or one wedged mid-request) must be force-killed, not orphaned with its
    # port still bound where a later start() would see a zombie answering /health.
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")

    # The API answers health, so the recorded pid is trusted and signalled.
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    # Still alive across the whole SIGTERM wait (times out), gone only once SIGKILL is delivered.
    signalled: list[tuple[int, int]] = []

    def fake_alive(_pid: int) -> bool:
        # Dead only after SIGKILL was sent.
        return signal.SIGKILL not in {sig for _, sig in signalled}

    monkeypatch.setattr(launch, "_process_alive", fake_alive)
    monkeypatch.setattr(
        "freeagent.sdk.launch.os.kill", lambda pid, sig: signalled.append((pid, sig))
    )
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda _seconds: None)
    # Make the SIGTERM wait expire immediately so the test doesn't spin for 10s.
    monkeypatch.setattr(launch, "_STOP_TIMEOUT_SECONDS", 0.0)

    assert launch.stop_api() is True
    assert signalled == [(12345, signal.SIGTERM), (12345, signal.SIGKILL)]
    assert not pid_file.exists()


def test_stop_api_raises_and_keeps_pid_file_when_sigkill_fails(
    monkeypatch: pytest.MonkeyPatch, state_dir_in_tmp: Path
) -> None:
    # The nastiest case: even SIGKILL cannot reap the process in time (e.g. stuck in uninterruptible
    # I/O). stop_api must raise a distinct failure (not a False a caller would read as "nothing was
    # running") and keep the pid file so the live API is not silently forgotten.
    pid_file = state_dir_in_tmp / launch.STATE_DIR_NAME / launch.API_PID_FILE_NAME
    pid_file.parent.mkdir(parents=True)
    pid_file.write_text("12345")

    # The API answers health, so the recorded pid is trusted and signalled.
    monkeypatch.setattr(launch, "_is_healthy", lambda _: True)
    # Never dies, no matter what is signalled.
    monkeypatch.setattr(launch, "_process_alive", lambda _pid: True)
    signalled: list[tuple[int, int]] = []
    monkeypatch.setattr(
        "freeagent.sdk.launch.os.kill", lambda pid, sig: signalled.append((pid, sig))
    )
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda _seconds: None)
    monkeypatch.setattr(launch, "_STOP_TIMEOUT_SECONDS", 0.0)
    monkeypatch.setattr(launch, "_STOP_KILL_TIMEOUT_SECONDS", 0.0)

    with pytest.raises(launch.StopFailedError, match="even after SIGKILL"):
        launch.stop_api()
    # Both signals were tried, and the pid file survives so the failure is not silently erased.
    assert signalled == [(12345, signal.SIGTERM), (12345, signal.SIGKILL)]
    assert pid_file.exists()


def test_wait_for_pid_exit_true_when_already_gone(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(launch, "_process_alive", lambda _pid: False)
    assert launch._wait_for_pid_exit(12345, timeout=1.0) is True


def test_wait_for_pid_exit_false_on_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(launch, "_process_alive", lambda _pid: True)
    monkeypatch.setattr("freeagent.sdk.launch.time.sleep", lambda _seconds: None)
    assert launch._wait_for_pid_exit(12345, timeout=0.0) is False


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
    :param ignores_sigterm: If True, the serve ignores ``terminate``: ``wait(timeout=...)`` raises
        :class:`subprocess.TimeoutExpired` until ``kill`` (SIGKILL) is delivered, after which
        ``wait`` reports ``-SIGKILL``. Models the shell/``npm`` wrapper that does not forward
        signals — the case bounded-wait teardown must escalate rather than hang on.
    :param unreapable: If True, the serve never becomes reapable: ``wait(timeout=...)`` raises
        :class:`subprocess.TimeoutExpired` even after ``kill``. Models a child stuck in
        uninterruptible I/O — teardown must abandon the wait rather than hang or crash.
    """

    def __init__(
        self,
        name: str,
        exit_code: int | None = None,
        term_code: int = -signal.SIGTERM,
        ignores_sigterm: bool = False,
        unreapable: bool = False,
    ) -> None:
        self.name = name
        self._term_code = term_code
        # An unreapable child also ignores SIGTERM (it never exits on its own), so the SIGTERM wait
        # must time out and escalate before the post-SIGKILL wait is reached.
        self._ignores_sigterm = ignores_sigterm or unreapable
        self._unreapable = unreapable
        self.terminated = False
        self.killed = False
        # A serve already exited on its own (crashed, or group-SIGINT'd) is reaped in place.
        self._exit_code = exit_code

    def poll(self) -> int | None:
        return self._exit_code

    def terminate(self) -> None:
        self.terminated = True
        if not self._ignores_sigterm:
            self._exit_code = self._term_code

    def kill(self) -> None:
        self.killed = True
        if not self._unreapable:
            self._exit_code = -signal.SIGKILL

    def wait(self, timeout: float | None = None) -> int:
        # A serve that ignores SIGTERM does not exit on terminate(): a bounded wait times out until
        # kill() (SIGKILL) has been delivered, mirroring a real Popen.wait(timeout=...). An
        # unreapable child times out even after kill(), so it never returns an exit code.
        if self._exit_code is None:
            if self._unreapable or (self._ignores_sigterm and not self.killed):
                raise subprocess.TimeoutExpired(cmd=self.name, timeout=timeout or 0)
            # A real wait() blocks until exit; a live serve here is treated as teardown-terminated.
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


def test_terminate_in_reverse_escalates_to_sigkill_when_sigterm_ignored() -> None:
    # A serve that ignores SIGTERM (a shell/npm wrapper that doesn't forward signals): the bounded
    # wait times out, teardown escalates to SIGKILL rather than hang, and the child is reaped.
    stubborn = FakeServeProcess("worker", ignores_sigterm=True)
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="worker", serve=["worker"]), stubborn),
    ]

    # Death by SIGKILL after ignoring SIGTERM is a genuine failure, not a clean teardown.
    assert launch._terminate_in_reverse(spawned) == -signal.SIGKILL
    assert stubborn.terminated is True
    assert stubborn.killed is True


def test_terminate_one_uses_the_bounded_teardown_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    # The wait must be bounded — a real Popen.wait() with no timeout would hang on a stuck child.
    waited: list[float | None] = []
    proc: Any = FakeServeProcess("viewer")
    original_wait = FakeServeProcess.wait

    def record_wait(self: FakeServeProcess, timeout: float | None = None) -> int:
        waited.append(timeout)
        return original_wait(self, timeout)

    monkeypatch.setattr(FakeServeProcess, "wait", record_wait)

    assert launch._terminate_one(launch.Service(name="viewer", serve=["viewer"]), proc) < 0
    assert waited == [launch._TEARDOWN_TIMEOUT_SECONDS]


def test_terminate_one_abandons_the_wait_when_even_sigkill_will_not_reap() -> None:
    # A child stuck in uninterruptible I/O does not become reapable even after SIGKILL.
    # _terminate_one must not hang on the post-kill wait or let TimeoutExpired escape: it reports
    # the child as force-killed (-SIGKILL) and returns, so teardown of the rest continues and run()
    # never crashes.
    unreapable: Any = FakeServeProcess("worker", unreapable=True)
    assert launch._terminate_one(launch.Service(name="worker", serve=["worker"]), unreapable) == (
        -signal.SIGKILL
    )
    assert unreapable.terminated is True
    assert unreapable.killed is True


def test_terminate_one_force_kills_the_in_flight_child_on_interrupt(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A Ctrl-C landing while _terminate_one waits on this child's SIGTERM must not leave that child
    # running: it is SIGKILLed before the interrupt is allowed to unwind.
    proc: Any = FakeServeProcess("viewer")

    def wait_interrupted(_self: FakeServeProcess, timeout: float | None = None) -> int:
        raise KeyboardInterrupt

    monkeypatch.setattr(FakeServeProcess, "wait", wait_interrupted)

    with pytest.raises(KeyboardInterrupt):
        launch._terminate_one(launch.Service(name="viewer", serve=["viewer"]), proc)
    # The in-flight child was force-killed before the interrupt propagated — it cannot leak.
    assert proc.terminated is True
    assert proc.killed is True


def test_terminate_in_reverse_reaps_remaining_children_on_mid_teardown_interrupt(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A second Ctrl-C mid-teardown must not abandon the earlier-spawned children: the interrupt is
    # caught so the loop finishes terminating every child, then re-raised for run() to handle.
    first = FakeServeProcess("first")
    second = FakeServeProcess("second")
    third = FakeServeProcess("third")
    spawned: list[tuple[launch.Service, Any]] = [
        (launch.Service(name="first", serve=["first"]), first),
        (launch.Service(name="second", serve=["second"]), second),
        (launch.Service(name="third", serve=["third"]), third),
    ]

    original_wait = FakeServeProcess.wait

    def interrupt_on_second_wait(self: FakeServeProcess, timeout: float | None = None) -> int:
        # Teardown runs in reverse order (third, second, first); the interrupt lands while waiting
        # on the second child's SIGTERM.
        if self.name == "second":
            raise KeyboardInterrupt
        return original_wait(self, timeout)

    monkeypatch.setattr(FakeServeProcess, "wait", interrupt_on_second_wait)

    with pytest.raises(KeyboardInterrupt):
        launch._terminate_in_reverse(spawned)

    # Every child was terminated — none leaked. The interrupted middle child was force-killed by
    # _terminate_one before the interrupt unwound; the earlier 'first' was still torn down after.
    assert third.terminated is True
    assert second.killed is True
    assert first.terminated is True


def test_run_reaps_children_and_returns_130_on_second_ctrl_c_during_teardown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # End-to-end: a first Ctrl-C ends the block, then a second Ctrl-C lands mid-teardown. run() must
    # still terminate every spawned child and return the conventional interrupt code, not a
    # traceback and not a leaked process.
    _no_platform(monkeypatch)
    monkeypatch.setattr(launch, "_run_prepare_steps", lambda services: 0)

    early = FakeServeProcess("viewer")
    late = FakeServeProcess("worker")
    spawned = [
        (launch.Service(name="viewer", serve=["viewer"]), early),
        (launch.Service(name="worker", serve=["worker"]), late),
    ]
    monkeypatch.setattr(launch, "_spawn_serves", lambda services: spawned)
    monkeypatch.setattr(launch, "_block_until_interrupt_or_exit", _raise_keyboard_interrupt)

    original_wait = FakeServeProcess.wait

    def interrupt_on_late_wait(self: FakeServeProcess, timeout: float | None = None) -> int:
        # The 'worker' is torn down first (reverse order); a second Ctrl-C hits while waiting on it.
        if self.name == "worker":
            raise KeyboardInterrupt
        return original_wait(self, timeout)

    monkeypatch.setattr(FakeServeProcess, "wait", interrupt_on_late_wait)

    assert launch.run(RecordingLauncher("collatz", [])) == 130
    # Both children were terminated despite the interrupt mid-teardown — nothing leaked. The
    # interrupted 'worker' was force-killed before the interrupt unwound.
    assert early.terminated is True
    assert late.terminated is True
    assert late.killed is True


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
