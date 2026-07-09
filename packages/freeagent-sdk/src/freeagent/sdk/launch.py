"""Ensure and own the Free Agent platform, and run a per-app foreground launch session.

The platform is app-agnostic — one NATS network and one API serve every installed application — so
bringing it up belongs here in the SDK, not in any app. Both the ``start``/``stop`` switch and per-
app launch sessions call this one code path, and because the compose file and NATS config ship as
SDK package data, it also works from a third-party repo that only depends on ``freeagent-sdk``.

The *platform* half — :func:`ensure_nats`, :func:`ensure_api`, :func:`stop_api` — ensures and owns
NATS and the API. The *session* half — :class:`Service`, the :class:`Launcher` protocol, and
:func:`run` — is what ``uv run <app>`` invokes: it ensures the platform is up, runs an app's serving
processes in the foreground, and on Ctrl-C tears down only what it spawned. The platform always
survives a session; the ``stop`` switch owns turning it off (see ADR-0009).

Everything here is stdlib-only (``subprocess``, ``urllib.request``, ``signal``, ``os``) so the SDK
gains no new dependency.

The API cannot join the compose network yet: it spawns ``python -m freeagent.worker.cli`` in its own
environment, so it must run on the host in the venv where the applications live (see ADR-0009). We
therefore own it as a detached host process tracked by a pid file. Health — not the pid file — is
the source of truth: a pid file pointing at a dead process is stale and simply overwritten.
"""

from __future__ import annotations

import dataclasses
import enum
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Sequence
from importlib import resources
from pathlib import Path
from typing import Protocol, runtime_checkable

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


# --------------------------------------------------------------------------------------------------
# Session half: a launcher describes an app's serving processes; run() runs them in the foreground.
# --------------------------------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class Service:
    """One serving process of an application, plus any build steps it needs first.

    A service is described declaratively so the harness owns every spawn and teardown: a launcher
    (see :class:`Launcher`) never starts a process itself, it hands :func:`run` a list of these.

    ``prepare`` steps run to completion, in order, before *any* service's ``serve`` is spawned;
    a failing prepare step aborts the whole session before it starts a single long-running process
    (an app whose viewer won't build has nothing worth serving). ``serve`` is the long-running
    process the session foregrounds — a dev server, a worker loop — that runs until the session's
    Ctrl-C terminates it.

    :ivar name: A short label for the service, used in the session's console output.
    :ivar serve: The long-running process's argv, e.g. ``["npm", "run", "serve"]``. A ``list``, not
        a bare ``Sequence``: a plain ``str`` satisfies ``Sequence[str]`` but would be split into
        one-character "arguments" by :class:`subprocess.Popen`, so the narrower type rejects that
        mistake at type-check time.
    :ivar prepare: Argvs run to completion, in order, before any ``serve`` starts; empty by default.
    :ivar cwd: The working directory for this service's ``prepare`` and ``serve`` commands, or
        ``None`` to inherit the session's. A viewer's commands, for instance, run in its package
        directory.
    :ivar url: A URL to print once the whole stack is up, e.g. the viewer's address; ``None`` to
        print nothing for this service.
    """

    name: str
    serve: list[str]
    prepare: list[list[str]] = dataclasses.field(default_factory=list)
    cwd: str | os.PathLike[str] | None = None
    url: str | None = None


@runtime_checkable
class Launcher(Protocol):
    """What an application supplies to describe its serving stack: a name and its services.

    The launching sibling of :class:`~freeagent.sdk.application.Application` (ADR-0006). Where an
    ``Application`` tells the worker how to *build an episode's entities*, a ``Launcher`` tells the
    session harness how to *serve the application* — its dev server, its build steps. A launcher
    describes serving processes only; it never creates episodes. Episode creation stays with clients
    (viewer → API → worker), preserving the layering in which the API never imports application or
    worker code.

    There is deliberately no ``freeagent.launchers`` entry-point group yet: each app declares its
    own console script whose ``main()`` hands a launcher to :func:`run`, and a generic
    ``launch <name>`` consumer that would need an enumerable group does not exist yet (ADR-0009).

    Because it is a :class:`~typing.Protocol`, a launcher need not inherit from anything — any
    object with a matching ``name`` and ``services()`` qualifies — and it is
    :func:`~typing.runtime_checkable`, so ``isinstance(obj, Launcher)`` checks the shape at runtime.

    :ivar name: The application's name, used in the session's console output.
    """

    name: str

    def services(self) -> list[Service]:
        """Return the services making up this application's serving stack.

        :return: The :class:`Service` values :func:`run` prepares, spawns, and later tears down.
        """
        ...


def _run_prepare_steps(services: Sequence[Service]) -> int:
    """Run every service's ``prepare`` commands to completion, in order, failing fast.

    Runs each service's prepare steps in the order the services were given, and within a service in
    the order listed. The first command to exit nonzero stops the whole sequence and its exit code
    is returned — no ``serve`` should start once a build step has failed.

    :param services: The services whose prepare steps to run.
    :return: ``0`` if every prepare command succeeded, else the exit code of the first that failed.
    """
    for service in services:
        for command in service.prepare:
            result = subprocess.run(command, cwd=service.cwd)
            if result.returncode != 0:
                print(
                    f"prepare step {command} for {service.name!r} failed "
                    f"(exit code {result.returncode})",
                    file=sys.stderr,
                )
                return result.returncode
    return 0


