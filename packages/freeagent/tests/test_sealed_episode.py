"""Named, sealed, JetStream-resident episodes (ADR-0003 / #40).

Two tiers, mirroring the rest of the suite:

* **Fast** tests on the in-memory transport: sealing rejects appends, stream
  metadata round-trips and merges, and a full episode driven on
  :class:`MemoryTransport` creates -> starts -> seals with the right status and
  friendly-name metadata.
* **Slow** tests (skipped without NATS) drive a real episode over JetStream and
  assert the server-side seal and metadata.
"""

from __future__ import annotations

import asyncio
import socket
import uuid
from typing import TYPE_CHECKING, Any

import pytest
from collatz_app import EVEN, ODD, CollatzAgent, CollatzEnvironment
from freeagent_testing import connected_transport, publish

from freeagent import (
    Agent,
    Envelope,
    Environment,
    EpisodeMetadata,
    EpisodeState,
    MemoryTransport,
    SealedStreamError,
    default_nats_url,
    fallback_episode_name,
)
from freeagent.metadata import KEY_NAME, KEY_STATUS
from freeagent.subjects import EpisodeSubjects, stream_name

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

APP = "demo"
EPISODE = "e1"
STREAM = stream_name(APP, EPISODE)
ALL = f"{APP}.episode.{EPISODE}.>"
PUBLIC = f"{APP}.episode.{EPISODE}.public"

FAST = {"setup_timeout": 0.5, "episode_timeout": 5.0, "grace_period": 0.05}
AGENT_FAST = {"grace_period": 0.02}


# ---------------------------------------------------------------------------
# Transport-level sealing and metadata (in-memory)
# ---------------------------------------------------------------------------


async def test_open_stream_accepts_appends_sealed_one_rejects() -> None:
    transport = MemoryTransport()
    await transport.ensure_stream(STREAM, [ALL])
    await transport.publish(PUBLIC, b"first")  # open: accepted
    assert await transport.stream_sealed(STREAM) is False

    await transport.seal_stream(STREAM)
    assert await transport.stream_sealed(STREAM) is True
    with pytest.raises(SealedStreamError):
        await transport.publish(PUBLIC, b"after-seal")


async def test_stream_metadata_roundtrips_and_merges() -> None:
    transport = MemoryTransport()
    meta = EpisodeMetadata(app=APP, name="brave-otter", status="setup").to_stream_metadata()
    await transport.ensure_stream(STREAM, [ALL], metadata=meta)
    assert (await transport.stream_metadata(STREAM))[KEY_NAME] == "brave-otter"

    await transport.set_stream_metadata(STREAM, {KEY_STATUS: "ended"})
    stored = await transport.stream_metadata(STREAM)
    decoded = EpisodeMetadata.from_stream_metadata(stored)
    assert decoded is not None
    assert decoded.status == "ended"  # merged
    assert decoded.name == "brave-otter"  # preserved


def test_from_stream_metadata_ignores_foreign_streams() -> None:
    assert EpisodeMetadata.from_stream_metadata({"some_other": "value"}) is None


# ---------------------------------------------------------------------------
# A full episode on the in-memory transport: create -> start -> seal
# ---------------------------------------------------------------------------


async def _run(
    env: Environment, agents: Sequence[Agent], transport: MemoryTransport, deadline_s: float = 5.0
) -> EpisodeState:
    agent_tasks = [asyncio.create_task(agent.serve(transport)) for agent in agents]
    try:
        state = await asyncio.wait_for(env.serve(transport), timeout=deadline_s)
        await asyncio.wait_for(asyncio.gather(*agent_tasks), timeout=deadline_s)
        return state
    finally:
        for task in agent_tasks:
            task.cancel()


