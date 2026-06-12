"""A tiny no-LLM application used by the runner's tests.

This module lives next to the test episode configurations on purpose: the
runner must make app modules that live beside the yml importable (in the
parent via ``sys.path``, in the children via ``PYTHONPATH``).
"""

from __future__ import annotations

from freeagent import Agent, Environment


class NoopAgent(Agent):
    """An agent that joins, stays silent, and winds down on ``shutdown``."""


class NoopEnvironment(Environment):
    """The base lifecycle with nothing on top; the episode ends by timeout."""


class CrashingAgent(Agent):
    """An agent whose run() fails immediately: it never answers presence.

    Used to drive an episode to ``aborted`` via the setup timeout.
    """

    async def run(self, nats_url: str) -> None:
        raise RuntimeError("sabotaged: this agent never joins the episode")


class NotAnAgent:
    """Importable, a class, but neither an Agent nor an Environment."""


NOT_A_CLASS = "importable, but not a class"
