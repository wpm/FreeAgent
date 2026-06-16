"""The Twenty Questions command-line app, mounted under ``free-agent``.

The application *is* its source: the app name, the environment class, and the
roster (which agent name is the Host and which are Players) are defined right
here, not in a configuration file. The YAML an operator passes to ``run`` holds
only per-episode tunables -- the NATS URL, the recorder, the episode id, and
each component's ``config`` block (timeouts, the Host's secret, model
overrides). The library does the launching; this module just wires the roster
to :func:`freeagent.run_episode`.

That same identity is also advertised, once, as an :class:`~freeagent.AppSpec`
(the module-level :data:`APP`), so a generic launcher -- the control service --
can run an episode of this app without importing it by name. ``APP`` is what the
``freeagent.apps`` entry point points at; it carries the subject-prefix name,
the environment, the roster, the settable config surface, and the Typer
sub-app (:data:`app`) the root CLI mounts.

Invoked as::

    free-agent [--log-level LEVEL] twenty-questions run [CONFIG.yml]
"""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from freeagent import (
    AppSpec,
    ConfigError,
    ConfigField,
    Environment,
    ParquetLogOption,
    SettableConfig,
    load_config,
)
from freeagent.cli.apps import ENVIRONMENT_FIELDS, LLM_AGENT_FIELDS

from .environment import TwentyQuestionsEnvironment
from .host import Host
from .player import Player

#: Application name; the subject prefix every episode of this app lives under.
#: Note the spelling: this is undashed, while the dashed ``twenty-questions`` is
#: the REST/entry-point name. :data:`APP` carries both ends of that mapping.
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

#: The Host's own settable config, on top of the standard LLMAgent keys.
_HOST_FIELDS: tuple[ConfigField, ...] = (
    *LLM_AGENT_FIELDS,
    ConfigField(
        "secret", "string", "the word or phrase to guess (default: a random canned secret)"
    ),
    ConfigField("max_questions", "integer", "how many questions the Players get (default 20)"),
)

#: Which ``config`` fields an operator may set, by component. Players expose the
#: standard LLMAgent surface; the Host adds the secret and the question budget.
SETTABLE_CONFIG = SettableConfig(
    environment=ENVIRONMENT_FIELDS,
    agents={
        "host": _HOST_FIELDS,
        "alice": LLM_AGENT_FIELDS,
        "bob": LLM_AGENT_FIELDS,
        "carol": LLM_AGENT_FIELDS,
    },
)

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
    parquet_log: ParquetLogOption = None,
) -> None:
    """Run one Twenty Questions episode from a YAML of tunables.

    Pass ``--parquet-log PATH`` to record the episode to a new Parquet file;
    omit it for no recording.
    """
    try:
        episode = load_config(config)
        code = APP.run(episode, parquet_log=parquet_log)
    except ConfigError as exc:
        typer.echo(f"free-agent: error: {exc}", err=True)
        raise typer.Exit(1) from exc
    except KeyboardInterrupt:
        typer.echo("free-agent: interrupted", err=True)
        raise typer.Exit(1) from None
    raise typer.Exit(code)


#: The application's self-description: what ``freeagent.apps`` registers and what
#: a generic launcher loads. Defined after ``app`` so it can carry the CLI.
APP = AppSpec(
    name=APP_NAME,
    environment=ENVIRONMENT,
    roster=ROSTER,
    settable_config=SETTABLE_CONFIG,
    cli=app,
)
