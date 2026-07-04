"""Library for building Free Agent applications.

A Free Agent application is a collection of independent agent processes that communicate with each
other over NATS. The base classes here give those processes a common lifecycle: connect to NATS,
publish and subscribe to subjects, and shut down cleanly.

An :class:`Application` bundles those processes under a bare name and registers itself as a Python
entry point, so the platform can turn a name arriving over REST into loadable code with
:func:`load_application`; see :mod:`freeagent.sdk.application`.
"""

from freeagent.sdk.application import (
    Application,
    EpisodeSpec,
    UnknownApplication,
    available_applications,
    load_application,
)
from freeagent.sdk.entity import Agent, Environment

__all__ = [
    "Agent",
    "Application",
    "Environment",
    "EpisodeSpec",
    "UnknownApplication",
    "available_applications",
    "load_application",
]
