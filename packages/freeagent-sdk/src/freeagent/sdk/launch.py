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

**State-directory policy.** The pid file must resolve to the same path for the ``ensure`` that
starts the API and the ``stop`` that later kills it, regardless of the directory each is invoked
from — otherwise a third-party ``stop`` run from a different directory computes a different path,
reports "nothing to stop", and orphans the API still holding the port. So:

- **Inside a git repository**, state lives at the repo root: ``<repo-root>/.freeagent/``. Every
  session in the repo — from any subdirectory — shares one pid file (the #98 behaviour).
- **Outside any git repository**, state lives in a user-level directory that does not depend on the
  caller's cwd: ``$XDG_STATE_HOME/freeagent/`` when ``XDG_STATE_HOME`` is set, otherwise
  ``~/.freeagent/``. This closes the out-of-repo hole from PR #112's review (issue #116): ensure and
  stop agree wherever they run.

The earlier fallback to :func:`Path.cwd` is gone; nothing here resolves against the caller's cwd.
"""

from __future__ import annotations

import dataclasses
import enum
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Sequence
from importlib import resources
from pathlib import Path
from typing import Protocol, runtime_checkable

NATS_HEALTH_URL = "http://localhost:8222/healthz"
"""NATS's HTTP monitoring health endpoint; a 200 means the network is up."""

API_HOST_ENV = "FREEAGENT_API_HOST"
"""Environment variable naming the address ``freeagent-api`` binds; default ``127.0.0.1``."""

API_PORT_ENV = "FREEAGENT_API_PORT"
"""Environment variable naming the port ``freeagent-api`` binds; default ``8000``."""

_API_DEFAULT_HOST = "127.0.0.1"
"""The API's default bind address, mirroring ``freeagent-api``'s own default."""

_API_DEFAULT_PORT = "8000"
"""The API's default port, mirroring ``freeagent-api``'s own default."""

API_EXECUTABLE = "freeagent-api"
"""The console script the API is spawned from; resolved on ``PATH`` within the active venv."""

STATE_DIR_NAME = ".freeagent"
"""The state directory's name.

In a git repository it is created at the repo root (``<repo-root>/.freeagent/``); out of a
repository the user-level directory it names is used directly (see :func:`_state_dir`).
"""

XDG_STATE_HOME_ENV = "XDG_STATE_HOME"
"""Environment variable naming the base for user-level state.

Out of a git repository the state directory is ``$XDG_STATE_HOME/freeagent/`` when this is set, else
``~/.freeagent/`` (see :func:`_state_dir`).
"""

_USER_STATE_SUBDIR = "freeagent"
"""The application's subdirectory name under an ``$XDG_STATE_HOME`` base (which is shared across
applications, so state must be namespaced), as opposed to the dotted :data:`STATE_DIR_NAME` used
directly under ``$HOME``."""

API_PID_FILE_NAME = "api.pid"
"""The pid file recording the detached API process, under :data:`STATE_DIR_NAME`."""

API_LOG_FILE_NAME = "api.log"
"""The log file the detached API's stdout/stderr append to, under :data:`STATE_DIR_NAME`."""

_HEALTH_EXECUTABLE_FIELD = "executable"
"""The ``/health`` field naming the serving interpreter (its venv); the API's identity for adoption.

The API reports its :data:`sys.executable` here (see :class:`freeagent.api.app.HealthResponse`), and
:func:`ensure_api` adopts a healthy listener only when this equals its own interpreter — so a wrong-
venv API squatting the port is refused, not silently used.
"""

_HEALTH_PID_FIELD = "pid"
"""The ``/health`` field naming the serving process's pid, used to name an impostor in the error."""

_PACKAGE_DATA = "freeagent.sdk._platform"
"""The package holding the shipped compose file and NATS config."""

_COMPOSE_FILE_NAME = "compose.yaml"
"""The compose file's name within :data:`_PACKAGE_DATA`."""

_HEALTH_TIMEOUT_SECONDS = 30.0
"""How long to wait for a service's health endpoint after starting it before giving up."""

_HEALTH_POLL_INTERVAL_SECONDS = 0.25
"""Delay between health-endpoint polls while waiting for a service to come up."""

_STOP_TIMEOUT_SECONDS = 10.0
"""How long to wait for the API to exit after SIGTERM before escalating to SIGKILL."""

_STOP_POLL_INTERVAL_SECONDS = 0.1
"""Delay between liveness checks while waiting for the API to exit."""

_STOP_KILL_TIMEOUT_SECONDS = 5.0
"""How long to wait for the API to exit after SIGKILL before giving up on it."""

_TEARDOWN_TIMEOUT_SECONDS = 10.0
"""How long to wait for a spawned serve to exit after SIGTERM before escalating to SIGKILL."""

_TEARDOWN_KILL_TIMEOUT_SECONDS = 5.0
"""How long to wait for a spawned serve to exit after SIGKILL before giving up on it."""


class Outcome(enum.Enum):
    """Whether an ensure call had to start the service or found it already running.

    Callers (e.g. the ``start`` switch and launch sessions) use this to print ``started X`` versus
    ``X already running``.
    """

    STARTED = "started"
    ALREADY_RUNNING = "already running"
    RECONCILED = "reconciled"
    """The service was already up and a reconciling ensure re-applied its configuration — an honest
    report for :func:`ensure_nats` with ``reconcile=True``, which runs ``compose up`` even over a
    healthy NATS (saying ``started`` there would misreport a container that never went down)."""


class DockerUnavailableError(RuntimeError):
    """Raised by :func:`ensure_nats` when docker cannot be used to bring NATS up."""


class StopFailedError(RuntimeError):
    """Raised by :func:`stop_api` when a live API could not be reaped even with SIGKILL.

    Distinguishes a genuine teardown failure — the API is still running, with its port still bound —
    from the ordinary "nothing to stop" case (which :func:`stop_api` returns ``False`` for). Callers
    (the ``stop`` switch) surface this rather than misreporting the still-running API as stopped.
    """


def api_health_url() -> str:
    """Return the API health endpoint, honoring the same bind variables ``freeagent-api`` reads.

    The API binds :data:`API_HOST_ENV`/:data:`API_PORT_ENV` (defaults ``127.0.0.1:8000``), so the
    health probe must be derived from those same variables — a hardcoded URL would spawn an API on
    the configured port and then poll the wrong one. Computed at call time, not import time, so a
    test's (or caller's) environment change is honored.

    :return: The ``GET /health`` URL for the API's configured bind.
    """
    host = os.environ.get(API_HOST_ENV, _API_DEFAULT_HOST)
    port = os.environ.get(API_PORT_ENV, _API_DEFAULT_PORT)
    return f"http://{host}:{port}/health"


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


@dataclasses.dataclass(frozen=True)
class _ListenerIdentity:
    """The serving environment a listener reported at ``/health`` — the venv and pid answering.

    Only carried by a :attr:`_ListenerState.IDENTIFIED` probe: the fields a launcher compares
    against its own environment and, on a mismatch, names in the impostor error.

    :ivar executable: The serving interpreter path (its venv) the listener reported.
    :ivar pid: The serving process's pid the listener reported.
    """

    executable: str
    pid: int


class _ListenerState(enum.Enum):
    """How a probe of the API's port came back — the three cases :func:`ensure_api` must tell apart.

    Bare liveness cannot distinguish these, which is why any healthy listener used to be adopted
    (issue #117). The probe is classified instead, and only :attr:`IDENTIFIED` can mean "our API".
    """

    ABSENT = "absent"
    """Nothing answered — connection refused, a timeout, or a non-2xx status.

    The port is free to spawn our own API on.
    """

    IDENTIFIED = "identified"
    """A Free Agent API answered with its identity (interpreter path and pid).

    Whether it is *ours* is a further check on the reported interpreter; the identity travels
    alongside this state.
    """

    UNIDENTIFIED = "unidentified"
    """A process answered 2xx but not as a Free Agent API — a non-JSON body, or JSON missing the
    identity fields.

    Something is squatting the port; it is not ours to adopt, and spawning over it would only
    collide, so it is an impostor to report.
    """


def _probe_listener(url: str) -> tuple[_ListenerState, _ListenerIdentity | None]:
    """Probe a health endpoint and classify what, if anything, is answering the API's port.

    Fetches ``url`` and reads its body as :class:`freeagent.api.app.HealthResponse` JSON:

    - a connection error, timeout, or non-2xx status is :attr:`_ListenerState.ABSENT` — the port is
      free;
    - a 2xx JSON body carrying both the interpreter path and pid is
      :attr:`_ListenerState.IDENTIFIED` and its :class:`_ListenerIdentity` is returned alongside;
    - a 2xx body that is not that shape (non-JSON, or JSON missing either field) is
      :attr:`_ListenerState.UNIDENTIFIED` — a non-API process holds the port.

    :param url: The health endpoint to probe.
    :return: The classified state, paired with the reported identity when (and only when) the state
        is :attr:`_ListenerState.IDENTIFIED`.
    """
    try:
        with urllib.request.urlopen(url, timeout=2) as response:
            if not 200 <= response.status < 300:
                return _ListenerState.ABSENT, None
            payload = json.loads(response.read())
    except (urllib.error.URLError, OSError):
        return _ListenerState.ABSENT, None
    except ValueError:
        # Covers both json.JSONDecodeError (2xx but not JSON) and UnicodeDecodeError (2xx but not
        # even decodable text — a squatter serving binary content). Either way: not our API.
        return _ListenerState.UNIDENTIFIED, None

    if isinstance(payload, dict):
        executable = payload.get(_HEALTH_EXECUTABLE_FIELD)
        pid = payload.get(_HEALTH_PID_FIELD)
        if isinstance(executable, str) and isinstance(pid, int):
            return _ListenerState.IDENTIFIED, _ListenerIdentity(executable=executable, pid=pid)
    return _ListenerState.UNIDENTIFIED, None


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


def _user_state_dir() -> Path:
    """Return the user-level state directory used when not inside a git repository.

    Follows the XDG Base Directory convention: ``$XDG_STATE_HOME/freeagent`` when ``XDG_STATE_HOME``
    is set (its base is shared across applications, so the ``freeagent`` subdirectory namespaces
    our state), otherwise ``~/.freeagent``. Deliberately independent of the caller's cwd so a
    third-party ``ensure`` and a later ``stop``, from different directories, resolve one pid file.

    A relative ``XDG_STATE_HOME`` is ignored — the spec says to treat a non-absolute value as unset,
    and honoring it would resolve against the caller's cwd and reopen the very hole this closes.

    :return: The absolute user-level state directory (not created here).
    """
    xdg_state_home = os.environ.get(XDG_STATE_HOME_ENV)
    if xdg_state_home and (base := Path(xdg_state_home)).is_absolute():
        return base / _USER_STATE_SUBDIR
    return Path.home() / STATE_DIR_NAME


def _state_dir() -> Path:
    """Return the directory holding platform state (the API pid file).

    ``<repo-root>/.freeagent`` when inside a git repository — so every session in the repo, from any
    subdirectory, shares one pid file (the #98 behaviour) — otherwise the cwd-independent user-level
    directory from :func:`_user_state_dir`. Never resolves against the caller's cwd, so ``ensure``
    and ``stop`` agree wherever each is invoked (issue #116).

    :return: The state directory (the ``.freeagent`` directory itself, created on write, not here).
    """
    repo_root = _repo_root()
    if repo_root is not None:
        return repo_root / STATE_DIR_NAME
    return _user_state_dir()


def _api_pid_file() -> Path:
    """Return the path to the API pid file under the state directory.

    :return: The ``api.pid`` path inside :func:`_state_dir`.
    """
    return _state_dir() / API_PID_FILE_NAME


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


def ensure_nats(
    compose_file: str | os.PathLike[str] | None = None, reconcile: bool = False
) -> Outcome:
    """Ensure the NATS docker network is up, starting it if necessary.

    Two modes, for the two kinds of caller (see ADR-0009):

    - **Session (the default,** ``reconcile=False`` **).** Polls :data:`NATS_HEALTH_URL`; if NATS
      already answers, this is a no-op. A launch session must never disrupt a NATS other sessions
      share, and running ``compose up`` on every ``uv run <app>`` would restart NATS on any config
      change — so answering healthz is enough and docker is not touched.
    - **Platform owner (**``reconcile=True``**, passed by the** ``start`` **switch).** Runs ``docker
      compose --file <resolved> up --detach --wait`` unconditionally — even when NATS already
      answers healthz — so a container left on a stale config (e.g. after pulling a branch that
      changed the NATS config) is reconciled against the compose file. This restores the
      reconciliation the old ``start`` always did, at the one place that owns the platform.

    When docker does run it is ``docker compose --file <resolved> up --detach --wait`` against the
    resolved compose file; health is confirmed by ``--wait``.

    :param compose_file: An explicit compose file, or ``None`` to use the packaged copy; see
        :func:`resolve_compose_file`.
    :param reconcile: When True, run ``compose up`` even if NATS already answers healthz, so a
        stale container is brought in line with the compose file. The ``start`` switch passes True;
        sessions leave it False to keep the non-disruptive short-circuit.
    :return: :attr:`Outcome.ALREADY_RUNNING` if NATS was already healthy and no reconciliation
        was requested; :attr:`Outcome.RECONCILED` if it was already healthy and ``compose up``
        re-applied the configuration over it; else :attr:`Outcome.STARTED`.
    :raises DockerUnavailableError: If the ``docker`` executable is missing, or ``docker compose
        up`` fails (e.g. the daemon is not running).
    """
    was_healthy = _is_healthy(NATS_HEALTH_URL)
    if not reconcile and was_healthy:
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
    return Outcome.RECONCILED if was_healthy else Outcome.STARTED


def _same_interpreter(executable: str) -> bool:
    """Report whether a reported interpreter path is this process's own interpreter.

    A venv exposes one binary under several aliases (``python``, ``python3``, ``python3.12`` —
    symlinks to one file), and the API's console-script shebang may name a different alias than the
    one this process was launched through. Aliases of one interpreter are the same environment, so
    the comparison resolves symlinks rather than matching strings — an exact-string check would
    reject a legitimately-ours API over spelling.

    :param executable: The interpreter path a listener reported at ``/health``.
    :return: True if it names the same interpreter as :data:`sys.executable`.
    """
    if executable == sys.executable:
        return True
    try:
        return Path(executable).resolve() == Path(sys.executable).resolve()
    except OSError:
        # An unresolvable path (e.g. a symlink loop) cannot be shown to be ours.
        return False


def _reject_wrong_environment_api(identity: _ListenerIdentity, health_url: str) -> None:
    """Abort because a *different environment's* Free Agent API holds the API's port.

    Called when ``/health`` answered with an identity whose interpreter is not this process's own: a
    wrong-venv API — the classic case being one left running from a since-deleted worktree — or an
    unrelated Free Agent API from another checkout. Adopting it is the silent-adoption failure the
    incident hit: its venv has no entry point for the caller's applications, so every request 404s
    while ``start`` cheerfully reports "already running". So we refuse, naming the impostor's venv
    and pid and how to clear it, rather than returning :attr:`Outcome.ALREADY_RUNNING`.

    :param identity: The mismatched identity the listener reported at ``/health``.
    :param health_url: The health URL probed, named in the message so the operator knows the port.
    :raises SystemExit: Always, with the impostor's venv and pid and the remedy.
    """
    sys.exit(
        f"a different environment's API is already listening at {health_url}: it is running from "
        f"{identity.executable!r} (pid {identity.pid}), not this environment's "
        f"{sys.executable!r}. That API's venv has no entry points for this environment's "
        f"applications, so adopting it would 404 every request. Stop it first — e.g. "
        f"`kill {identity.pid}` — then try again."
    )


def _reject_unidentified_listener(health_url: str) -> None:
    """Abort because a process that is not a Free Agent API holds the API's port.

    Called when the port answered 2xx but without a verifiable Free Agent identity — a non-JSON
    body, or JSON missing the identity fields. It is not our API to adopt, and spawning our own over
    it would only collide on the port, so we refuse with the port to clear and how.

    :param health_url: The health URL probed, named in the message so the operator knows the port.
    :raises SystemExit: Always, telling the operator to free the port.
    """
    split = urllib.parse.urlsplit(health_url)
    port = split.port or (443 if split.scheme == "https" else 80)
    sys.exit(
        f"something that is not the Free Agent API is already listening at {health_url}: it "
        f"answers health checks but does not report a Free Agent identity. Free that port — e.g. "
        f"find the process with `lsof -nP -iTCP:{port} -sTCP:LISTEN` and stop it — then try again."
    )


def ensure_api() -> Outcome:
    """Ensure *this environment's* ``freeagent-api`` host process is up, spawning it if necessary.

    Probes :func:`api_health_url`, then decides by the identity the listener reports at ``/health``
    (see :class:`freeagent.api.app.HealthResponse`) — never by bare liveness. Any healthy listener
    would satisfy a liveness-only check, so an API from a deleted worktree's venv, another checkout,
    or an unrelated process squatting the port would be silently adopted and never replaced by one
    from the caller's own environment. Instead:

    - The listener's interpreter is this process's own (compared with symlink aliases resolved, see
      :func:`_same_interpreter`): it is our API, so this is a no-op returning
      :attr:`Outcome.ALREADY_RUNNING`.
    - A listener reports a *different* interpreter (a wrong-venv API), or answers without a
      verifiable identity (a non-API process on the port): it is not ours. We do not adopt it — we
      abort with an actionable error naming the impostor (see
      :func:`_reject_wrong_environment_api` and :func:`_reject_unidentified_listener`).
    - Nothing answers: spawn the ``freeagent-api`` console script in its own session (detached from
      this process group so it outlives the caller), record its pid in the pid file, and poll health
      until ready. The detached process's stdout/stderr append to :data:`API_LOG_FILE_NAME` beside
      the pid file — a persistent daemon must not inherit a mortal terminal's stdio.

    Health remains the source of truth: a pid file left over from a dead process is stale and gets
    overwritten. The API is app-agnostic and belongs to no session, so callers ensure but never own
    it — only the ``start``/``stop`` switch tears it down (see :func:`stop_api`).

    :return: :attr:`Outcome.ALREADY_RUNNING` if this environment's API was already healthy, else
        :attr:`Outcome.STARTED`.
    :raises SystemExit: If a wrong-environment API or a non-API process is already answering the
        port (with the impostor's venv, pid, and remedy); if the ``freeagent-api`` executable is
        missing from the environment (with a message telling the author to add ``freeagent-api`` to
        their dev dependencies); or if the API was spawned but never became healthy within the
        timeout.
    """
    health_url = api_health_url()
    state, identity = _probe_listener(health_url)
    # IDENTIFIED carries an identity and is the only state that does, so gating on the identity
    # itself both narrows the type and holds under `python -O` (an assert would not).
    if identity is not None:
        if _same_interpreter(identity.executable):
            return Outcome.ALREADY_RUNNING
        _reject_wrong_environment_api(identity, health_url)
    elif state is _ListenerState.UNIDENTIFIED:
        _reject_unidentified_listener(health_url)

    if shutil.which(API_EXECUTABLE) is None:
        sys.exit(
            f"the {API_EXECUTABLE!r} executable was not found in this environment. Add "
            f"'freeagent-api' to your dev dependencies so the platform can start the API."
        )

    pid_file = _api_pid_file()
    pid_file.parent.mkdir(parents=True, exist_ok=True)
    log_file = pid_file.parent / API_LOG_FILE_NAME

    # start_new_session detaches the child into its own session and process group, so it survives
    # this process exiting (a foreground launch session's Ctrl-C must not take the shared API down).
    # Its stdio must not stay tied to this terminal: when the terminal closes, writes to its pty
    # start failing and the "persistent" API can begin erroring — so log to a file instead.
    with log_file.open("ab") as log:
        process = subprocess.Popen(
            [API_EXECUTABLE],
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
        )

    pid_file.write_text(str(process.pid))

    if not _wait_until_healthy(health_url):
        sys.exit(
            f"the API was started (pid {process.pid}) but did not become healthy at "
            f"{health_url} within {_HEALTH_TIMEOUT_SECONDS:.0f}s. See {log_file} for its output."
        )
    return Outcome.STARTED


def _wait_for_pid_exit(pid: int, timeout: float) -> bool:
    """Poll a pid's liveness until it exits or the timeout elapses.

    :param pid: The process id to wait on.
    :param timeout: How long to keep polling, in seconds.
    :return: True if the process was gone within the timeout, False if it was still alive on expiry.
    """
    deadline = time.monotonic() + timeout
    while _process_alive(pid):
        if time.monotonic() >= deadline:
            return False
        time.sleep(_STOP_POLL_INTERVAL_SECONDS)
    return True


def stop_api() -> bool:
    """Stop the API this platform started, if any, and remove its pid file.

    Health — not the pid file — is the source of truth (see the module docstring), so the pid file
    is trusted only when the API actually answers :func:`api_health_url`. When it does, the recorded
    pid is SIGTERMed and waited on; otherwise the file is stale — the API is gone even if the OS has
    recycled its pid onto some innocent process — so it is simply cleared and "nothing to do"
    reported, never signalled.

    Signalling is itself guarded: a pid recycled between the health check and the kill can no longer
    be ours, so a :class:`ProcessLookupError` (the pid is gone) or :class:`PermissionError` (the pid
    now belongs to another user) is caught, the stale file cleared, and "nothing to do" reported —
    no raw traceback, and the file does not persist to fail every later ``stop`` the same way.

    If the API has not exited within :data:`_STOP_TIMEOUT_SECONDS`, teardown escalates to SIGKILL
    and waits again — a SIGTERM-ignoring API (or one wedged mid-request) must not be left orphaned
    with port 8000 still bound, where a later ``start`` would see the zombie answering ``/health``
    and refuse to restart. The pid file is only unlinked once the process is confirmed gone; if even
    SIGKILL does not reap it within :data:`_STOP_KILL_TIMEOUT_SECONDS`, the pid file is kept and
    :class:`StopFailedError` is raised so the still-running API is reported as a failure, distinct
    from the "nothing to do" case and never silently erased.

    A silent no-op when nothing is recorded, so ``stop`` is safe to call unconditionally.

    :return: True if a live API was signalled and confirmed stopped, False if there was nothing to
        do (no pid recorded, the API was not answering health, or its pid was recycled).
    :raises StopFailedError: If a live API could not be reaped even with SIGKILL; the pid file is
        kept so the still-running process is not forgotten.
    """
    pid_file = _api_pid_file()
    pid = _read_pid(pid_file)

    if pid is None or not _is_healthy(api_health_url()):
        pid_file.unlink(missing_ok=True)
        return False

    try:
        os.kill(pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        # The pid the health-confirmed API was recorded under is no longer a process we can signal:
        # recycled onto a dead pid or another user's process. Treat the file as stale, not fatal.
        pid_file.unlink(missing_ok=True)
        return False

    if _wait_for_pid_exit(pid, _STOP_TIMEOUT_SECONDS):
        pid_file.unlink(missing_ok=True)
        return True

    # SIGTERM was ignored (or the process is wedged): escalate rather than orphan a live API. The
    # same recycled-pid guard applies — the process may have exited (and its pid been reused)
    # between the SIGTERM wait expiring and this kill.
    try:
        os.kill(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pid_file.unlink(missing_ok=True)
        return True
    if not _wait_for_pid_exit(pid, _STOP_KILL_TIMEOUT_SECONDS):
        # Even SIGKILL did not reap it in time (e.g. stuck in uninterruptible I/O). Keep the pid
        # file so the still-running API is not silently forgotten, and report the failure loudly
        # rather than returning a False that a caller would misread as "nothing was running".
        raise StopFailedError(
            f"the API (pid {pid}) did not exit even after SIGKILL; its pid file has been left in "
            f"place. The process may be stuck in uninterruptible I/O — check {pid_file}."
        )

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
    own console script whose ``main()`` hands a launcher to :func:`run`, and a generic ``launch
    <name>`` consumer that would need an enumerable group does not exist yet (ADR-0009).

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
    :return: Zero if every prepare command succeeded, else the exit code of the first that failed.
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


def _terminate_one(service: Service, process: subprocess.Popen[bytes]) -> int:
    """Terminate a single spawned serve with a bounded wait, escalating to SIGKILL on expiry.

    A still-running child is SIGTERMed and given :data:`_TEARDOWN_TIMEOUT_SECONDS` to exit; a child
    that had already exited on its own (crashed, or killed by the terminal's group SIGINT on Ctrl-C)
    is reaped in place with no signal. A serve that ignores SIGTERM — a common failing of shell or
    ``npm`` wrappers that do not forward signals — would otherwise block teardown forever on an
    unbounded ``wait()``; instead it is escalated to SIGKILL and reaped, so teardown always
    completes and no child leaks with its port still bound.

    A :class:`KeyboardInterrupt` (a Ctrl-C landing in one of the waits) SIGKILLs the child before it
    is allowed to unwind, so the very process being reaped when the interrupt arrives is never left
    running — the interrupt then propagates for the caller to keep tearing the rest down.

    :param service: The service owning the process, for the diagnostic message on a real failure.
    :param process: The spawned serve to terminate and reap.
    :return: The process's exit code (a POSIX signal death reports ``-signum``); ``-SIGKILL`` if it
        had to be force-killed after ignoring SIGTERM, even when the force-kill could not be reaped
        in time (teardown never blocks on an unreapable child).
    :raises KeyboardInterrupt: If a Ctrl-C lands during a wait; re-raised only after the child has
        been SIGKILLed so it cannot leak.
    """
    if process.poll() is None:
        process.terminate()
    try:
        return process.wait(timeout=_TEARDOWN_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        # The serve ignored SIGTERM. Force it down so teardown cannot hang and no child can leak.
        print(
            f"service {service.name!r} did not exit after SIGTERM; escalating to SIGKILL.",
            file=sys.stderr,
        )
    except KeyboardInterrupt:
        # A second Ctrl-C landed while waiting on this child's SIGTERM. Force-kill it before letting
        # the interrupt unwind, so the child being reaped is not left running with its port bound.
        process.kill()
        raise
    process.kill()
    try:
        return process.wait(timeout=_TEARDOWN_KILL_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        # Even SIGKILL has not been reaped yet (e.g. the child is stuck in uninterruptible I/O). Do
        # not let the wait hang teardown or escape as a traceback: report it and treat the child as
        # force-killed. The kernel will reap it once the syscall returns.
        print(
            f"service {service.name!r} did not exit even after SIGKILL; abandoning the wait.",
            file=sys.stderr,
        )
        return -signal.SIGKILL


def _terminate_in_reverse(
    spawned: Sequence[tuple[Service, subprocess.Popen[bytes]]],
) -> int:
    """SIGTERM the spawned serve processes in reverse spawn order and collect their exit codes.

    Reverse order tears the stack down in the opposite order it came up — the last thing started is
    the first stopped. Each child is terminated with a bounded wait and SIGKILL escalation (see
    :func:`_terminate_one`), so a serve that ignores SIGTERM cannot hang teardown or leak. Death by
    the teardown signals SIGTERM/SIGINT is a clean exit, not a failure (see :func:`_is_clean_exit`);
    the returned code is the first genuine failure — a positive nonzero exit or death by another
    signal, including the ``-SIGKILL`` of a force-killed child — so a crashed or wedged serve, but
    not a cleanly-terminated one, makes the session exit nonzero.

    A :class:`KeyboardInterrupt` mid-teardown — a user's second Ctrl-C while children are still
    being reaped — must not abandon the remaining children: it is caught so the loop keeps
    terminating them, then re-raised for :func:`run` to turn into the conventional interrupt exit
    code. Without this, the interrupt would unwind out through the earlier, not-yet-terminated
    services, leaking them with their ports still bound.

    :param spawned: The service/process pairs to terminate, in spawn order.
    :return: Zero if every child exited cleanly, else the first failing exit code seen, in reverse
        spawn order.
    :raises KeyboardInterrupt: If a Ctrl-C lands mid-teardown; re-raised only after every remaining
        child has been terminated.
    """
    failure = 0
    interrupt: KeyboardInterrupt | None = None
    for service, process in reversed(spawned):
        try:
            returncode = _terminate_one(service, process)
        except KeyboardInterrupt as caught:
            # A second Ctrl-C during teardown: remember it, but keep terminating the rest before
            # letting it unwind, so no earlier-spawned child is left running.
            interrupt = caught
            continue
        if not _is_clean_exit(returncode) and failure == 0:
            failure = returncode
            print(
                f"service {service.name!r} exited with code {returncode}",
                file=sys.stderr,
            )
    if interrupt is not None:
        raise interrupt
    return failure


def _block_until_interrupt_or_exit(
    spawned: Sequence[tuple[Service, subprocess.Popen[bytes]]],
) -> None:
    """Block the session until SIGINT (Ctrl-C) or until any spawned serve exits on its own.

    Polls the spawned processes between sleeps: a serve that dies — a crash, a port collision at
    startup, an external kill — ends the wait so :func:`run` can tear the rest down and surface the
    failure immediately, instead of claiming the stack is up while nothing is serving.
    :class:`KeyboardInterrupt` (raised by the default SIGINT handler) unwinds to :func:`run`, which
    tears the session down the same way. Factored out so tests can drive the interrupt without a
    real signal.

    :param spawned: The service/process pairs whose continued liveness keeps the session blocked.
    """
    while all(process.poll() is None for _, process in spawned):
        time.sleep(_SESSION_POLL_INTERVAL_SECONDS)


_SESSION_POLL_INTERVAL_SECONDS = 1.0
"""How long each idle sleep in the session's block-until-interrupt loop lasts, in seconds."""

_INTERRUPT_EXIT_CODE = 130
"""The exit code for a session interrupted outside its blocking loop: 128 + SIGINT, by shell
convention."""


def run(launcher: Launcher) -> int:
    """Run an application's serving stack as a foreground session, until Ctrl-C.

    The whole session lifecycle for ``uv run <app>``:

    1. Ensure the platform is up — NATS then the API (:func:`ensure_nats`, :func:`ensure_api`).
       This *ensures* but never owns the platform; a session's teardown leaves it running.
    2. Run every service's ``prepare`` steps to completion, in order, failing fast: the first
       nonzero exit aborts the session with that code, before any ``serve`` is spawned.
    3. Spawn every service's ``serve`` process, then print the stack's URLs.
    4. Block until SIGINT (Ctrl-C) — or until any ``serve`` exits on its own, so a crashed server
       (e.g. its port was already taken) ends the session immediately instead of leaving it
       claiming the stack is up.
    5. SIGTERM the spawned ``serve`` processes in reverse order and wait for them, escalating to
       SIGKILL any that ignore SIGTERM so teardown always completes and no child leaks. The exit
       code is nonzero if any child failed.

    Only what this session spawned is torn down. The platform (NATS, the API) is untouched by the
    teardown — the ``stop`` switch owns it — so a second concurrent session keeps working.

    A Ctrl-C landing *outside* the blocking loop — during the platform ensure, a long ``prepare``
    build, the spawn window, or a second Ctrl-C during teardown itself — is also a clean end of the
    session, never a traceback: anything already spawned is torn down and the conventional interrupt
    code ``130`` is returned.

    :param launcher: The application's :class:`Launcher`, supplying its :class:`Service` list.
    :return: A process exit code: the failing prepare step's code if one failed, otherwise the
        first failing ``serve`` exit code seen during teardown, otherwise ``130`` for a Ctrl-C
        outside the blocking loop, otherwise ``0``.
    """
    try:
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
                _block_until_interrupt_or_exit(spawned)
            except KeyboardInterrupt:
                pass
        finally:
            # Teardown in a finally so anything spawned is terminated even if we never reach the
            # block (e.g. a KeyboardInterrupt lands between spawn and the loop). The mid-spawn
            # leak — a Popen raising partway through _spawn_serves — is handled inside
            # _spawn_serves itself.
            exit_code = _terminate_in_reverse(spawned)

        return exit_code
    except KeyboardInterrupt:
        # Ctrl-C before the blocking loop (ensure, prepare, spawn, URL print), or a *second* Ctrl-C
        # during teardown: either way the session ends cleanly — teardown of anything spawned
        # already ran via the finally above (a mid-teardown interrupt is caught inside
        # _terminate_in_reverse, which finishes reaping the children before re-raising) — with the
        # conventional interrupt exit code instead of a traceback.
        return _INTERRUPT_EXIT_CODE
