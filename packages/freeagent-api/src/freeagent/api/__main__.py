"""Entry point for running the Free Agent API server."""

from __future__ import annotations

import os

import uvicorn

HOST_ENV = "FREEAGENT_API_HOST"
"""Environment variable naming the address the server binds; default ``127.0.0.1``."""

PORT_ENV = "FREEAGENT_API_PORT"
"""Environment variable naming the port the server binds; default ``8000``."""

RELOAD_ENV = "FREEAGENT_API_RELOAD"
"""Environment variable enabling uvicorn's auto-reload when set to ``1``; off by default.

Reload is a development convenience with a real cost: uvicorn runs a supervisor process that
watches the source tree, and any file change restarts the server — dropping the in-memory
:class:`~freeagent.api.episodes.EpisodeManager` state mid-episode and orphaning worker
subprocesses. It is therefore opt-in rather than the default.
"""


def main() -> None:
    """Run the API server with uvicorn.

    The bind address and dev-mode auto-reload come from :data:`HOST_ENV`, :data:`PORT_ENV`, and
    :data:`RELOAD_ENV`, defaulting to a plain server on ``127.0.0.1:8000`` with no reload.
    """
    uvicorn.run(
        "freeagent.api.app:app",
        host=os.environ.get(HOST_ENV, "127.0.0.1"),
        port=int(os.environ.get(PORT_ENV, "8000")),
        reload=os.environ.get(RELOAD_ENV) == "1",
    )


if __name__ == "__main__":
    main()
