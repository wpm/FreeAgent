"""Collatz: a deterministic, LLM-free Free Agent application that exercises every SDK path.

The environment hands each agent a :class:`~freeagent.app.collatz.message.Chain` and the agent
extends it by exactly one Collatz step, sending the longer chain back as its own request (ADR-0008's
ack-then-counter-request shape). The environment owns all game-state judgment: it stops an agent
when its chain reaches 1 and publishes :class:`~freeagent.sdk.message.EpisodeComplete` when every
agent has finished. Collatz doubles as the validation of the entry-point loading story (ADR-0006):
it registers the ``collatz`` entry point resolving to :data:`application`, so
:func:`~freeagent.sdk.load_application` can turn the bare name ``collatz`` into loadable code.
"""

from freeagent.app.collatz.application import (
    CollatzApplication,
    CollatzConfig,
    application,
)
from freeagent.app.collatz.entity import CollatzAgent, CollatzEnvironment
from freeagent.app.collatz.message import Chain, collatz_step, extend, is_complete

__all__ = [
    "Chain",
    "CollatzAgent",
    "CollatzApplication",
    "CollatzConfig",
    "CollatzEnvironment",
    "application",
    "collatz_step",
    "extend",
    "is_complete",
]
