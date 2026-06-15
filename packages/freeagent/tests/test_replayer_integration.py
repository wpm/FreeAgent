"""Integration tests for the replayer against a real NATS + JetStream server.

Skipped cleanly (via the ``nats_url`` fixture) when NATS is not running. These
prove the acceptance criteria from a recorded log: messages are re-published on
their original subjects in ``stream_seq`` order, and a subscriber to
``<app>.episode.<id>.public`` sees the same public-channel sequence as the
original episode. The synthetic log uses an arbitrary app name to demonstrate
the replayer is app-agnostic; episode ids are fresh uuids so a fresh stream is
created and never collides with concurrently running episodes.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import uuid
from datetime import UTC, datetime, timedelta
from typing import TYPE_CHECKING, Any

import nats
import nats.js.errors
import pytest

from freeagent import (
    Envelope,
    EpisodeSubjects,
    MessageRecord,
    NatsTransport,
    make_record,
    replay_episode,
    write_parquet,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator
    from pathlib import Path

# An arbitrary, non-Twenty-Questions app name: the replayer never needs to know.
APP = "replaytest"
T0 = datetime(2026, 6, 11, 12, 0, 0, tzinfo=UTC)


def _episode_records(subjects: EpisodeSubjects) -> list[tuple[str, str, Any]]:
    """A mini-episode as (subject, sender, payload) triples, in stream order."""
    return [
        (subjects.agent("alice"), "env", {"type": "presence", "request_id": "req-1"}),
        (subjects.reply("req-1"), "alice", {"type": "present", "nonce": "n-alice"}),
        (subjects.control, "env", {"type": "start"}),
        (subjects.public, "alice", "Is it an animal?"),
        (subjects.public, "bob", "¡Sí! — это животное 🦊"),
        (subjects.public, "alice", "Is it an octopus?"),
        (subjects.control, "env", {"type": "shutdown"}),
        (subjects.public, "bob", "goodbye all"),
    ]


def _write_episode(path: Path, subjects: EpisodeSubjects, *, gap: float = 0.0) -> None:
    """Write the mini-episode to a Parquet log with *gap* seconds between rows."""
    records: list[MessageRecord] = []
    for seq, (subject, sender, payload) in enumerate(_episode_records(subjects), start=1):
        envelope = Envelope(episode_id=subjects.episode_id, sender=sender, payload=payload)
        records.append(
            make_record(
                stream_seq=seq,
                subject=subject,
                received_at=T0 + timedelta(seconds=seq * gap),
                data=envelope.to_bytes(),
                fallback_episode_id=subjects.episode_id,
            )
        )
    write_parquet(records, path)


@pytest.fixture
async def fresh_episode(nats_url: str) -> AsyncIterator[EpisodeSubjects]:
    """Fresh episode subjects; deletes the stream the replayer creates, afterwards."""
    subjects = EpisodeSubjects(app=APP, episode_id=uuid.uuid4().hex)
    client = await nats.connect(nats_url)
    try:
        yield subjects
    finally:
        with contextlib.suppress(nats.js.errors.NotFoundError):
            await client.jetstream().delete_stream(subjects.stream)
        await client.close()


async def _collect(
    nats_url: str, subject: str, expected: int, *, wait_seconds: float = 10.0
) -> list[Any]:
    """Subscribe from sequence 1 and collect up to *expected* messages on *subject*."""
    received: list[Any] = []
    done = asyncio.Event()
    transport = NatsTransport(nats_url)
    await transport.connect()

    async def handler(_subject: str, data: bytes) -> None:
        received.append(Envelope.from_bytes(data))
        if len(received) >= expected:
            done.set()

    subscription = await transport.subscribe(subject, handler)
    try:
        await asyncio.wait_for(done.wait(), timeout=wait_seconds)
    finally:
        await subscription.unsubscribe()
        await transport.close()
    return received


async def test_replay_publishes_public_channel_in_order(
    nats_url: str, fresh_episode: EpisodeSubjects, tmp_path: Path
) -> None:
    subjects = fresh_episode
    log = tmp_path / "episode.parquet"
    _write_episode(log, subjects)

    expected_public = [
        (sender, payload)
        for subject, sender, payload in _episode_records(subjects)
        if subject == subjects.public
    ]

    count = await replay_episode(parquet_path=log, nats_url=nats_url, as_fast_as_possible=True)
    assert count == len(_episode_records(subjects))

    # A subscriber (reading the replay stream from sequence 1) sees exactly the
    # original public-channel sequence: same senders, same payloads, same order.
    public = await _collect(nats_url, subjects.public, expected=len(expected_public))
    assert [(env.sender, env.payload) for env in public] == expected_public
    assert all(env.episode_id == subjects.episode_id for env in public)


async def test_replay_preserves_every_subject(
    nats_url: str, fresh_episode: EpisodeSubjects, tmp_path: Path
) -> None:
    subjects = fresh_episode
    log = tmp_path / "episode.parquet"
    _write_episode(log, subjects)
    records = _episode_records(subjects)

    await replay_episode(parquet_path=log, nats_url=nats_url, as_fast_as_possible=True)

    # The whole episode stream, read back in order, matches every original row.
    all_messages = await _collect(nats_url, subjects.all_subjects, expected=len(records))
    assert [env.sender for env in all_messages] == [sender for _, sender, _ in records]
    assert [env.payload for env in all_messages] == [payload for _, _, payload in records]


async def test_replay_speed_scales_duration(
    nats_url: str, fresh_episode: EpisodeSubjects, tmp_path: Path
) -> None:
    subjects = fresh_episode
    log = tmp_path / "episode.parquet"
    # 0.3s between each of 8 rows -> ~2.1s real time; at speed 100 it is near-instant.
    _write_episode(log, subjects, gap=0.3)

    loop = asyncio.get_running_loop()
    start = loop.time()
    await replay_episode(parquet_path=log, nats_url=nats_url, speed=100.0)
    elapsed = loop.time() - start

    # Timing is honored but scaled down hard by the speed multiplier.
    assert elapsed < 1.0


async def test_replay_round_trips_a_recorded_log_to_json(
    nats_url: str, fresh_episode: EpisodeSubjects, tmp_path: Path
) -> None:
    """The replayed payloads JSON-match the log's payload column (the recorder's contract)."""
    subjects = fresh_episode
    log = tmp_path / "episode.parquet"
    _write_episode(log, subjects)

    import pyarrow.parquet as pq

    rows = pq.read_table(log).to_pylist()
    rows.sort(key=lambda row: row["stream_seq"])

    await replay_episode(parquet_path=log, nats_url=nats_url, as_fast_as_possible=True)
    all_messages = await _collect(nats_url, subjects.all_subjects, expected=len(rows))

    for env, row in zip(all_messages, rows, strict=True):
        assert env.sender == row["sender"]
        assert env.payload == json.loads(row["payload"])
