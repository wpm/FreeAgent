"""Collatz integration test: the full stack over a real NATS + JetStream server.

No LLM, no fakes: ``Environment.run`` and ``Agent.run`` against
``nats://localhost:4222`` -- coordinated startup with presence confirmation,
the ``start`` control broadcast, in-world messaging through the fold and the
outbox, the think queue, the env-inbox shutdown signal, the stopping grace
period, and recorder-compatible logging. After the episode ends, the
episode's JetStream stream is read back from sequence 1 and the full message
sequence is asserted.

Skips cleanly when NATS is unreachable.
"""

from __future__ import annotations

import asyncio
import socket
import uuid
from typing import TYPE_CHECKING, Any
from urllib.parse import urlparse

import nats
import pytest
from collatz_app import (
    EVEN,
    ODD,
    CollatzAgent,
    CollatzEnvironment,
    expected_senders,
    trajectory,
)
from nats.js.api import ConsumerConfig, DeliverPolicy

from freeagent import ENV_NAME, Envelope, EpisodeState, default_nats_url

if TYPE_CHECKING:
    from nats.aio.msg import Msg

    from freeagent import EpisodeSubjects

NATS_URL = default_nats_url()
_PARSED_NATS = urlparse(NATS_URL)
NATS_HOST = _PARSED_NATS.hostname or "localhost"
NATS_PORT = _PARSED_NATS.port or 4222
SKIP_REASON = (
    "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
)

APP = "collatz"
SEED = 6
ROSTER = (EVEN, ODD)
ENV_CONFIG: dict[str, Any] = {"setup_timeout": 10.0, "episode_timeout": 30.0, "grace_period": 0.3}
AGENT_CONFIG: dict[str, Any] = {"grace_period": 0.1}


def _nats_running() -> bool:
    """Short-timeout TCP probe: is anything listening on the NATS port?"""
    try:
        with socket.create_connection((NATS_HOST, NATS_PORT), timeout=1.0):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(not _nats_running(), reason=SKIP_REASON)


async def read_episode_stream(subjects: EpisodeSubjects) -> list[tuple[str, Envelope]]:
    """Read the episode's whole JetStream stream back, in stream order.

    Ordered consumer from sequence 1; every message must parse as a valid
    :class:`Envelope` (exactly what the recorder relies on).
    """
    client = await nats.connect(NATS_URL)
    try:
        js = client.jetstream()
        info = await js.stream_info(subjects.stream)
        total = info.state.messages
        messages: list[tuple[str, Envelope]] = []
        complete = asyncio.Event()

        async def callback(msg: Msg) -> None:
            messages.append((msg.subject, Envelope.from_bytes(msg.data)))
            if len(messages) >= total:
                complete.set()

        subscription = await js.subscribe(
            subjects.all_subjects,
            cb=callback,
            ordered_consumer=True,
            config=ConsumerConfig(deliver_policy=DeliverPolicy.ALL),
        )
        await asyncio.wait_for(complete.wait(), timeout=10.0)
        await subscription.unsubscribe()
        return messages
    finally:
        await client.close()


