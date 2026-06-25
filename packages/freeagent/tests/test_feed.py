"""The per-episode feed: one read path for live and replay (ADR-0003 / #42).

Fast tests drive :class:`EpisodeFeed` over a fake stream source -- no NATS -- and
assert the normalized event sequence: a status snapshot, an ``open`` connection
event, message history, the ``live`` boundary, and then either live appends (live
episode) or an immediate ``closed`` (sealed/replay). The "an observer can't tell
live from replay" invariant is checked directly: the same rows produce the same
message events whether read live or as a sealed replay.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from freeagent import Envelope
from freeagent.service import EpisodeFeed
from freeagent.service.feed import StreamRow, to_episode_message
from freeagent.service.models import EpisodeView

if TYPE_CHECKING:
    from collections.abc import Sequence

ROOT = "demo.episode.e1"
_T0 = datetime(2026, 6, 25, 12, 0, 0, tzinfo=UTC)


def _row(seq: int, sender: str, payload: object, *, channel: str = "public") -> StreamRow:
    subject = f"{ROOT}.{channel}"
    data = Envelope(episode_id="e1", sender=sender, payload=payload).to_bytes()
    return StreamRow(stream_seq=seq, subject=subject, received_at=_T0, data=data)


def _view(*, status: str, sealed: bool, name: str = "amber-otter") -> EpisodeView:
    return EpisodeView(
        id="e1",
        application="demo",
        app="demo",
        episode_id="e1",
        subject_root=ROOT,
        name=name,
        mode="live",
        status=status,
        sealed=sealed,
        outcome=None,
        nats_url="nats://localhost:4222",
        created_at=_T0,
    )


class _FakeSource:
    """A scripted :class:`~freeagent.service.feed.StreamSource` for tests.

    *script* items are ``StreamRow`` (a delivered message), ``None`` (a tail
    timeout with nothing new), or the string ``"SEAL"`` (the stream just sealed,
    delivered as a timeout). After the script is exhausted, ``next`` returns
    ``None`` forever.
    """

    def __init__(self, *, target: int, script: Sequence[object], sealed: bool = False) -> None:
        self._target = target
        self._script = list(script)
        self._sealed = sealed
        self._i = 0
        self.opened = False
        self.closed = False

    async def last_seq(self) -> int:
        return self._target

    async def sealed(self) -> bool:
        return self._sealed

    async def open(self) -> None:
        self.opened = True

    async def next(self, timeout: float) -> StreamRow | None:  # noqa: ASYNC109 -- timeout/signature mirror the protocol
        if self._i >= len(self._script):
            return None
        item = self._script[self._i]
        self._i += 1
        if item == "SEAL":
            self._sealed = True
            return None
        return item  # StreamRow or None

    async def close(self) -> None:
        self.closed = True


async def _drive(source: _FakeSource, view: EpisodeView) -> list[dict]:
    """Run the feed against *source*, collecting the emitted events as dicts."""
    events: list[dict] = []

    async def send(frame: str) -> None:
        events.append(json.loads(frame))

    async def snapshot() -> EpisodeView:
        return view

    await EpisodeFeed(source, snapshot, send).run()
    return events


def _types(events: list[dict]) -> list[str]:
    return [e.get("type") for e in events]


def _phases(events: list[dict]) -> list[str]:
    return [e["phase"] for e in events if e.get("type") == "connection"]


async def test_sealed_episode_streams_history_then_closes() -> None:
    rows = [_row(1, "env", {"type": "start"}, channel="control"), _row(2, "host", "hello")]
    source = _FakeSource(target=2, script=rows, sealed=True)
    events = await _drive(source, _view(status="ended", sealed=True))

    # status snapshot, open, two messages, live boundary, closed.
    assert _types(events) == [
        "status",
        "connection",
        "message",
        "message",
        "connection",
        "connection",
    ]
    assert _phases(events) == ["open", "live", "closed"]
    assert events[0]["status"] == "ended"
    assert events[0]["sealed"] is True
    senders = [e["message"]["sender"] for e in events if e.get("type") == "message"]
    assert senders == ["env", "host"]


async def test_empty_sealed_episode_goes_live_then_closes() -> None:
    source = _FakeSource(target=0, script=[], sealed=True)
    events = await _drive(source, _view(status="ended", sealed=True))
    assert _types(events) == ["status", "connection", "connection", "connection"]
    assert _phases(events) == ["open", "live", "closed"]


async def test_live_episode_streams_history_then_tails_until_seal() -> None:
    history = [_row(1, "host", "q1"), _row(2, "alice", "a1")]
    # history (seq 1,2) -> live boundary, then one timeout, a live append, then SEAL.
    script = [*history, None, _row(3, "host", "q2"), "SEAL"]
    source = _FakeSource(target=2, script=script, sealed=False)
    events = await _drive(source, _view(status="running", sealed=False))

    # open, then live at the history boundary, then closed once it seals.
    assert _phases(events) == ["open", "live", "closed"]
    senders = [e["message"]["sender"] for e in events if e.get("type") == "message"]
    assert senders == ["host", "alice", "host"]  # 2 history + 1 live append
    # The live boundary is emitted after the 2nd (history target) message, before
    # the live append -- the moment an observer can't tell live from replay.
    live_index = next(i for i, e in enumerate(events) if e.get("phase") == "live")
    messages_before_live = sum(1 for e in events[:live_index] if e.get("type") == "message")
    assert messages_before_live == 2
    # A second status snapshot is emitted when the live episode seals.
    assert _types(events).count("status") == 2


async def test_live_and_replay_produce_identical_messages() -> None:
    rows = [_row(1, "host", "q"), _row(2, "alice", "a"), _row(3, "host", "done")]

    replay = await _drive(
        _FakeSource(target=3, script=rows, sealed=True), _view(status="ended", sealed=True)
    )
    live = await _drive(
        _FakeSource(target=3, script=[*rows, "SEAL"], sealed=False),
        _view(status="running", sealed=False),
    )

    def messages(events: list[dict]) -> list[dict]:
        return [e["message"] for e in events if e.get("type") == "message"]

    # The message stream is byte-identical between live and replay reads.
    assert messages(replay) == messages(live)


def test_to_episode_message_derives_channel_and_handles_non_envelope() -> None:
    msg = to_episode_message(_row(5, "alice", "hi", channel="public"))
    assert msg.channel == "public"
    assert msg.sender == "alice"
    assert msg.payload == "hi"
    assert msg.stream_seq == 5

    raw = StreamRow(stream_seq=6, subject=f"{ROOT}.public", received_at=_T0, data=b"not-json{")
    fallback = to_episode_message(raw)
    assert fallback.sender == ""
    assert fallback.payload == "not-json{"


# ---------------------------------------------------------------------------
# Real JetStream: NatsStreamSource over a sealed episode (skipped without NATS)
# ---------------------------------------------------------------------------


def _nats_running() -> bool:
    import socket

    try:
        with socket.create_connection(("127.0.0.1", 4222), timeout=1.0):
            return True
    except OSError:
        return False


async def test_nats_feed_replays_a_sealed_episode() -> None:
    import asyncio
    import uuid

    import pytest

    if not _nats_running():
        pytest.skip("NATS is not running")

    from collatz_app import EVEN, ODD, CollatzAgent, CollatzEnvironment

    from freeagent import EpisodeState, default_nats_url
    from freeagent.service.feed import NatsStreamSource

    nats_url = default_nats_url()
    episode_id = uuid.uuid4().hex
    config = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.3}
    env = CollatzEnvironment("collatz", (EVEN, ODD), episode_id=episode_id, config=config)
    agents = [
        CollatzAgent(env.subject_root, EVEN, config={"grace_period": 0.1}),
        CollatzAgent(
            env.subject_root, ODD, config={"grace_period": 0.1, "starter": True, "seed": 6}
        ),
    ]
    state, *_ = await asyncio.wait_for(
        asyncio.gather(env.run(nats_url), *(agent.run(nats_url) for agent in agents)),
        timeout=20.0,
    )
    assert state is EpisodeState.ENDED  # the episode is now sealed

    source = NatsStreamSource(nats_url, "collatz", episode_id)
    view = _view(status="ended", sealed=True)
    view = view.model_copy(update={"app": "collatz", "episode_id": episode_id})
    try:
        events = await asyncio.wait_for(_drive(source, view), timeout=15.0)
    finally:
        await source.close()

    # Full history streamed, the live boundary marked, then closed -- no second
    # NATS server, the same read path a live observer would take.
    assert _phases(events) == ["open", "live", "closed"]
    messages = [e for e in events if e.get("type") == "message"]
    assert len(messages) >= 1
    assert [m["message"]["stream_seq"] for m in messages] == sorted(
        m["message"]["stream_seq"] for m in messages
    )
