"""Serve the control service with uvicorn, bound to localhost.

The runtime front door: build the app over a NATS URL and run it under uvicorn
on the loopback interface (no auth, consistent with the local testbed). Kept
apart from :mod:`freeagent.service.app` so importing the app factory -- as the
tests do -- never pulls in the server.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import uvicorn

from .app import create_app

if TYPE_CHECKING:
    from collections.abc import Sequence

#: Bind to loopback only: the service is unauthenticated and meant for the
#: local testbed, never a public interface.
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8000


def run(
    *,
    host: str = DEFAULT_HOST,
    port: int = DEFAULT_PORT,
    nats_url: str | None = None,
    allowed_origins: Sequence[str] | None = None,
    log_level: str | None = None,
) -> None:
    """Build and serve the control service (blocks until interrupted)."""
    app = create_app(nats_url=nats_url, allowed_origins=allowed_origins)
    uvicorn.run(app, host=host, port=port, log_level=(log_level or "info").lower())


if __name__ == "__main__":
    run()
