"""The ``collatz`` console script: launch Collatz's viewer as a foreground session.

``uv run collatz`` resolves this module's :func:`main`, which hands a :class:`CollatzLauncher` to
the SDK's shared session harness (:func:`freeagent.sdk.run`). The harness ensures the platform is up
(NATS and the API), runs the launcher's ``prepare`` steps, foregrounds its ``serve`` process, prints
the viewer URL, and on Ctrl-C tears down only what it spawned — the platform survives for the next
session (see ADR-0009).

Collatz owns only the *description* of its serving stack, never any orchestration: a single
:class:`~freeagent.sdk.Service` for the viewer, whose ``prepare`` builds the TypeScript (installing
node modules first only when they are missing) and whose ``serve`` is a stdlib static file server
over the built viewer directory. Everything else — spawning, teardown, platform ensure — belongs to
the harness.

The viewer directory is located relative to this package file, which is correct for an in-repo
checkout. A published wheel would instead ship the built viewer assets as package data; that is a
documentation concern for the epic's docs issue, not machinery here (ADR-0009).
"""

from __future__ import annotations

import sys
from pathlib import Path

from freeagent.sdk import Launcher, Service, run
from freeagent.sdk.launch import DockerUnavailableError

VIEWER_URL = "http://localhost:8080"
"""The address the static viewer server binds, printed once the stack is up."""

_VIEWER_PORT = "8080"
"""The port the viewer's static server listens on; matches :data:`VIEWER_URL`."""

# This file lives at <app>/src/freeagent/app/collatz/launch.py, so the app root — holding the
# `viewer/` directory alongside `src/` — is five levels up (past `collatz`, `app`, `freeagent`,
# `src`, into the app root).
_VIEWER_DIR = Path(__file__).resolve().parents[4] / "viewer"
"""The Collatz viewer directory, located relative to this file for in-repo launches."""

_NODE_MODULES_DIR_NAME = "node_modules"
"""The directory whose absence means ``npm install`` has not run in the viewer yet."""


class CollatzLauncher:
    """The Collatz application's :class:`~freeagent.sdk.Launcher`: describes its viewer service.

    Supplies the harness with one :class:`~freeagent.sdk.Service`, the viewer. Its ``prepare`` runs
    ``npm install`` only when ``node_modules`` is missing (so a warm checkout skips the slow
    install) and then always ``npm run build`` (so the served ``dist/`` reflects the current
    sources); its ``serve`` is a stdlib static file server on :data:`VIEWER_URL`, with the viewer
    directory as its working directory.

    :ivar name: The application's name, ``"collatz"``, used in the harness's console output.
    """

    name: str = "collatz"

    def services(self) -> list[Service]:
        """Return the Collatz serving stack: a single viewer service.

        ``prepare`` builds the viewer — ``npm install`` first only when ``node_modules`` is absent,
        then always ``npm run build``. ``serve`` serves the viewer directory statically with the
        stdlib HTTP server on the viewer's port; its ``cwd`` is the viewer directory so both the
        build and the server run there. ``url`` is :data:`VIEWER_URL`, printed once the stack is up.

        :return: A one-element list holding the viewer :class:`~freeagent.sdk.Service`.
        """
        prepare: list[list[str]] = []
        if not (_VIEWER_DIR / _NODE_MODULES_DIR_NAME).exists():
            prepare.append(["npm", "install"])
        prepare.append(["npm", "run", "build"])

        return [
            Service(
                name="viewer",
                serve=[sys.executable, "-m", "http.server", _VIEWER_PORT],
                prepare=prepare,
                cwd=_VIEWER_DIR,
                url=VIEWER_URL,
            )
        ]


def main() -> None:
    """Run the Collatz viewer session via the SDK harness; the ``collatz`` console script's entry.

    Hands a :class:`CollatzLauncher` to :func:`freeagent.sdk.run` and exits with the session's exit
    code. A :class:`~freeagent.sdk.DockerUnavailableError` from bringing the platform up is rendered
    as a clean one-line message rather than a traceback.
    """
    launcher: Launcher = CollatzLauncher()
    try:
        sys.exit(run(launcher))
    except DockerUnavailableError as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
