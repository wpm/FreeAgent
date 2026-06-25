"""The ``free-agent serve`` command: run the episode service.

A **top-level** command on the root ``free-agent`` CLI, a sibling of ``replay``
and of the per-application sub-commands: the episode service is app-agnostic
library infrastructure (it launches *any* installed app by its REST name), so it
belongs beside ``replay``, not under one application.

Start is ``free-agent serve``; stop is Ctrl-C. The service binds to loopback with
no auth -- consistent with the local NATS testbed (``SECURITY.md``). It is an
app-independent REST/JetStream API and serves no UI (ADR-0004): a UI is a separate
process that calls it cross-origin. In the Docker network it binds
``--host 0.0.0.0`` so the one published port is reachable from the host.
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
    """Run the FreeAgent episode service: an app-independent REST API over episodes."""
    # Imported lazily so the rest of the CLI need not load the web stack.
    from freeagent.service import run

    url = nats_url if nats_url is not None else default_nats_url()
    try:
        run(host=host, port=port, nats_url=url)
    except KeyboardInterrupt:
        typer.echo("free-agent: episode service stopped", err=True)
        raise typer.Exit(0) from None
