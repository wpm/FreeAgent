"""Ensure and own the Free Agent platform: NATS (docker) and the ``freeagent-api`` host process.

The platform is app-agnostic — one NATS network and one API serve every installed application — so
bringing it up belongs here in the SDK, not in any app. Both the ``start``/``stop`` switch and per-
app launch sessions call this one code path, and because the compose file and NATS config ship as
SDK package data, it also works from a third-party repo that only depends on ``freeagent-sdk``.

Everything here is stdlib-only (``subprocess``, ``urllib.request``, ``signal``, ``os``) so the SDK
gains no new dependency.

The API cannot join the compose network yet: it spawns ``python -m freeagent.worker.cli`` in its own
environment, so it must run on the host in the venv where the applications live (see ADR-0009). We
therefore own it as a detached host process tracked by a pid file. Health — not the pid file — is
the source of truth: a pid file pointing at a dead process is stale and simply overwritten.
"""

from __future__ import annotations

import enum
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from importlib import resources
from pathlib import Path

NATS_HEALTH_URL = "http://localhost:8222/healthz"
"""NATS's HTTP monitoring health endpoint; a 200 means the network is up."""

API_HEALTH_URL = "http://localhost:8000/health"
"""The ``freeagent-api`` health endpoint on its default bind (``127.0.0.1:8000``)."""

API_EXECUTABLE = "freeagent-api"
"""The console script the API is spawned from; resolved on ``PATH`` within the active venv."""

STATE_DIR_NAME = ".freeagent"
"""The per-repo state directory holding the API pid file."""

API_PID_FILE_NAME = "api.pid"
"""The pid file recording the detached API process, under :data:`STATE_DIR_NAME`."""

_PACKAGE_DATA = "freeagent.sdk._platform"
"""The package holding the shipped compose file and NATS config."""

_COMPOSE_FILE_NAME = "compose.yaml"
"""The compose file's name within :data:`_PACKAGE_DATA`."""

_HEALTH_TIMEOUT_SECONDS = 30.0
"""How long to wait for a service's health endpoint after starting it before giving up."""

_HEALTH_POLL_INTERVAL_SECONDS = 0.25
"""Delay between health-endpoint polls while waiting for a service to come up."""

_STOP_TIMEOUT_SECONDS = 10.0
"""How long to wait for the API to exit after SIGTERM before giving up."""

_STOP_POLL_INTERVAL_SECONDS = 0.1
"""Delay between liveness checks while waiting for the API to exit."""


class Outcome(enum.Enum):
    """Whether an ensure call had to start the service or found it already running.

    Callers (e.g. the ``start`` switch and launch sessions) use this to print ``started X`` versus
    ``X already running``.
    """

    STARTED = "started"
    ALREADY_RUNNING = "already running"


class DockerUnavailableError(RuntimeError):
    """Raised by :func:`ensure_nats` when docker cannot be used to bring NATS up."""


def _is_healthy(url: str) -> bool:
    """Report whether an HTTP health endpoint answers with a 2xx status.

    Any connection error, timeout, or non-2xx response counts as unhealthy — the service is either
    down or not yet ready.

    :param url: The health endpoint to probe.
    :return: True if the endpoint returned a 2xx status, False otherwise.
    """
    try:
        with urllib.request.urlopen(url, timeout=2) as response:
            return bool(200 <= response.status < 300)
    except (urllib.error.URLError, OSError):
        return False


def _wait_until_healthy(url: str, timeout: float = _HEALTH_TIMEOUT_SECONDS) -> bool:
    """Poll a health endpoint until it is healthy or the timeout elapses.

    :param url: The health endpoint to poll.
    :param timeout: How long to keep polling, in seconds.
    :return: True if the endpoint became healthy in time, False on timeout.
    """
    deadline = time.monotonic() + timeout
    while True:
        if _is_healthy(url):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(_HEALTH_POLL_INTERVAL_SECONDS)


def _repo_root() -> Path | None:
    """Find the enclosing git repository root, if the current directory is inside one.

    Walks up from the current working directory looking for a ``.git`` entry. Used to place the
    state directory at the repo root when in-repo, so every session in the repo shares one pid file.

    :return: The repository root, or ``None`` when not inside a repository.
    """
    for directory in (Path.cwd(), *Path.cwd().parents):
        if (directory / ".git").exists():
            return directory
    return None


def _state_dir() -> Path:
    """Return the directory holding platform state (the API pid file).

    The repo root when inside a repository, otherwise the current working directory — so an in-repo
    invocation from any subdirectory shares one pid file, and a third-party invocation still gets a
    stable location.

    :return: The state directory (``.freeagent`` is created beneath it on write, not here).
    """
    return _repo_root() or Path.cwd()


def _api_pid_file() -> Path:
    """Return the path to the API pid file under the state directory."""
    return _state_dir() / STATE_DIR_NAME / API_PID_FILE_NAME


def _read_pid(pid_file: Path) -> int | None:
    """Read a pid from a pid file, tolerating a missing or malformed file.

    :param pid_file: The file to read.
    :return: The recorded pid, or ``None`` if the file is absent or does not hold an integer.
    """
    try:
        return int(pid_file.read_text().strip())
    except (FileNotFoundError, ValueError):
        return None


