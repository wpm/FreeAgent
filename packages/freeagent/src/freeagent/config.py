"""Default configuration values shared across FreeAgent components.

All values are in seconds. Applications override them through the ``config``
mapping passed to :class:`freeagent.environment.Environment` (keys
``setup_timeout``, ``episode_timeout``, ``grace_period``) and to
:class:`freeagent.agent.Agent` (key ``grace_period``).
"""

from __future__ import annotations

DEFAULT_SETUP_TIMEOUT: float = 30.0
"""Seconds the environment waits in setup for the full roster before aborting."""

DEFAULT_EPISODE_TIMEOUT: float = 600.0
"""Seconds an episode may run before the environment initiates shutdown."""

DEFAULT_GRACE_PERIOD: float = 5.0
"""Seconds of cooperative wind-down between the shutdown broadcast and the end."""
