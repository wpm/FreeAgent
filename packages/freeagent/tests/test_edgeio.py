"""Parquet import/export as episode edge I/O (ADR-0003 / #43).

Fast tests drive **import** over an in-memory transport -- no NATS -- proving a
recorded log plays into a fresh, sealed, metadata-bearing stream that rejoins the
browse surface (the :class:`EpisodeStore` lists it). A slow NATS test does the
full round trip: run a real episode, export it, re-import it, and confirm it
appears and reads like any other episode.
"""

from __future__ import annotations

import socket
from datetime import UTC, datetime
from pathlib import Path

import pytest

from freeagent import Envelope, EpisodeMetadata, MemoryTransport, make_record, write_parquet
from freeagent.replayer import ReplayerError
from freeagent.service import EpisodeStore, import_episode
from freeagent.service.edgeio import EdgeIOError
from freeagent.service.registry import EpisodeExistsError, _application_of
from freeagent.subjects import EpisodeSubjects, stream_name

APP = "demo"
ORIG = "orig"


def _write_log(path: Path, *, app: str = APP, episode_id: str = ORIG, count: int = 3) -> None:
    """Write a small recorder-compatible Parquet log for *count* public messages."""
    subjects = EpisodeSubjects(app=app, episode_id=episode_id)
    now = datetime.now(UTC)
    records = [
        make_record(
            stream_seq=i + 1,
            subject=subjects.public,
            received_at=now,
            data=Envelope(episode_id=episode_id, sender="alice", payload={"n": i}).to_bytes(),
            fallback_episode_id=episode_id,
        )
        for i in range(count)
    ]
    write_parquet(records, path)


async def test_import_creates_a_fresh_sealed_episode(tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log, count=3)
    transport = MemoryTransport()

    result = await import_episode(transport, parquet_path=log, episode_id="copy", name="archived")

    assert result.app == APP
    assert result.episode_id == "copy"
    assert result.name == "archived"

    stream = stream_name(APP, "copy")
    assert await transport.stream_sealed(stream) is True
    meta = EpisodeMetadata.from_stream_metadata(await transport.stream_metadata(stream))
    assert meta is not None
    assert meta.app == APP
    assert meta.mode == "replay"
    assert meta.status == "ended"
    assert meta.name == "archived"

    # It rejoins the browse surface: the durable store lists it.
    records = await EpisodeStore(transport, application_of=_application_of).list()
    assert [r.episode_id for r in records] == ["copy"]
    assert records[0].sealed is True


async def test_import_rewrites_subject_and_episode_id(tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log, count=2)
    transport = MemoryTransport()

    await import_episode(transport, parquet_path=log, episode_id="copy")

    new_root = EpisodeSubjects(app=APP, episode_id="copy").root
    published = [
        (subject, Envelope.from_bytes(data))
        for subject, data in transport.history
        if subject.startswith(new_root)
    ]
    assert len(published) == 2
    for subject, envelope in published:
        assert subject == f"{new_root}.public"  # subject rewritten to the new id
        assert envelope.episode_id == "copy"  # envelope id rewritten too
        assert envelope.sender == "alice"


async def test_import_assigns_an_id_when_omitted(tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    transport = MemoryTransport()

    result = await import_episode(transport, parquet_path=log)

    assert result.episode_id != ORIG  # a fresh id, not the recorded one
    assert stream_name(APP, result.episode_id) in transport.streams


async def test_import_into_a_taken_id_conflicts(tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    transport = MemoryTransport()
    # Pre-seed the target stream so the import collides.
    await transport.ensure_stream(stream_name(APP, "copy"), [f"{APP}.episode.copy.>"])

    with pytest.raises(EpisodeExistsError):
        await import_episode(transport, parquet_path=log, episode_id="copy")


async def test_import_of_empty_log_is_rejected(tmp_path: Path) -> None:
    log = tmp_path / "empty.parquet"
    write_parquet([], log)
    transport = MemoryTransport()

    with pytest.raises(EdgeIOError):
        await import_episode(transport, parquet_path=log)


async def test_import_of_unreadable_log_raises_replayer_error(tmp_path: Path) -> None:
    bad = tmp_path / "not.parquet"
    bad.write_text("this is not parquet")
    transport = MemoryTransport()

    with pytest.raises(ReplayerError):
        await import_episode(transport, parquet_path=bad)


# ---------------------------------------------------------------------------
# Slow: a real export -> import round trip over NATS (skipped without NATS)
# ---------------------------------------------------------------------------


def _nats_running() -> bool:
    try:
        with socket.create_connection(("127.0.0.1", 4222), timeout=1.0):
            return True
    except OSError:
        return False


async def test_export_then_import_round_trip(tmp_path: Path) -> None:
    import asyncio
    import uuid

    if not _nats_running():
        pytest.skip("NATS is not running")

    from collatz_app import EVEN, ODD, CollatzAgent, CollatzEnvironment

    from freeagent import EpisodeState, NatsTransport, default_nats_url
    from freeagent.service import export_episode

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
    assert state is EpisodeState.ENDED  # sealed

    log = tmp_path / "collatz.parquet"
    rows = await export_episode(nats_url=nats_url, app="collatz", episode_id=episode_id, output=log)
    assert rows > 0
    assert log.exists()

    transport = NatsTransport(nats_url)
    await transport.connect()
    try:
        result = await import_episode(transport, parquet_path=log)
        # The imported episode is a fresh, sealed stream that lists like any other.
        records = await EpisodeStore(transport, application_of=_application_of).list()
        ids = {r.episode_id for r in records}
        assert result.episode_id in ids
        assert any(r.episode_id == result.episode_id and r.sealed for r in records)
        await transport.delete_stream(stream_name("collatz", result.episode_id))
        await transport.delete_stream(stream_name("collatz", episode_id))
    finally:
        await transport.close()
