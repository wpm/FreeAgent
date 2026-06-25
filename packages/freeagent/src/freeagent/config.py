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

DEFAULT_LIVENESS_INTERVAL: float = 15.0
"""Seconds between mid-episode liveness probes the environment broadcasts.

A lost role aborts the episode (ADR-0005): while RUNNING, the environment
periodically re-requests presence from the roster; a member that was present at
start but stops answering past :data:`DEFAULT_LIVENESS_TIMEOUT` drives a graceful
abort. ``0`` disables the probe entirely. The default is conservative so it never
disturbs short, timing-sensitive episodes."""

DEFAULT_LIVENESS_TIMEOUT: float = 10.0
"""Seconds a previously-present role may go unanswered before it is declared lost.

The deadline that distinguishes a genuinely absent participant (its child died)
from one that is merely quiet: each liveness probe waits this long for a reply
before the environment aborts. Must comfortably exceed a healthy round-trip."""