def _spawn_serves(services: Sequence[Service]) -> list[tuple[Service, subprocess.Popen[bytes]]]:
    """Spawn every service's ``serve`` process, in order, returning them paired with their service.

    Each ``serve`` is a long-running foreground child of this session (unlike the detached platform
    processes): a plain :class:`subprocess.Popen` in the session's own process group, so :func:`run`
    can SIGTERM it during teardown (and a terminal's Ctrl-C, sent to the whole foreground group,
    reaches it too).

    Spawning is all-or-nothing: if a later ``serve`` fails to start (e.g. its executable is
    missing), the already-spawned earlier ones are terminated before the error propagates, so a
    partial launch never leaks running children.

    :param services: The services to spawn ``serve`` processes for.
    :return: Each service paired with its spawned process, in spawn order.
    :raises OSError: If a ``serve`` process cannot be spawned; earlier ones are torn down first.
    """
    spawned: list[tuple[Service, subprocess.Popen[bytes]]] = []
    for service in services:
        try:
            process = subprocess.Popen(service.serve, cwd=service.cwd)
        except OSError:
            _terminate_in_reverse(spawned)
            raise
        spawned.append((service, process))
    return spawned


def _is_clean_exit(returncode: int) -> bool:
    """Report whether a serve process's exit code counts as a clean teardown, not a failure.

    Clean means either a normal zero exit, or death by the signals a session's teardown expects:
    SIGTERM (what :func:`_terminate_in_reverse` sends) or SIGINT (what a terminal delivers to the
    whole foreground process group on Ctrl-C). On POSIX a process killed by signal ``N`` reports a
    *negative* returncode of ``-N``, so ``-SIGTERM`` and ``-SIGINT`` are the expected teardown codes
    and must not make an otherwise-normal session exit nonzero.

    A positive nonzero code (the serve crashed or exited with an error) or death by any *other*
    signal is a genuine failure.

    :param returncode: A :meth:`subprocess.Popen.wait` return value.
    :return: True if the code is a clean teardown exit (``0``, ``-SIGTERM``, or ``-SIGINT``).
    """
    return returncode in (0, -signal.SIGTERM, -signal.SIGINT)


def _terminate_in_reverse(
    spawned: Sequence[tuple[Service, subprocess.Popen[bytes]]],
) -> int:
    """SIGTERM the spawned serve processes in reverse spawn order and collect their exit codes.

    Reverse order tears the stack down in the opposite order it came up — the last thing started is
    the first stopped. Each still-running child is SIGTERMed; a child that had already exited on its
    own (crashed, or killed by the terminal's group SIGINT on Ctrl-C) is reaped in place. Death by
    the teardown signals SIGTERM/SIGINT is a clean exit, not a failure (see :func:`_is_clean_exit`);
    the returned code is the first genuine failure — a positive nonzero exit or death by another
    signal — so a crashed serve, but not a cleanly-terminated one, makes the session exit nonzero.

    :param spawned: The service/process pairs to terminate, in spawn order.
    :return: ``0`` if every child exited cleanly, else the first failing exit code seen, in reverse
        spawn order.
    """
    failure = 0
    for service, process in reversed(spawned):
        if process.poll() is None:
            process.terminate()
        returncode = process.wait()
        if not _is_clean_exit(returncode) and failure == 0:
            failure = returncode
            print(
                f"service {service.name!r} exited with code {returncode}",
                file=sys.stderr,
            )
    return failure


def _block_until_interrupt() -> None:
    """Block the session until it receives SIGINT (Ctrl-C).

    Sleeps indefinitely; :class:`KeyboardInterrupt` (raised by the default SIGINT handler) unwinds
    to :func:`run`, which then tears the session down. Factored out so tests can drive the
    interrupt without a real signal.
    """
    while True:
        time.sleep(_SESSION_POLL_INTERVAL_SECONDS)


_SESSION_POLL_INTERVAL_SECONDS = 1.0
"""How long each idle sleep in the session's block-until-interrupt loop lasts, in seconds."""


def run(launcher: Launcher) -> int:
    """Run an application's serving stack as a foreground session, until Ctrl-C.

    The whole session lifecycle for ``uv run <app>``:

    1. Ensure the platform is up — NATS then the API (:func:`ensure_nats`, :func:`ensure_api`).
       This *ensures* but never owns the platform; a session's teardown leaves it running.
    2. Run every service's ``prepare`` steps to completion, in order, failing fast: the first
       nonzero exit aborts the session with that code, before any ``serve`` is spawned.
    3. Spawn every service's ``serve`` process, then print the stack's URLs.
    4. Block until SIGINT (Ctrl-C).
    5. SIGTERM the spawned ``serve`` processes in reverse order and wait for them. The exit code is
       nonzero if any child exited nonzero.

    Only what this session spawned is torn down. The platform (NATS, the API) is untouched by the
    teardown — the ``stop`` switch owns it — so a second concurrent session keeps working.

    :param launcher: The application's :class:`Launcher`, supplying its :class:`Service` list.
    :return: A process exit code: the failing prepare step's code if one failed, otherwise the
        first nonzero ``serve`` exit code seen during teardown, otherwise ``0``.
    """
    ensure_nats()
    ensure_api()

    services = launcher.services()

    prepare_failure = _run_prepare_steps(services)
    if prepare_failure != 0:
        return prepare_failure

    spawned = _spawn_serves(services)
    try:
        print(f"{launcher.name} is up. Press Ctrl-C to stop.")
        for service, _ in spawned:
            if service.url is not None:
                print(f"  {service.name}: {service.url}")

        try:
            _block_until_interrupt()
        except KeyboardInterrupt:
            pass
    finally:
        # Teardown in a finally so anything spawned is terminated even if we never reach the block
        # (e.g. a KeyboardInterrupt lands between spawn and the loop). The mid-spawn leak — a Popen
        # raising partway through _spawn_serves — is handled inside _spawn_serves itself.
        exit_code = _terminate_in_reverse(spawned)

    return exit_code
