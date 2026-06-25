"""The ``free-agent serve`` command: run the episode service.

A **top-level** command on the root ``free-agent`` CLI, a sibling of ``replay``
and of the per-application sub-commands: the episode service is app-agnostic
library infrastructure (it launches *any* installed app by its REST name), so it
belongs beside ``replay``, not under one application.

Start is ``free-agent serve``; stop is Ctrl-C. The service binds to loopback with
no auth -- consistent with the local NATS testbed (``SECURITY.md``). Pass
``--ui-dir`` (or set ``$FREEAGENT_UI_DIR``) to also serve a built UI bundle from
this origin -- one process to the browser, as in the Docker network (ADR-0003).
For that network it binds ``--host 0.0.0.0`` so the one published port is
reachable from the host.
"""

from __future__ import annotations

import os
from typing import Annotated

import typer

from .config import default_nats_url

#: Environment variable naming the built UI bundle directory to serve.
UI_DIR_ENV_VAR = "FREEAGENT_UI_DIR"


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
    ui_dir: Annotated[
        str | None,
        typer.Option(
            "--ui-dir",
            help="serve a built UI bundle from this directory (one origin); "
            "defaults to $FREEAGENT_UI_DIR.",
        ),
    ] = None,
) -> None:
    """Run the FreeAgent episode service: a REST API (+ optional UI) over episodes."""
    # Imported lazily so the rest of the CLI need not load the web stack.
    from freeagent.service import run

    url = nats_url if nats_url is not None else default_nats_url()
    bundle = ui_dir if ui_dir is not None else os.environ.get(UI_DIR_ENV_VAR)
    try:
        run(host=host, port=port, nats_url=url, ui_dir=bundle)
    except KeyboardInterrupt:
        typer.echo("free-agent: episode service stopped", err=True)
        raise typer.Exit(0) from None
