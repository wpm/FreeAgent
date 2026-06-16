"""The ``free-agent serve`` command: run the control service on localhost.

A **top-level** command on the root ``free-agent`` CLI, a sibling of ``replay``
and of the per-application sub-commands: the control service is app-agnostic
library infrastructure (it launches *any* installed app by its REST name), so it
belongs beside ``replay``, not under one application.

Start is ``free-agent serve``; stop is Ctrl-C. The service binds to loopback with
no auth -- consistent with the local NATS testbed (``SECURITY.md``) -- and serves
the REST API only (the browser viewer is served by its own dev server and calls
in cross-origin; CORS is configured for it).
"""

from __future__ import annotations

from typing import Annotated

import typer

from .config import default_nats_url


def serve(
    host: Annotated[
        str,
        typer.Option("--host", help="interface to bind (loopback by default; no auth)."),
    ] = "127.0.0.1",
    port: Annotated[
        int,
        typer.Option("--port", help="port to listen on."),
    ] = 8000,
    nats_url: Annotated[
        str | None,
        typer.Option(
            "--nats-url",
            help="NATS server URL the launched episodes use; "
            "defaults to $FREEAGENT_NATS_URL or nats://localhost:4222.",
        ),
    ] = None,
) -> None:
    """Run the FreeAgent control service: a REST API over running episodes."""
    # Imported lazily so the rest of the CLI need not load the web stack.
    from freeagent.service import run

    url = nats_url if nats_url is not None else default_nats_url()
    try:
        run(host=host, port=port, nats_url=url)
    except KeyboardInterrupt:
        typer.echo("free-agent: control service stopped", err=True)
        raise typer.Exit(0) from None
