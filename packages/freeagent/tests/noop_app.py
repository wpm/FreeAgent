"""A tiny no-LLM application used by the CLI orchestration tests.

It is importable both in the test process and in the child processes the
orchestrator spawns (the integration test exports this directory on
``PYTHONPATH`` so ``python -m freeagent.cli.child`` can import it).
"""

from __future__ import annotations

import typer

from freeagent import AGENT_FIELDS, Agent, AppSpec, ConfigField, Environment, SettableConfig


class NoopAgent(Agent):
    """An agent that joins, stays silent, and winds down on ``shutdown``."""


class NoopEnvironment(Environment):
    """The base lifecycle with nothing on top; the episode ends by timeout."""


_cli = typer.Typer(name="noop")


@_cli.command()
def ping() -> None:
    typer.echo("pong from noop")


#: A self-describing registration the apps-registry tests load by REST name.
APP = AppSpec(
    name="noopapp",
    environment=NoopEnvironment,
    roster={"alpha": NoopAgent},
    settable_config=SettableConfig(
        environment=(ConfigField("episode_timeout", "number", "seconds before timeout"),),
        agents={"alpha": AGENT_FIELDS},
    ),
    cli=_cli,
)


class CrashingAgent(Agent):
    """An agent whose run() fails immediately: it never answers presence.

    Used to drive an episode to ``aborted`` via the setup timeout.
    """

    async def run(self, nats_url: str) -> None:
        raise RuntimeError("sabotaged: this agent never joins the episode")


class NotAnAgent:
    """Importable, a class, but neither an Agent nor an Environment."""