def _process_alive(pid: int) -> bool:
    """Report whether a process with the given pid exists.

    Sends signal 0, which performs the kernel's existence-and-permission check without delivering a
    signal. A ``ProcessLookupError`` means the process is gone; a ``PermissionError`` means it
    exists but is owned by someone else (treated as alive).

    :param pid: The process id to check.
    :return: True if the process exists, False otherwise.
    """
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def resolve_compose_file(compose_file: str | os.PathLike[str] | None) -> Path:
    """Resolve the compose file to use, falling back to the packaged copy.

    When ``compose_file`` is given (this repo passes ``docker/compose.yaml``), it is used verbatim.
    Otherwise the copy shipped as SDK package data is materialized to a real filesystem path via
    :func:`importlib.resources.as_file` — so third-party repos get the platform for free.

    The caller is responsible for keeping any :class:`contextlib.ExitStack`/context alive; here we
    extract the concrete path from the packaged resource eagerly because the resource is a plain
    file in an installed wheel (never a zip member in this project), so the path is stable.

    :param compose_file: An explicit compose-file path, or ``None`` to use the packaged copy.
    :return: The filesystem path to the compose file.
    """
    if compose_file is not None:
        return Path(compose_file)
    with resources.as_file(resources.files(_PACKAGE_DATA).joinpath(_COMPOSE_FILE_NAME)) as packaged:
        return Path(packaged)


def ensure_nats(compose_file: str | os.PathLike[str] | None = None) -> Outcome:
    """Ensure the NATS docker network is up, starting it if necessary.

    Polls :data:`NATS_HEALTH_URL`; if NATS already answers, this is a no-op. Otherwise runs ``docker
    compose --file <resolved> up --detach --wait`` and confirms health.

    :param compose_file: An explicit compose file, or ``None`` to use the packaged copy; see
        :func:`resolve_compose_file`.
    :return: :attr:`Outcome.ALREADY_RUNNING` if NATS was already healthy, else
        :attr:`Outcome.STARTED`.
    :raises DockerUnavailableError: If the ``docker`` executable is missing, or ``docker compose
        up`` fails (e.g. the daemon is not running).
    """
    if _is_healthy(NATS_HEALTH_URL):
        return Outcome.ALREADY_RUNNING

    if shutil.which("docker") is None:
        raise DockerUnavailableError(
            "docker is required to start NATS but was not found on PATH. Install Docker and ensure "
            "the daemon is running, then try again."
        )

    resolved = resolve_compose_file(compose_file)
    result = subprocess.run(
        ["docker", "compose", "--file", str(resolved), "up", "--detach", "--wait"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise DockerUnavailableError(
            "failed to start NATS with docker compose "
            f"(exit code {result.returncode}). Is the Docker daemon running?\n"
            f"{result.stderr.strip()}"
        )
    return Outcome.STARTED


def ensure_api() -> Outcome:
    """Ensure the ``freeagent-api`` host process is up, spawning it detached if necessary.

    Polls :data:`API_HEALTH_URL`; if the API already answers, this is a no-op. Otherwise spawns the
    ``freeagent-api`` console script in its own session (detached from this process group so it
    outlives the caller), records its pid in the pid file, and polls health until ready.

    Health is the source of truth: a pid file left over from a dead process is stale and gets
    overwritten. The API is app-agnostic and belongs to no session, so callers ensure but never own
    it — only the ``start``/``stop`` switch tears it down (see :func:`stop_api`).

    :return: :attr:`Outcome.ALREADY_RUNNING` if the API was already healthy, else
        :attr:`Outcome.STARTED`.
    :raises SystemExit: If the ``freeagent-api`` executable is missing from the environment, with a
        message telling the author to add ``freeagent-api`` to their dev dependencies; or if the API
        was spawned but never became healthy within the timeout.
    """
    if _is_healthy(API_HEALTH_URL):
        return Outcome.ALREADY_RUNNING

    if shutil.which(API_EXECUTABLE) is None:
        sys.exit(
            f"the {API_EXECUTABLE!r} executable was not found in this environment. Add "
            f"'freeagent-api' to your dev dependencies so the platform can start the API."
        )

    # start_new_session detaches the child into its own session and process group, so it survives
    # this process exiting (a foreground launch session's Ctrl-C must not take the shared API down).
    process = subprocess.Popen([API_EXECUTABLE], start_new_session=True)

    pid_file = _api_pid_file()
    pid_file.parent.mkdir(parents=True, exist_ok=True)
    pid_file.write_text(str(process.pid))

    if not _wait_until_healthy(API_HEALTH_URL):
        sys.exit(
            f"the API was started (pid {process.pid}) but did not become healthy at "
            f"{API_HEALTH_URL} within {_HEALTH_TIMEOUT_SECONDS:.0f}s."
        )
    return Outcome.STARTED


def stop_api() -> bool:
    """Stop the API this platform started, if any, and remove its pid file.

    SIGTERMs the pid recorded in the pid file, waits for the process to exit, then removes the file.
    A silent no-op when nothing is recorded or the recorded process is already gone (a stale pid
    file is simply cleared) — so ``stop`` is safe to call unconditionally.

    :return: True if a live API was signalled and stopped, False if there was nothing to do.
    """
    pid_file = _api_pid_file()
    pid = _read_pid(pid_file)

    if pid is None or not _process_alive(pid):
        pid_file.unlink(missing_ok=True)
        return False

    os.kill(pid, signal.SIGTERM)

    deadline = time.monotonic() + _STOP_TIMEOUT_SECONDS
    while _process_alive(pid) and time.monotonic() < deadline:
        time.sleep(_STOP_POLL_INTERVAL_SECONDS)

    pid_file.unlink(missing_ok=True)
    return True
