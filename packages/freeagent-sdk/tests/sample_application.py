"""A minimal :class:`~freeagent.sdk.application.Application` for the entry-point loader tests.

The loader resolves an entry point to whatever object it names; this module supplies that object
(:data:`application`) plus the ``value`` string an entry point would carry to reach it, so tests can
register it through an :func:`~importlib.metadata.entry_points` fixture without a real install. See
:mod:`test_application`.
"""

from __future__ import annotations

from freeagent.sdk.application import EpisodeSpec
from freeagent.sdk.entity import Agent, Environment


class SampleApplication:
    """A structural :class:`~freeagent.sdk.application.Application`: a name and two factories."""

    name = "sample"

    def make_environment(self, episode: EpisodeSpec) -> Environment:
        return Environment(episode.episode_root)

    def make_agents(self, episode: EpisodeSpec) -> list[Agent]:
        count = episode.config["agent_count"]
        return [Agent(episode.episode_root, f"agent-{i}") for i in range(count)]


application = SampleApplication()
"""The object a ``sample`` entry point resolves to; see :data:`SAMPLE_ENTRY_POINT_VALUE`."""

SAMPLE_ENTRY_POINT_VALUE = "sample_application:application"
"""The ``value`` an entry point uses to reach :data:`application`, as it would in a pyproject."""


def not_an_application() -> None:
    """A plain function, standing in for an entry point misregistered at a non-:class:`Application`.

    Missing ``name``/``make_environment``/``make_agents``, so
    :func:`~freeagent.sdk.application.load_application` rejects it with
    :class:`~freeagent.sdk.application.InvalidApplication`; see :mod:`test_application`.
    """


NOT_AN_APPLICATION_ENTRY_POINT_VALUE = "sample_application:not_an_application"
"""The ``value`` an entry point uses to reach :func:`not_an_application`."""
