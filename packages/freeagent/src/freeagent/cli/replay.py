"""The ``free-agent replay`` command: re-publish a recorded episode onto NATS.

This is a **top-level** command on the root ``free-agent`` CLI, a sibling of
the per-application sub-commands, because the Parquet log is uniform across
every application -- one replay tool replays any app's episode (ADR-0001). It
reads an episode log and re-publishes its messages, in ``stream_seq`` order
with original or scaled timing, onto a NATS server intended to be a separate,
local one so replay never mixes with live traffic.

Start is ``free-agent replay LOG``; stop is Ctrl-C. Pause and seek live on
:class:`~freeagent.replayer.Replayer` for an embedding GUI to drive and are not
wired to interactive CLI input in v1.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Annotated

import typer

from freeagent.replayer import ReplayerError, replay_episode
from freeagent.transport import TransportError

from .config import default_nats_url


def _validate_speed(value: float) -> float:
    """Typer callback: reject a non-positive speed as a usage error (exit 2)."""
    if value <= 0:
        raise typer.BadParameter("must be greater than 0")
    return value


def replay(
    parquet_log: Annotated[
        Path,
        typer.Argument(
            exists=True,
            dir_okay=False,
            readable=True,
            help="episode Parquet log to replay",
        ),
    ],
    nats_url: Annotated[
        str | None,
        typer.Option(
            "--nats-url",
            help="target NATS server URL (intended to be a separate, local nats-server); "
            "defaults to $FREEAGENT_NATS_URL or nats://localhost:4222.",
        ),
    ] = None,
    speed: Annotated[
        float,
        typer.Option(
            "--speed",
            callback=_validate_speed,
            help="playback speed multiplier (2.0 is twice as fast); ignored with "
            "--as-fast-as-possible.",
        ),
    ] = 1.0,
    as_fast_as_possible: Annotated[
        bool,
        typer.Option(
            "--as-fast-as-possible",
            help="ignore recorded timing and publish messages back-to-back.",
        ),
    ] = False,
) -> None:
    """Replay a recorded episode by re-publishing its messages onto NATS.

    Messages are published on their original subjects in ``stream_seq`` order.
    Timing is preserved by default and scaled by ``--speed``; pass
    ``--as-fast-as-possible`` to ignore timing entirely.
    """
    url = nats_url if nats_url is not None else default_nats_url()
    try:
        count = asyncio.run(
            replay_episode(
                parquet_path=parquet_log,
                nats_url=url,
                speed=speed,
                as_fast_as_possible=as_fast_as_possible,
            )
        )
    except (ReplayerError, TransportError) as exc:
        typer.echo(f"free-agent: error: {exc}", err=True)
        raise typer.Exit(1) from exc
    except KeyboardInterrupt:
        typer.echo("free-agent: replay interrupted", err=True)
        raise typer.Exit(130) from None
    typer.echo(f"free-agent: replayed {count} message(s) from {parquet_log} to {url}")
