"""The episode replayer: re-publish a recorded Parquet log onto NATS.

Replay is NATS playback, not log reading (ADR-0001): the recorded episode is
re-published onto a (separate, local) NATS server using byte-identical
subjects, so a viewer cannot tell live from replay and carries one code path.
The replayer is library infrastructure, app-agnostic, and exposed as the
top-level ``free-agent replay`` command; :func:`replay_episode` and
:class:`Replayer` are also importable for embedding (e.g. a GUI driving pause,
seek, and speed).
"""

from __future__ import annotations

from .replayer import (
    REQUIRED_COLUMNS,
    Replayer,
    ReplayerError,
    ReplayMessage,
    load_episode,
    replay_episode,
)

__all__ = [
    "REQUIRED_COLUMNS",
    "ReplayMessage",
    "Replayer",
    "ReplayerError",
    "load_episode",
    "replay_episode",
]
