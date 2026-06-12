"""FreeAgent episode recorder: JetStream stream -> one Parquet file per episode."""

from freeagent_recorder.recorder import (
    DEFAULT_IDLE_TIMEOUT,
    PARQUET_SCHEMA,
    MessageRecord,
    RecorderError,
    make_record,
    record_episode,
    write_parquet,
)

__all__ = [
    "DEFAULT_IDLE_TIMEOUT",
    "PARQUET_SCHEMA",
    "MessageRecord",
    "RecorderError",
    "make_record",
    "record_episode",
    "write_parquet",
]
