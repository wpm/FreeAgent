"""The ``free-agent`` root CLI and the building blocks applications mount into it.

``free-agent --log-level LEVEL APP_NAME COMMAND ...`` is the convention. The
root app owns one thing every application shares -- the ``--log-level`` option
-- and then dispatches to a per-application Typer sub-app discovered from the
``freeagent.apps`` entry-point group::

    # in an application's pyproject.toml
    [project.entry-points."freeagent.apps"]
    twenty-questions = "twentyquestions.cli:app"

Installing an application makes ``free-agent twenty-questions ...`` work; the
library never imports its applications by name. Each application defines its own
commands (``run`` and whatever else it needs) and calls
:func:`freeagent.cli.orchestrate.run_episode` for the standard launch behavior.

The orchestration, config loader, and child entry point live alongside this
module and are re-exported from :mod:`freeagent` for applications to import.
"""

from __future__ import annotations

import os
import sys
from importlib.metadata import entry_points
from pathlib import Path
from typing import TYPE_CHECKING, Annotated

import typer

from freeagent.logging import DEFAULT_LOG_LEVEL, LOG_LEVEL_ENV_VAR, configure_logging

from .config import (
    NATS_URL_ENV_VAR,
    ConfigError,
    EpisodeConfig,
    EpisodePlan,
    default_nats_url,
    load_config,
    make_plan,
)
from .orchestrate import (
    EpisodeHandle,
    EpisodeOutcome,
    EpisodeStatus,
    run_episode,
    start_episode,
)
from .replay import replay as _replay_command

if TYPE_CHECKING:
    from collections.abc import Sequence

ENTRY_POINT_GROUP = "freeagent.apps"


def _reject_existing(value: Path | None) -> Path | None:
    """Typer callback: refuse to overwrite an existing ``--parquet-log`` target.

    Click's ``Path(exists=...)`` only *asserts* existence; it has no
    "must-not-exist" mode, so the no-overwrite rule is enforced here. Raising
    ``typer.BadParameter`` surfaces as a usage error (exit 2) before the command
    body runs, so a finished log is never clobbered.
    """
    if value is not None and value.exists():
        raise typer.BadParameter(f"{value} already exists; refusing to overwrite")
    return value


#: The shared ``--parquet-log`` option every application's ``run`` command reuses.
#:
#: Recording is a per-run decision: pass ``--parquet-log PATH`` to record this
#: episode's full message stream to one new Parquet file; omit it for no
#: recording. The target must not already exist (see :func:`_reject_existing`).
#: Apps add ``parquet_log: ParquetLogOption = None`` to their ``run`` signature
#: and pass it to :func:`run_episode`.
ParquetLogOption = Annotated[
    Path | None,
    typer.Option(
        "--parquet-log",
        help="record this episode to a new Parquet file at PATH (must not exist); "
        "omit for no recording.",
        dir_okay=False,
        writable=True,
        callback=_reject_existing,
    ),
]

__all__ = [
    "NATS_URL_ENV_VAR",
    "ConfigError",
    "EpisodeConfig",
    "EpisodeHandle",
    "EpisodeOutcome",
    "EpisodePlan",
    "EpisodeStatus",
    "ParquetLogOption",
    "build_root_app",
    "default_nats_url",
    "load_config",
    "main",
    "make_plan",
    "run_episode",
    "start_episode",
]


def _log_level_callback(
    log_level: Annotated[
        str,
        typer.Option(
            envvar=LOG_LEVEL_ENV_VAR,
            help="logging level (TRACE/DEBUG/INFO/SUCCESS/WARNING/ERROR/CRITICAL); "
            f"reads {LOG_LEVEL_ENV_VAR} when unset.",
        ),
    ] = DEFAULT_LOG_LEVEL,
) -> None:
    """Configure logging once, before any application command runs.

    The resolved level is exported back to ``FREEAGENT_LOG_LEVEL`` so the child
    processes the orchestrator launches inherit it.
    """
    resolved = configure_logging(level=log_level)  # app-level logging; core stays quiet
    os.environ[LOG_LEVEL_ENV_VAR] = resolved


def _discover_apps() -> dict[str, typer.Typer]:
    """Load every ``freeagent.apps`` entry point into a name -> Typer sub-app map."""
    apps: dict[str, typer.Typer] = {}
    for entry in entry_points(group=ENTRY_POINT_GROUP):
        loaded = entry.load()
        if not isinstance(loaded, typer.Typer):
            raise TypeError(
                f"{ENTRY_POINT_GROUP} entry point {entry.name!r} "
                f"({entry.value}) is not a typer.Typer instance"
            )
        apps[entry.name] = loaded
    return apps


def build_root_app() -> typer.Typer:
    """Build the ``free-agent`` Typer app with every installed application mounted."""
    root = typer.Typer(
        name="free-agent",
        help="Run a FreeAgent application: free-agent [--log-level LEVEL] APP COMMAND ...",
        add_completion=False,
        no_args_is_help=True,
    )
    root.callback()(_log_level_callback)
    # The replayer is a library-level command, app-agnostic and a sibling of
    # the per-app sub-commands (ADR-0001): one tool replays any app's episode.
    root.command("replay")(_replay_command)
    for name, sub_app in sorted(_discover_apps().items()):
        root.add_typer(sub_app, name=name)
    return root


def main(argv: Sequence[str] | None = None) -> None:
    """Console entry point for ``free-agent``: build the root app and dispatch.

    ``argv`` defaults to ``sys.argv[1:]``; passing a list drives the app in
    tests. The Typer/Click app raises ``SystemExit`` with the command's exit
    code.
    """
    app = build_root_app()
    app(args=argv, prog_name="free-agent")


if __name__ == "__main__":
    main(sys.argv[1:])
