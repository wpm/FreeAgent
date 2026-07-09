"""Library for building Free Agent applications.

A Free Agent application is a collection of independent agent processes that communicate with each
other over NATS. The base classes here give those processes a common lifecycle: connect to NATS,
publish and subscribe to subjects, and shut down cleanly.

An :class:`Application` bundles those processes under a bare name and registers itself as a Python
entry point, so the platform can turn a name arriving over REST into loadable code with
:func:`load_application`; see :mod:`freeagent.sdk.application`.
"""

from freeagent.sdk.application import (
    AmbiguousApplication,
    Application,
    EpisodeSpec,
    InvalidApplication,
    UnknownApplication,
    available_applications,
    load_application,
)
from freeagent.sdk.entity import Agent, Environment
from freeagent.sdk.launch import Launcher, Service, run

__all__ = [
    "Agent",
    "AmbiguousApplication",
    "Application",
    "Environment",
    "EpisodeSpec",
    "InvalidApplication",
    "Launcher",
    "Service",
    "UnknownApplication",
    "available_applications",
    "load_application",
    "run",
]