async def test_collatz_episode_full_message_sequence() -> None:
    episode_id = uuid.uuid4().hex  # the NATS volume is persistent: never reuse an id
    env = CollatzEnvironment(APP, ROSTER, episode_id=episode_id, config=ENV_CONFIG)
    agents = [
        CollatzAgent(env.subject_root, EVEN, config=AGENT_CONFIG),
        # Seed 6 is even, so the odd agent fires the kickoff: the responsible
        # (even) agent must perceive it, and no agent perceives its own send.
        CollatzAgent(env.subject_root, ODD, config={**AGENT_CONFIG, "starter": True, "seed": SEED}),
    ]

    state, *_ = await asyncio.wait_for(
        asyncio.gather(env.run(NATS_URL), *(agent.run(NATS_URL) for agent in agents)),
        timeout=15.0,
    )

    assert state is EpisodeState.ENDED
    assert env.state_history == [
        EpisodeState.SETUP,
        EpisodeState.RUNNING,
        EpisodeState.STOPPING,
        EpisodeState.ENDED,
    ]
    assert env.shutdown_reason == "collatz reached 1"

    s = env.subjects
    messages = await read_episode_stream(s)

    # Recorder compatibility: every wire message parsed as a valid Envelope
    # (read_episode_stream already did), carries this episode's id, and lives
    # under the episode's subject root.
    for subject, envelope in messages:
        assert envelope.episode_id == episode_id
        assert subject.startswith(f"{s.root}.")

    numbers = trajectory(SEED)
    senders = expected_senders(SEED, starter=ODD)
    assert numbers == [6, 3, 10, 5, 16, 8, 4, 2, 1]
    # 2 presence requests + 2 replies + start + the chain + done + shutdown + 2 byes
    assert len(messages) == 9 + len(numbers)

    # --- Setup (messages 0-3): two presence requests and two replies. The
    # requests are published sequentially (roster order) and each reply
    # follows its own request, but a reply may interleave before the second
    # request -- assert only the order that is pinned.
    setup = messages[:4]
    requests = [
        (i, subject, envelope)
        for i, (subject, envelope) in enumerate(setup)
        if envelope.payload.get("type") == "freeagent.presence_request"
    ]
    replies = [
        (i, subject, envelope)
        for i, (subject, envelope) in enumerate(setup)
        if envelope.payload.get("type") == "freeagent.presence_reply"
    ]
    assert len(requests) == 2
    assert len(replies) == 2
    # Requests: sender env, one per roster member, inbox subjects in roster order.
    assert [subject for _, subject, _ in requests] == [s.agent(name) for name in ROSTER]
    assert all(envelope.sender == ENV_NAME for _, _, envelope in requests)
    request_index = {
        envelope.payload["request_id"]: (i, name)
        for (i, _, envelope), name in zip(requests, ROSTER, strict=True)
    }
    # Replies: each on its episode-scoped reply subject, from the requested
    # agent, after its request; both roster members replied.
    for i, subject, envelope in replies:
        request_id = envelope.payload["request_id"]
        request_i, name = request_index[request_id]
        assert i > request_i
        assert subject == s.reply(request_id)
        assert envelope.sender == name
        assert envelope.payload["agent"] == name
    assert {envelope.sender for _, _, envelope in replies} == set(ROSTER)

    # --- The gun: control start, after all presence replies.
    subject, envelope = messages[4]
    assert subject == s.control
    assert envelope.sender == ENV_NAME
    assert envelope.payload == {"type": "start"}

    # --- The Collatz chain: exact values, exact senders, exact stream order,
    # all broadcast on the public subject.
    chain = messages[5 : 5 + len(numbers)]
    assert [(subject, envelope.sender, envelope.payload) for subject, envelope in chain] == [
        (s.public, sender, {"type": "number", "value": value})
        for value, sender in zip(numbers, senders, strict=True)
    ]

    # --- The done signal on the environment's inbox, from whoever computed 1.
    subject, envelope = messages[5 + len(numbers)]
    assert subject == s.env
    assert envelope.sender == senders[-1]
    assert envelope.payload == {"type": "done", "final": 1}

    # --- The decision: control shutdown.
    subject, envelope = messages[6 + len(numbers)]
    assert subject == s.control
    assert envelope.sender == ENV_NAME
    assert envelope.payload == {"type": "shutdown"}

    # --- Trailing goodbyes land AFTER the shutdown broadcast, inside the
    # stopping grace period; their relative order is not pinned.
    byes = messages[7 + len(numbers) :]
    assert all(
        subject == s.public and envelope.payload == {"type": "bye"} for subject, envelope in byes
    )
    assert {envelope.sender for _, envelope in byes} == set(ROSTER)


async def test_missing_agent_aborts_episode() -> None:
    episode_id = uuid.uuid4().hex
    env = CollatzEnvironment(
        APP, ROSTER, episode_id=episode_id, config={**ENV_CONFIG, "setup_timeout": 0.5}
    )
    even = CollatzAgent(env.subject_root, EVEN, config=AGENT_CONFIG)  # odd never launches

    state, _ = await asyncio.wait_for(
        asyncio.gather(env.run(NATS_URL), even.run(NATS_URL)), timeout=10.0
    )

    assert state is EpisodeState.ABORTED
    assert env.state_history == [EpisodeState.SETUP, EpisodeState.ABORTED]
    assert env.abort_reason is not None
    assert ODD in env.abort_reason
