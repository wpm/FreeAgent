"""Unit tests for the replayer's reading and playback logic: no NATS, no network.

These cover loading a Parquet log into messages, reconstructing the wire bytes,
and the :class:`Replayer` controls (order, speed-scaled timing, pause, seek,
stop) driven against the in-memory transport with an injectable sleep.
"""

from __future__ import annotations

import asyncio
import json
from datetime import UTC, datetime, timedelta
from typing import TYPE_CHECKING

import pytest

from freeagent import (
    Envelope,
    MemoryTransport,
    MessageRecord,
    Replayer,
    ReplayerError,
    ReplayMessage,
    load_episode,
    make_record,
    write_parquet,
)
from freeagent.replayer.replayer import _ensure_streams

if TYPE_CHECKING:
    from pathlib import Path

T0 = datetime(2026, 6, 11, 12, 0, 0, tzinfo=UTC)
APP = "replaytest"
EPISODE = "ep1"
ROOT = f"{APP}.episode.{EPISODE}"


def _msg(
    seq: int,
    *,
    subject: str = f"{ROOT}.public",
    sender: str = "alice",
    payload: str | None = '"hi"',
    offset: float = 0.0,
) -> ReplayMessage:
    return ReplayMessage(
        episode_id=EPISODE,
        stream_seq=seq,
        subject=subject,
        sender=sender,
        received_at=T0 + timedelta(seconds=offset),
        payload=payload,
    )


# --------------------------------------------------------------------------- #
# Loading                                                                      #
# --------------------------------------------------------------------------- #


def _record(seq: int, payload: object, sender: str = "alice") -> MessageRecord:
    envelope = Envelope(episode_id=EPISODE, sender=sender, payload=payload)
    return make_record(
        stream_seq=seq,
        subject=f"{ROOT}.public",
        received_at=T0 + timedelta(seconds=seq),
        data=envelope.to_bytes(),
        fallback_episode_id=EPISODE,
    )


def test_load_episode_sorts_by_stream_seq(tmp_path: Path) -> None:
    path = tmp_path / "episode.parquet"
    write_parquet([_record(3, "c"), _record(1, "a"), _record(2, "b")], path)

    messages = load_episode(path)
    assert [m.stream_seq for m in messages] == [1, 2, 3]
    assert [json.loads(m.payload) for m in messages] == ["a", "b", "c"]
    assert all(m.episode_id == EPISODE for m in messages)


def test_load_episode_rejects_missing_columns(tmp_path: Path) -> None:
    import pyarrow as pa
    import pyarrow.parquet as pq

    path = tmp_path / "bad.parquet"
    pq.write_table(pa.table({"subject": ["x"], "payload": ["y"]}), path)

    with pytest.raises(ReplayerError, match="missing column"):
        load_episode(path)


def test_load_episode_rejects_unreadable_file(tmp_path: Path) -> None:
    path = tmp_path / "not.parquet"
    path.write_text("not parquet at all", encoding="utf-8")

    with pytest.raises(ReplayerError, match="cannot read Parquet file"):
        load_episode(path)


# --------------------------------------------------------------------------- #
# Wire reconstruction                                                          #
# --------------------------------------------------------------------------- #


def test_to_wire_rebuilds_the_envelope() -> None:
    payload = {"type": "say", "text": "héllo — 日本語", "n": [1, 2.5, None, True]}
    message = _msg(1, sender="bob", payload=json.dumps(payload))

    envelope = Envelope.from_bytes(message.to_wire())
    assert envelope.episode_id == EPISODE
    assert envelope.sender == "bob"
    assert envelope.payload == payload


def test_to_wire_handles_a_null_payload() -> None:
    envelope = Envelope.from_bytes(_msg(1, payload=None).to_wire())
    assert envelope.payload is None


def test_to_wire_passes_raw_non_envelope_bytes_through() -> None:
    # A message the recorder captured raw (not valid JSON) is republished verbatim.
    message = _msg(1, sender="", payload="not an envelope")
    assert message.to_wire() == b"not an envelope"


# --------------------------------------------------------------------------- #
# Playback order                                                               #
# --------------------------------------------------------------------------- #


async def test_play_publishes_in_stream_seq_order_as_fast_as_possible() -> None:
    transport = MemoryTransport()
    messages = [
        _msg(1, subject=f"{ROOT}.control", sender="env", payload='{"type": "start"}'),
        _msg(2, payload='"first"'),
        _msg(3, subject=f"{ROOT}.agent.alice", sender="env", payload='"hello alice"'),
        _msg(4, payload='"second"'),
    ]
    replayer = Replayer(messages, transport, as_fast_as_possible=True)

    count = await replayer.play()

    assert count == 4
    assert [subject for subject, _ in transport.history] == [m.subject for m in messages]
    # Each published frame round-trips as the original envelope.
    senders = [Envelope.from_bytes(data).sender for _, data in transport.history]
    assert senders == ["env", "alice", "env", "alice"]


async def test_play_returns_zero_for_an_empty_episode() -> None:
    transport = MemoryTransport()
    assert await Replayer([], transport, as_fast_as_possible=True).play() == 0
    assert transport.history == []


