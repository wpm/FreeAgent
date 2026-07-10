"""On/off switch for the Free Agent platform: the NATS docker network and the ``freeagent-api``.

The platform is app-agnostic — one NATS network and one API serve every installed application — so
this switch owns both (see ADR-0009). It keeps no orchestration logic of its own: bringing services
up is delegated to :mod:`freeagent.sdk.launch`, the shared platform code that every app-launch
session also calls, so a session and the switch share exactly one code path (and one API).

``start`` ensures NATS and the API are up, reporting what it started versus what was already
running. ``stop`` stops the API this switch owns and takes the NATS network down. The in-repo
``docker/compose.yaml`` is the source of truth for this repo; the SDK's packaged copy is the out-of-
repo fallback only.
"""

import shutil
import subprocess
import sys
from pathlib import Path

from freeagent.sdk import launch

# This file lives at <repo>/src/freeagent/start.py, so the repo root is three
# levels up.
REPO_ROOT = Path(__file__).resolve().parents[2]
COMPOSE_FILE = REPO_ROOT / "docker" / "compose.yaml"


def start() -> None:
    """Bring up the Free Agent platform: the NATS docker network and the ``freeagent-api`` process.

    Delegates to :mod:`freeagent.sdk.launch`: :func:`~freeagent.sdk.launch.ensure_nats` (against
    this repo's compose file) then :func:`~freeagent.sdk.launch.ensure_api`. ``ensure_api`` is
    idempotent, so a second ``start`` while the API is up reports it already running.

    As the platform owner, ``start`` *reconciles* NATS: it passes ``reconcile=True`` so
    ``ensure_nats`` runs ``docker compose up --wait`` even when NATS already answers healthz,
    bringing a container left on a stale config back in line with the compose file (launch sessions,
    which must not disrupt a shared NATS, keep the non-disruptive short-circuit). After changing the
    platform config, run ``stop`` then ``start`` to apply it.

    Exits nonzero if docker is unavailable, using the launch module's guidance message.
    """
    try:
        nats_outcome = launch.ensure_nats(COMPOSE_FILE, reconcile=True)
    except launch.DockerUnavailableError as error:
        sys.exit(str(error))
    print(f"NATS network {nats_outcome.value}.")

    api_outcome = launch.ensure_api()
    print(f"freeagent-api {api_outcome.value}.")


def stop() -> None:
    """Tear down the Free Agent platform: stop the ``freeagent-api`` process and the NATS network.

    Delegates the API teardown to :func:`freeagent.sdk.launch.stop_api` (which SIGTERMs the pid this
    switch recorded, escalates to SIGKILL if needed, and clears the pid file), then runs ``docker
    compose down`` on this repo's compose file. Both halves are safe from any state — fully up, half
    up, or already down — so ``stop`` always leaves the platform down. If the API could not be
    reaped even with SIGKILL, exits nonzero with the failure rather than falsely reporting it
    stopped and taking NATS down under a still-running API. Exits with docker's return code, or with
    a clean guidance message (never a raw traceback) if docker is not installed.
    """
    try:
        stopped = launch.stop_api()
    except launch.StopFailedError as error:
        sys.exit(str(error))
    if stopped:
        print("freeagent-api stopped.")
    else:
        print("freeagent-api was not running.")

    if shutil.which("docker") is None:
        sys.exit(
            "docker is required to take the NATS network down but was not found on PATH. The API "
            "has been stopped; install Docker to bring the network down too."
        )

    returncode = subprocess.run(
        ["docker", "compose", "--file", str(COMPOSE_FILE), "down"]
    ).returncode
    if returncode == 0:
        print("NATS network is down.")
    sys.exit(returncode)


def reformat() -> None:
    """Format and lint-fix the repo: docformatter, then ruff format, then ruff check --fix.

    Every tool gets an explicit list of the tracked Python files, enumerated with ``git ls-files``,
    rather than a directory to walk. docformatter's own recursive walk silently drops most of the
    tree the moment it meets a cache directory (``.mypy_cache`` and friends), so on any checkout
    that had run the tools it formatted almost nothing while looking green — see issue #120. A git-
    enumerated list makes coverage identical on every checkout, fresh or littered.

    Exits with the first nonzero return code, if any, after all three tools have run.
    """
    # -z: NUL-separated output is never C-quoted, so non-ASCII tracked paths survive verbatim.
    # Only stdout is captured; stderr flows through so a failing git explains itself.
    ls_files = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "ls-files", "-z", "*.py"],
        stdout=subprocess.PIPE,
        text=True,
    )
    if ls_files.returncode != 0:
        sys.exit(ls_files.returncode)
    python_files = [str(REPO_ROOT / name) for name in ls_files.stdout.split("\0") if name]
    if not python_files:
        sys.exit(0)

    docformatter_code = subprocess.run(
        [
            "docformatter",
            "--in-place",
            "--wrap-summaries=100",
            "--wrap-descriptions=100",
            *python_files,
        ]
    ).returncode
    # docformatter exits 3 when it rewrites files; that's not a failure here.
    docformatter_code = 0 if docformatter_code == 3 else docformatter_code

    ruff_format_code = subprocess.run(["ruff", "format", *python_files]).returncode
    ruff_check_code = subprocess.run(["ruff", "check", "--fix", *python_files]).returncode

    sys.exit(docformatter_code or ruff_format_code or ruff_check_code)


if __name__ == "__main__":
    start()
