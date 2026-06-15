"""The episode recorder: drain one episode's JetStream stream to one Parquet file.

The recorder is library infrastructure, not an application. The orchestrator
spawns it as its own OS process per episode (see
:func:`freeagent.cli.orchestrate._spawn_recorder`); :func:`record_episode` and
the Parquet shapes below are also importable for recording an episode in
process. There is no standalone console script.
"""

from __future__ import annotations

from .recorder import (
    DEFAULT_IDLE_TIMEOUT,
    DEFAULT_STREAM_WAIT,
    PARQUET_SCHEMA,
    MessageRecord,
    RecorderError,
    make_record,
    record_episode,
    write_parquet,
)

__all__ = [
    "DEFAULT_IDLE_TIMEOUT",
    "DEFAULT_STREAM_WAIT",
    "PARQUET_SCHEMA",
    "MessageRecord",
    "RecorderError",
    "make_record",
    "record_episode",
    "write_parquet",
]
