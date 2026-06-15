"""The Twenty Questions command-line app, mounted under ``free-agent``.

The application *is* its source: the app name, the environment class, and the
roster (which agent name is the Host and which are Players) are defined right
here, not in a configuration file. The YAML an operator passes to ``run`` holds
only per-episode tunables -- the NATS URL, the recorder, the episode id, and
each component's ``config`` block (timeouts, the Host's secret, model
overrides). The library does the launching; this module just wires the roster
to :func:`freeagent.run_episode`.

Invoked as::

    free-agent [--log-level LEVEL] twenty-questions run [CONFIG.yml]
"""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from freeagent import ConfigError, Environment, load_config, run_episode

from .environment import TwentyQuestionsEnvironment
from .host import Host
from .player import Player

#: Application name; the subject prefix every episode of this app lives under.
APP_NAME = "twentyquestions"

#: The environment class, defined in source.
ENVIRONMENT: type[Environment] = TwentyQuestionsEnvironment

#: The roster: one Host and three Players. Names are fixed in source; the YAML
#: only tunes each one's ``config``. Add or remove entries here to change who
#: plays -- that is a source change, by design.
ROSTER = {
    "host": Host,
    "alice": Player,
    "bob": Player,
    "carol": Player,
}

app = typer.Typer(
    name=APP_NAME,
    help="Twenty Questions: one Host and three Players, no turn-taking.",
    add_completion=False,
    no_args_is_help=True,
)


@app.command()
def run(
    config: Annotated[
        Path | None,
        typer.Argument(help="episode tunables *.yml file; all-defaults when omitted"),
    ] = None,
) -> None:
    """Run one Twenty Questions episode from a YAML of tunables."""
    try:
        episode = load_config(config)
        code = run_episode(episode, app=APP_NAME, environment=ENVIRONMENT, agents=ROSTER)
    except ConfigError as exc:
        typer.echo(f"free-agent: error: {exc}", err=True)
        raise typer.Exit(1) from exc
    except KeyboardInterrupt:
        typer.echo("free-agent: interrupted", err=True)
        raise typer.Exit(1) from None
    raise typer.Exit(code)
