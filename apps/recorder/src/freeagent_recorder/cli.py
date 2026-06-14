"""Command-line entry point for the FreeAgent episode recorder.

Usage::

    freeagent-recorder --nats-url <url> --app <app> --episode-id <id> \\
        --output <path> [--idle-timeout <secs>] [--log-level <level>]

Exits 0 on success (file written), nonzero with a message on stderr on
failure (e.g. the episode's stream not found after a reasonable wait).
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Annotated

import typer

from freeagent import DEFAULT_LOG_LEVEL, LOG_LEVEL_ENV_VAR, configure_logging
from freeagent_recorder.recorder import DEFAULT_IDLE_TIMEOUT, RecorderError, record_episode

if TYPE_CHECKING:
    from collections.abc import Sequence

app = typer.Typer(
    name="freeagent-recorder",
    help="Drain one FreeAgent episode's JetStream stream to a Parquet file.",
    add_completion=False,
)


# A callback (not a command) so the options live at the top level -- the runner
# launches us as ``freeagent-recorder --nats-url ...`` with no subcommand name.
@app.callback(invoke_without_command=True)
def record(
    nats_url: Annotated[str, typer.Option(help="NATS server URL")],
    app_name: Annotated[str, typer.Option("--app", help="application name (subject prefix)")],
    episode_id: Annotated[str, typer.Option(help="episode id to record")],
    output: Annotated[Path, typer.Option(help="Parquet output path")],
    idle_timeout: Annotated[
        float,
        typer.Option(
            help="seconds of post-shutdown silence before the recording is considered complete",
        ),
    ] = DEFAULT_IDLE_TIMEOUT,
    log_level: Annotated[
        str,
        typer.Option(
            envvar=LOG_LEVEL_ENV_VAR,
            help="logging level (TRACE/DEBUG/INFO/SUCCESS/WARNING/ERROR/CRITICAL); "
            f"reads {LOG_LEVEL_ENV_VAR} when unset.",
        ),
    ] = DEFAULT_LOG_LEVEL,
) -> None:
    """Record one episode to a Parquet file."""
    configure_logging(level=log_level)  # app-level logging; core stays quiet
    try:
        count = asyncio.run(
            record_episode(
                nats_url=nats_url,
                app=app_name,
                episode_id=episode_id,
                output=output,
                idle_timeout=idle_timeout,
            )
        )
    except RecorderError as error:
        print(f"freeagent-recorder: {error}", file=sys.stderr)
        raise typer.Exit(1) from error
    except KeyboardInterrupt:
        print("freeagent-recorder: interrupted", file=sys.stderr)
        raise typer.Exit(130) from None
    print(f"freeagent-recorder: wrote {count} rows to {output}")


def main(argv: Sequence[str] | None = None) -> None:
    """Console entry point: dispatch to the Typer app.

    ``argv`` defaults to ``sys.argv[1:]``. The single-command app is invoked
    so that ``--nats-url`` etc. are top-level options (no subcommand name).
    """
    app(args=argv, prog_name="freeagent-recorder")


if __name__ == "__main__":
    main()
