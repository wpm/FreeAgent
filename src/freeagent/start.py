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
    this repo's compose file) then :func:`~freeagent.sdk.launch.ensure_api`. Both are idempotent, so
    a second ``start`` while everything is up is a no-op that reports each as already running.

    Exits nonzero if docker is unavailable, using the launch module's guidance message.
    """
    try:
        nats_outcome = launch.ensure_nats(COMPOSE_FILE)
    except launch.DockerUnavailableError as error:
        sys.exit(str(error))
    print(f"NATS network {nats_outcome.value}.")

    api_outcome = launch.ensure_api()
    print(f"freeagent-api {api_outcome.value}.")


def stop() -> None:
    """Tear down the Free Agent platform: stop the ``freeagent-api`` process and the NATS network.

    Delegates the API teardown to :func:`freeagent.sdk.launch.stop_api` (which SIGTERMs the pid this
    switch recorded and clears the pid file), then runs ``docker compose down`` on this repo's
    compose file. Both halves are safe from any state — fully up, half up, or already down — so
    ``stop`` always leaves the platform down. Exits with docker's return code.
    """
    if launch.stop_api():
        print("freeagent-api stopped.")
    else:
        print("freeagent-api was not running.")

    returncode = subprocess.run(
        ["docker", "compose", "--file", str(COMPOSE_FILE), "down"]
    ).returncode
    if returncode == 0:
        print("NATS network is down.")
    sys.exit(returncode)


def reformat() -> None:
    """Format and lint-fix the repo: docformatter, then ruff format, then ruff check --fix.

    Runs docformatter recursively over the source trees only (not the repo root), since
    docformatter's own dir walk doesn't prune hidden/cache directories and aborts the entire walk
    the moment it meets one, rather than just skipping it. Exits with the first nonzero return code,
    if any, after all three tools have run.
    """
    source_dirs = [
        str(REPO_ROOT / "src"),
        str(REPO_ROOT / "packages"),
        str(REPO_ROOT / "apps"),
    ]
    docformatter_code = subprocess.run(
        [
            "docformatter",
            "--in-place",
            "--recursive",
            "--wrap-summaries=100",
            "--wrap-descriptions=100",
            *source_dirs,
            "--exclude",
            ".mypy_cache",
            ".ruff_cache",
            ".pytest_cache",
            "__pycache__",
        ]
    ).returncode
    # docformatter exits 3 when it rewrites files; that's not a failure here.
    docformatter_code = 0 if docformatter_code == 3 else docformatter_code

    ruff_format_code = subprocess.run(["ruff", "format", *source_dirs]).returncode
    ruff_check_code = subprocess.run(["ruff", "check", "--fix", *source_dirs]).returncode

    sys.exit(docformatter_code or ruff_format_code or ruff_check_code)


if __name__ == "__main__":
    start()
