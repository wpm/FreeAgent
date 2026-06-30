"""On/off switch for the Free Agent docker network.

Launches and tears down every backing service that Free Agent needs to run. Resolves the compose
file relative to the repository root so the commands work regardless of the current working
directory.
"""

import subprocess
import sys
from pathlib import Path

# This file lives at <repo>/src/freeagent/start.py, so the repo root is three
# levels up.
REPO_ROOT = Path(__file__).resolve().parents[2]
COMPOSE_FILE = REPO_ROOT / "docker" / "compose.yaml"


def _compose(*args: str) -> int:
    """Run a `docker compose` subcommand against the Free Agent compose file.

    Returns docker's exit code. Exits early if the compose file is missing.
    """
    if not COMPOSE_FILE.is_file():
        sys.exit(f"compose file not found at {COMPOSE_FILE}")
    return subprocess.run(["docker", "compose", "--file", str(COMPOSE_FILE), *args]).returncode


def start() -> None:
    """Bring up the Free Agent docker network and wait for it to be healthy.

    Runs `docker compose up` detached, waiting until every service with a healthcheck reports
    healthy. Exits with docker's return code so failures propagate to the caller.
    """
    returncode = _compose("up", "--detach", "--wait")
    if returncode == 0:
        print("Free Agent network is up.")
    sys.exit(returncode)


def stop() -> None:
    """Tear down the Free Agent docker network.

    Runs `docker compose down`, stopping and removing the containers and network (named volumes are
    preserved). Exits with docker's return code.
    """
    returncode = _compose("down")
    if returncode == 0:
        print("Free Agent network is down.")
    sys.exit(returncode)


def reformat() -> None:
    """Reformat docstrings across the repo with docformatter.

    Runs docformatter recursively over the source trees only (not the repo root), since
    docformatter's own dir walk doesn't prune hidden/cache directories and aborts the entire walk
    the moment it meets one, rather than just skipping it.
    """
    returncode = subprocess.run(
        [
            "docformatter",
            "--in-place",
            "--recursive",
            "--wrap-summaries=100",
            "--wrap-descriptions=100",
            str(REPO_ROOT / "src"),
            str(REPO_ROOT / "packages"),
            "--exclude",
            ".mypy_cache",
            ".ruff_cache",
            ".pytest_cache",
            "__pycache__",
        ]
    ).returncode
    sys.exit(returncode)


if __name__ == "__main__":
    start()
