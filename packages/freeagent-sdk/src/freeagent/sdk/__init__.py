"""Library for building Free Agent applications.

A Free Agent application is a collection of independent agent processes that communicate with each
other over NATS. The base classes here give those processes a common lifecycle: connect to NATS,
publish and subscribe to subjects, and shut down cleanly.
"""

from freeagent.sdk.entity import Agent

__all__ = ["Agent"]