class _NamingEnv(Environment):
    """Ends on an agent's 'done' signal and contributes a friendly name."""

    def __init__(
        self,
        app: str,
        roster: Sequence[str],
        episode_id: str | None = None,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(app, roster, episode_id, config)
        self.contribute_name: str | None = None

    async def perceive(self, message: Envelope) -> None:
        if message.payload == "done":
            if self.contribute_name is not None:
                self.set_name(self.contribute_name)
            self.initiate_shutdown("agent signal")


class _SignalingAgent(Agent):
    """Signals 'the episode is over' to the env inbox as soon as it starts."""

    async def control(self, message: Envelope) -> None:
        await super().control(message)
        if isinstance(message.payload, dict) and message.payload.get("type") == "start":
            self.act("done", recipients=["env"])


async def test_episode_seals_with_status_metadata_on_end() -> None:
    transport = await connected_transport()
    env = _NamingEnv(APP, ["alice"], episode_id=EPISODE, config=FAST)
    agents = [_SignalingAgent(env.subject_root, "alice", config=AGENT_FAST)]

    state = await _run(env, agents, transport)

    assert state is EpisodeState.ENDED
    assert await transport.stream_sealed(STREAM) is True
    meta = EpisodeMetadata.from_stream_metadata(await transport.stream_metadata(STREAM))
    assert meta is not None
    assert meta.app == APP
    assert meta.status == "ended"
    assert meta.mode == "live"
    assert meta.created_at  # an ISO timestamp was stamped at creation
    # No application name was contributed, so the fallback stands.
    assert meta.name == fallback_episode_name(EPISODE)


async def test_app_contributed_name_lands_in_metadata() -> None:
    transport = await connected_transport()
    env = _NamingEnv(APP, ["alice"], episode_id=EPISODE, config=FAST)
    env.contribute_name = "elephant"
    agents = [_SignalingAgent(env.subject_root, "alice", config=AGENT_FAST)]

    await _run(env, agents, transport)

    meta = EpisodeMetadata.from_stream_metadata(await transport.stream_metadata(STREAM))
    assert meta is not None
    assert meta.name == "elephant"


async def test_sealed_episode_rejects_late_publish() -> None:
    transport = await connected_transport()
    env = _NamingEnv(APP, ["alice"], episode_id=EPISODE, config=FAST)
    agents = [_SignalingAgent(env.subject_root, "alice", config=AGENT_FAST)]
    await _run(env, agents, transport)

    # The episode is sealed; an append on any of its subjects is rejected.
    with pytest.raises(SealedStreamError):
        await publish(transport, PUBLIC, "latecomer", "anything")


async def test_aborted_episode_seals_with_aborted_outcome() -> None:
    transport = await connected_transport()
    # An empty roster starts immediately; the operator aborts it.
    env = Environment(APP, [], episode_id=EPISODE, config=FAST)

    async def abort_soon() -> None:
        await asyncio.sleep(0.05)
        env.initiate_abort("operator stop")

    aborter = asyncio.create_task(abort_soon())
    state = await asyncio.wait_for(env.serve(transport), timeout=5.0)
    await aborter

    assert state is EpisodeState.ABORTED
    assert await transport.stream_sealed(STREAM) is True
    meta = EpisodeMetadata.from_stream_metadata(await transport.stream_metadata(STREAM))
    assert meta is not None
    assert meta.status == "aborted"
    assert meta.outcome == "aborted"


# ---------------------------------------------------------------------------
# Real JetStream: server-side seal + metadata (skipped without NATS)
# ---------------------------------------------------------------------------

NATS_URL = default_nats_url()


def _nats_running() -> bool:
    try:
        with socket.create_connection(("127.0.0.1", 4222), timeout=1.0):
            return True
    except OSError:
        return False


@pytest.mark.skipif(not _nats_running(), reason="NATS is not running")
async def test_real_episode_seals_stream_and_writes_metadata() -> None:
    import nats

    episode_id = uuid.uuid4().hex  # persistent volume: never reuse an id
    config = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.3}
    env = CollatzEnvironment("collatz", (EVEN, ODD), episode_id=episode_id, config=config)
    agents = [
        CollatzAgent(env.subject_root, EVEN, config={"grace_period": 0.1}),
        CollatzAgent(
            env.subject_root, ODD, config={"grace_period": 0.1, "starter": True, "seed": 6}
        ),
    ]
    state, *_ = await asyncio.wait_for(
        asyncio.gather(env.run(NATS_URL), *(agent.run(NATS_URL) for agent in agents)),
        timeout=20.0,
    )
    assert state is EpisodeState.ENDED

    subjects = EpisodeSubjects(app="collatz", episode_id=episode_id)
    client = await nats.connect(NATS_URL)
    try:
        js = client.jetstream()
        info = await js.stream_info(subjects.stream)
        assert info.config.sealed is True
        meta = EpisodeMetadata.from_stream_metadata(dict(info.config.metadata or {}))
        assert meta is not None
        assert meta.status == "ended"
        assert meta.app == "collatz"
        # A publish into the sealed stream is rejected server-side.
        with pytest.raises(Exception):  # noqa: B017, PT011 -- any JetStream rejection is fine
            await js.publish(subjects.public, b"after-seal")
    finally:
        await client.close()