# --------------------------------------------------------------------------- #
# Timing                                                                       #
# --------------------------------------------------------------------------- #


async def test_play_preserves_inter_message_timing() -> None:
    delays: list[float] = []

    async def fake_sleep(seconds: float) -> None:
        delays.append(seconds)

    messages = [_msg(1, offset=0.0), _msg(2, offset=1.0), _msg(3, offset=3.0)]
    replayer = Replayer(messages, MemoryTransport(), sleep=fake_sleep)

    await replayer.play()

    # No wait before the first message; then the gaps 1.0s and 2.0s.
    assert delays == [1.0, 2.0]


async def test_speed_scales_the_delays() -> None:
    delays: list[float] = []

    async def fake_sleep(seconds: float) -> None:
        delays.append(seconds)

    messages = [_msg(1, offset=0.0), _msg(2, offset=1.0), _msg(3, offset=3.0)]
    replayer = Replayer(messages, MemoryTransport(), speed=2.0, sleep=fake_sleep)

    await replayer.play()

    assert delays == [0.5, 1.0]


async def test_as_fast_as_possible_never_sleeps() -> None:
    delays: list[float] = []

    async def fake_sleep(seconds: float) -> None:
        delays.append(seconds)

    messages = [_msg(1, offset=0.0), _msg(2, offset=5.0)]
    replayer = Replayer(messages, MemoryTransport(), as_fast_as_possible=True, sleep=fake_sleep)

    await replayer.play()
    assert delays == []


async def test_clock_ties_yield_no_wait() -> None:
    delays: list[float] = []

    async def fake_sleep(seconds: float) -> None:
        delays.append(seconds)

    # Same timestamp on both messages: a zero (or negative) gap is clamped to no wait.
    messages = [_msg(1, offset=2.0), _msg(2, offset=2.0)]
    await Replayer(messages, MemoryTransport(), sleep=fake_sleep).play()
    assert delays == []


def test_speed_must_be_positive() -> None:
    with pytest.raises(ValueError, match="greater than 0"):
        Replayer([], MemoryTransport(), speed=0)
    replayer = Replayer([], MemoryTransport())
    with pytest.raises(ValueError, match="greater than 0"):
        replayer.speed = -1.0


# --------------------------------------------------------------------------- #
# Controls: seek, pause/resume, stop                                           #
# --------------------------------------------------------------------------- #


async def test_seek_starts_playback_from_a_later_position() -> None:
    transport = MemoryTransport()
    messages = [_msg(seq, payload=f'"{seq}"') for seq in (1, 2, 3, 4)]
    replayer = Replayer(messages, transport, as_fast_as_possible=True)

    replayer.seek(2)
    count = await replayer.play()

    assert count == 2
    payloads = [Envelope.from_bytes(data).payload for _, data in transport.history]
    assert payloads == ["3", "4"]


async def test_seek_clamps_out_of_range() -> None:
    replayer = Replayer([_msg(1), _msg(2)], MemoryTransport())
    replayer.seek(99)
    assert replayer.index == 2
    replayer.seek(-5)
    assert replayer.index == 0


async def test_pause_holds_then_resume_finishes() -> None:
    transport = MemoryTransport()
    messages = [_msg(seq) for seq in (1, 2, 3)]
    replayer = Replayer(messages, transport, as_fast_as_possible=True)

    replayer.pause()
    task = asyncio.create_task(replayer.play())
    await asyncio.sleep(0.02)  # let play() reach the pause boundary
    assert replayer.published == 0

    replayer.resume()
    assert await asyncio.wait_for(task, timeout=1) == 3


async def test_stop_before_play_publishes_nothing() -> None:
    transport = MemoryTransport()
    replayer = Replayer([_msg(1), _msg(2)], transport, as_fast_as_possible=True)
    replayer.stop()

    assert await replayer.play() == 0
    assert transport.history == []


async def test_stop_wakes_a_paused_replayer() -> None:
    replayer = Replayer([_msg(1), _msg(2)], MemoryTransport(), as_fast_as_possible=True)
    replayer.pause()
    task = asyncio.create_task(replayer.play())
    await asyncio.sleep(0.02)

    replayer.stop()
    assert await asyncio.wait_for(task, timeout=1) == 0


# --------------------------------------------------------------------------- #
# Stream provisioning                                                          #
# --------------------------------------------------------------------------- #


async def test_ensure_streams_creates_one_stream_per_episode_root() -> None:
    transport = MemoryTransport()
    messages = [
        _msg(1, subject=f"{ROOT}.public"),
        _msg(2, subject=f"{ROOT}.control", sender="env"),
        _msg(3, subject="otherapp.episode.ep2.public"),
    ]
    await _ensure_streams(transport, messages)

    assert transport.streams == {
        f"{APP}_episode_{EPISODE}": (f"{ROOT}.>",),
        "otherapp_episode_ep2": ("otherapp.episode.ep2.>",),
    }


async def test_ensure_streams_rejects_a_non_episode_subject() -> None:
    with pytest.raises(ReplayerError, match="not a FreeAgent episode subject"):
        await _ensure_streams(MemoryTransport(), [_msg(1, subject="bogus.subject")])
