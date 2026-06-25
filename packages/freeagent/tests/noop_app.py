"""A tiny no-LLM application used by the CLI orchestration tests.

It is importable both in the test process and in the child processes the
orchestrator spawns (the integration test exports this directory on
``PYTHONPATH`` so ``python -m freeagent.cli.child`` can import it).
"""

from __future__ import annotations

import asyncio
import signal

import typer

from freeagent import AGENT_FIELDS, Agent, AppSpec, ConfigField, Environment, SettableConfig
from freeagent.transport import NatsTransport


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


#: The JetStream stream the worker integration test pre-creates for markers.
MARKER_STREAM = "freeagent_test_markers"
#: Subject a :class:`MarkerAgent` publishes its id under (test asserts no dupes).
MARKER_SUBJECT = "freeagent.test.marker"


class MarkerAgent(Agent):
    """A self-contained worker-launched child: publish my id once, then exit 0.

    Used by the worker integration test to prove a manifest is launched exactly
    once: each manifest's child publishes its ``agent_id`` to ``MARKER_SUBJECT``,
    so the test counts markers. It does not join an episode (no environment is
    running); it just exercises the worker's pull -> launch -> ack path against
    real NATS.
    """

    async def run(self, nats_url: str) -> None:
        transport = NatsTransport(nats_url)
        await transport.connect()
        try:
            await transport.publish(MARKER_SUBJECT, self.id.encode())
        finally:
            await transport.close()


class StragglerAgent(Agent):
    """A child that comes up, then ignores cooperative shutdown forever.

    It traps SIGTERM (so ``terminate()`` does nothing) and blocks, so only
    ``kill()`` ends it -- the worker's terminate->kill escalation is what stops
    it. The test asserts the worker still returns promptly.
    """

    async def run(self, nats_url: str) -> None:
        # Connect so the worker's readiness window sees a live child, not an
        # EXIT_TRANSPORT failure.
        transport = NatsTransport(nats_url)
        await transport.connect()
        try:
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            # Block forever on an event that is never set: only kill() ends us.
            await asyncio.Event().wait()
        finally:
            await transport.close()
