"""Unit tests for the shared manifest work-queue names (ADR-0005, #53)."""

from __future__ import annotations

import pytest

from freeagent import (
    WORK_QUEUE_STREAM,
    WORK_QUEUE_SUBJECT_PREFIX,
    WORK_QUEUE_SUBJECTS,
    MemoryTransport,
    work_subject,
)
from freeagent.transport import subject_matches


def test_work_subject_is_per_episode_under_the_prefix() -> None:
    subject = work_subject("noopapp", "ep1")
    assert subject == f"{WORK_QUEUE_SUBJECT_PREFIX}.noopapp.ep1"


def test_work_subject_is_captured_by_the_stream_wildcard() -> None:
    subject = work_subject("noopapp", "ep1")
    assert any(subject_matches(pattern, subject) for pattern in WORK_QUEUE_SUBJECTS)


def test_stream_captures_a_single_wildcard() -> None:
    assert (f"{WORK_QUEUE_SUBJECT_PREFIX}.>",) == WORK_QUEUE_SUBJECTS
    assert WORK_QUEUE_STREAM == "freeagent_work"


@pytest.mark.parametrize(
    ("app", "episode_id"),
    [("bad app", "ep1"), ("noopapp", "ep 1"), ("no.dots", "ep1")],
)
def test_work_subject_rejects_unsafe_names(app: str, episode_id: str) -> None:
    with pytest.raises(ValueError, match="invalid"):
        work_subject(app, episode_id)


async def test_memory_work_queue_stream_is_idempotent_and_flagged() -> None:
    transport = MemoryTransport()
    await transport.connect()
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, WORK_QUEUE_SUBJECTS)
    # Re-ensuring with different subjects keeps the original (a real server
    # leaves the long-lived stream in place) and merges metadata.
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, ("other.>",), metadata={"k": "v"})
    assert transport.streams[WORK_QUEUE_STREAM] == WORK_QUEUE_SUBJECTS
    assert transport.work_queue_streams == {WORK_QUEUE_STREAM}
    assert transport.stream_meta[WORK_QUEUE_STREAM] == {"k": "v"}
