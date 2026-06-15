"""Integration tests for the recorder against a real NATS + JetStream server.

Skipped cleanly (via the ``nats_url`` fixture) when NATS is not running.
Episode ids are fresh uuids per test: the JetStream volume is persistent and
old streams exist.
"""

from __future__ import annotations

import asyncio
import itertools
import json
import uuid
from typing import TYPE_CHECKING, Any

import nats
import pyarrow.parquet as pq
import pytest

from freeagent import (
    PARQUET_SCHEMA,
    Envelope,
    EpisodeSubjects,
    RecorderError,
    record_episode,
)

if TYPE_CHECKING:
    from pathlib import Path

    from nats.js import JetStreamContext

APP = "recordertest"
IDLE_TIMEOUT = 0.5


def _mini_episode(subjects: EpisodeSubjects) -> list[tuple[str, Envelope]]:
    """A realistic mini-episode: presence handshake, start, chatter, shutdown, goodbye."""
    ep = subjects.episode_id

    def env_msg(subject: str, payload: Any) -> tuple[str, Envelope]:
        return subject, Envelope(episode_id=ep, sender="env", payload=payload)

    def agent_msg(subject: str, sender: str, payload: Any) -> tuple[str, Envelope]:
        return subject, Envelope(episode_id=ep, sender=sender, payload=payload)

    return [
        env_msg(subjects.agent("alice"), {"type": "presence", "request_id": "req-1"}),
        agent_msg(subjects.reply("req-1"), "alice", {"type": "present", "nonce": "n-alice"}),
        env_msg(subjects.control, {"type": "start"}),
        agent_msg(subjects.public, "alice", {"type": "say", "text": "Is it an animal?"}),
        agent_msg(subjects.public, "bob", {"type": "say", "text": "¡Sí! — это животное 🦊"}),
        env_msg(subjects.control, {"type": "shutdown"}),
        # Goodbye during the grace period, AFTER shutdown.
        agent_msg(subjects.public, "bob", {"type": "say", "text": "goodbye all"}),
    ]


async def _publish(js: JetStreamContext, messages: list[tuple[str, Envelope]]) -> None:
    for subject, envelope in messages:
        await js.publish(subject, envelope.to_bytes())


def _assert_round_trip(
    parquet_path: Path, messages: list[tuple[str, Envelope]], episode_id: str
) -> None:
    """Every published message present, ordered, with exact payload round-trip."""
    table = pq.read_table(parquet_path)
    assert table.schema.equals(PARQUET_SCHEMA)
    rows = table.to_pylist()
    assert len(rows) == len(messages)

    seqs = [row["stream_seq"] for row in rows]
    assert seqs == sorted(seqs)
    assert len(set(seqs)) == len(seqs)

    timestamps = [row["received_at"] for row in rows]
    assert all(t.tzinfo is not None for t in timestamps)
    assert all(a <= b for a, b in itertools.pairwise(timestamps))

    for row, (subject, envelope) in zip(rows, messages, strict=True):
        assert row["episode_id"] == episode_id
        assert row["subject"] == subject
        assert row["sender"] == envelope.sender
        assert json.loads(row["payload"]) == envelope.payload


@pytest.fixture
async def episode(nats_url: str) -> Any:
    """A fresh episode's subjects plus a JetStream context; deletes the stream after."""
    subjects = EpisodeSubjects(app=APP, episode_id=uuid.uuid4().hex)
    client = await nats.connect(nats_url)
    js = client.jetstream()
    await js.add_stream(name=subjects.stream, subjects=[subjects.all_subjects])
    try:
        yield subjects, js
    finally:
        await js.delete_stream(subjects.stream)
        await client.close()


async def test_recorder_concurrent_with_live_episode(
    nats_url: str, episode: tuple[EpisodeSubjects, JetStreamContext], tmp_path: Path
) -> None:
    subjects, js = episode
    output = tmp_path / "episode.parquet"
    messages = _mini_episode(subjects)

    recorder = asyncio.create_task(
        record_episode(
            nats_url=nats_url,
            app=subjects.app,
            episode_id=subjects.episode_id,
            output=output,
            idle_timeout=IDLE_TIMEOUT,
        )
    )
    # Trickle the episode out while the recorder is consuming.
    for subject, envelope in messages:
        await js.publish(subject, envelope.to_bytes())
        await asyncio.sleep(0.05)

    count = await asyncio.wait_for(recorder, timeout=15)
    assert count == len(messages)
    _assert_round_trip(output, messages, subjects.episode_id)


async def test_recorder_after_episode_ended(
    nats_url: str, episode: tuple[EpisodeSubjects, JetStreamContext], tmp_path: Path
) -> None:
    subjects, js = episode
    output = tmp_path / "episode.parquet"
    messages = _mini_episode(subjects)

    # The whole episode is already in the stream before the recorder starts.
    await _publish(js, messages)

    count = await asyncio.wait_for(
        record_episode(
            nats_url=nats_url,
            app=subjects.app,
            episode_id=subjects.episode_id,
            output=output,
            idle_timeout=IDLE_TIMEOUT,
        ),
        timeout=15,
    )
    assert count == len(messages)
    _assert_round_trip(output, messages, subjects.episode_id)


async def test_recorder_records_non_envelope_messages(
    nats_url: str, episode: tuple[EpisodeSubjects, JetStreamContext], tmp_path: Path
) -> None:
    subjects, js = episode
    output = tmp_path / "episode.parquet"

    await js.publish(subjects.log("raw"), b"not an envelope")
    shutdown = Envelope(episode_id=subjects.episode_id, sender="env", payload={"type": "shutdown"})
    await js.publish(subjects.control, shutdown.to_bytes())

    count = await asyncio.wait_for(
        record_episode(
            nats_url=nats_url,
            app=subjects.app,
            episode_id=subjects.episode_id,
            output=output,
            idle_timeout=IDLE_TIMEOUT,
        ),
        timeout=15,
    )
    assert count == 2
    rows = pq.read_table(output).to_pylist()
    assert rows[0]["sender"] == ""
    assert rows[0]["payload"] == "not an envelope"
    assert rows[0]["episode_id"] == subjects.episode_id
    assert rows[1]["sender"] == "env"


async def test_recorder_fails_when_stream_never_appears(nats_url: str, tmp_path: Path) -> None:
    with pytest.raises(RecorderError, match="not found"):
        await record_episode(
            nats_url=nats_url,
            app=APP,
            episode_id=uuid.uuid4().hex,
            output=tmp_path / "never.parquet",
            idle_timeout=IDLE_TIMEOUT,
            stream_wait=0.6,
        )
