"""FreeAgent dev runner: launch an episode's processes from a YAML config.

``free-agent run <config>.yml`` reads an episode configuration, validates
it, launches the environment, every roster agent, and (optionally) the
recorder as separate OS processes, waits for the episode to end, cleans
everything up, and exits 0 (ended), 2 (aborted), or 1 (config/launch/
internal error).
"""

from __future__ import annotations

from .config import ConfigError, EpisodeConfig, EpisodePlan, load_config, make_plan

__all__ = [
    "ConfigError",
    "EpisodeConfig",
    "EpisodePlan",
    "load_config",
    "make_plan",
]
