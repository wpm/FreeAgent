"""The Collatz :class:`~freeagent.sdk.Application`: its config model and the two factories.

An :class:`~freeagent.sdk.EpisodeSpec`'s ``config`` dict is opaque to the platform; Collatz
validates it here with :class:`CollatzConfig` -- the Python-as-source-of-truth discipline -- and
turns it into the episode's :class:`~freeagent.app.collatz.entity.CollatzEnvironment` and
:class:`~freeagent.app.collatz.entity.CollatzAgent` instances. The module-level :data:`application`
object is what the ``collatz`` entry point resolves to; see :func:`~freeagent.sdk.load_application`.
"""

from __future__ import annotations

from freeagent.app.collatz.entity import CollatzAgent, CollatzEnvironment
from freeagent.sdk import Agent, Environment, EpisodeSpec
from freeagent.sdk.entity import DEFAULT_NATS_SERVER
from pydantic import BaseModel, Field, model_validator


class CollatzConfig(BaseModel):
    """The validated shape of a Collatz episode's ``config`` dict.

    An episode is defined by the starting number handed to each agent. ``starts`` gives those
    numbers directly, one per agent; the agents are named ``agent-0``, ``agent-1``, ... in order.
    Requiring at least one start keeps an episode from being trivially, instantly complete with no
    agents to judge.

    :ivar starts: The starting number for each agent, in agent order; at least one, each a positive
        integer (the domain of the Collatz map).
    :ivar servers: NATS server URL(s) the episode's entities connect to; defaults to the SDK's
        default server.
    """

    starts: list[int] = Field(min_length=1)
    servers: str | list[str] = DEFAULT_NATS_SERVER

    @model_validator(mode="after")
    def _starts_are_positive(self) -> CollatzConfig:
        """Reject non-positive starting numbers, which fall outside the Collatz map's domain.

        :return: This config, unchanged, once every start is confirmed positive.
        :raises ValueError: If any start is less than 1.
        """
        bad = [n for n in self.starts if n < 1]
        if bad:
            raise ValueError(f"Collatz starting numbers must be positive integers; got {bad}")
        return self

    def agent_starts(self) -> dict[str, int]:
        """Map each agent's name to its starting number.

        Agents are named ``agent-0``, ``agent-1``, ... positionally, matching
        :meth:`CollatzApplication.make_agents`.

        :return: A dict from agent name to starting number, one entry per element of :attr:`starts`.
        """
        return {f"agent-{i}": n for i, n in enumerate(self.starts)}


class CollatzApplication:
    """The Collatz :class:`~freeagent.sdk.Application`: a name and the two entity factories.

    Structurally satisfies the :class:`~freeagent.sdk.Application` protocol (a ``name`` plus
    :meth:`make_environment` and :meth:`make_agents`) without inheriting from it. Both factories
    validate the episode's ``config`` through :class:`CollatzConfig`, so a malformed config fails
    loudly at build time inside the application rather than deep in a running entity.
    """

    name = "collatz"

    def make_environment(self, episode: EpisodeSpec) -> Environment:
        """Build the episode's single :class:`~freeagent.app.collatz.entity.CollatzEnvironment`.

        :param episode: The episode to build for; its ``config`` is validated by
            :class:`CollatzConfig`.
        :return: The episode's environment, seeded with each agent's starting number, not yet
            started.
        """
        config = CollatzConfig.model_validate(episode.config)
        return CollatzEnvironment(
            episode.episode_root, config.agent_starts(), servers=config.servers
        )

    def make_agents(self, episode: EpisodeSpec) -> list[Agent]:
        """Build the episode's :class:`~freeagent.app.collatz.entity.CollatzAgent` instances.

        One agent per starting number, named ``agent-0``, ``agent-1``, ... to match the
        environment's bookkeeping keys (:meth:`CollatzConfig.agent_starts`).

        :param episode: The episode to build for; its ``config`` is validated by
            :class:`CollatzConfig`.
        :return: The episode's agents, in agent order, not yet started.
        """
        config = CollatzConfig.model_validate(episode.config)
        return [
            CollatzAgent(episode.episode_root, name, servers=config.servers)
            for name in config.agent_starts()
        ]


application = CollatzApplication()
"""The object the ``collatz`` entry point resolves to.

See :func:`~freeagent.sdk.load_application`.
"""
