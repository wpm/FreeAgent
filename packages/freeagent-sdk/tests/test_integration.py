"""Integration tests exercising the SDK against a real ``nats-server`` subprocess.

Unlike the unit tests, which drive the SDK through the ``FakeClient`` stand-in, these connect real
:class:`~freeagent.sdk.entity.Entity` instances to an actual server started by the ``nats_server``
fixture (see ``conftest.py`` and ``nats_server.py``). They cover what a fake can't credibly fake:
real subject matching, real async request/reply delivery, and — the acceptance criterion that
motivates a *shared* server with *per-test* subject roots — that two entities rooted under
different ``episode_root`` subjects cannot see each other's messages.

Every test here carries the :func:`pytest.mark.integration` marker, so ``pytest -m "not
integration"`` runs the whole unit suite without needing a NATS binary at all.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable

import pytest
from fixtures import Ping
from freeagent.sdk import Agent
from freeagent.sdk.entity import Entity, Environment
from freeagent.sdk.message import Ack, Message, StartEntity

pytestmark = pytest.mark.integration


class RecordingAgent(Agent):
    """An agent that records every message :meth:`process_message` handles, for assertions.

    Records into an :class:`asyncio.Queue` so a test can ``await`` the next processed message with a
    timeout rather than polling, keeping the test's wait bounded by real delivery latency.
    """

    def __init__(self, episode_root: str, name: str, servers: str) -> None:
        super().__init__(episode_root, name, servers=servers)
        self.processed: asyncio.Queue[Message] = asyncio.Queue()

    async def process_message(self, message: Message) -> Message | None:
        await self.processed.put(message)
        return None


async def _eventually(condition: Callable[[], bool], timeout: float = 5.0) -> None:
    """Poll ``condition`` until it holds or ``timeout`` elapses, yielding to the loop between tries.

    For asserting on state a counterpart entity settles asynchronously — e.g. an agent clearing its
    client after it Acks a stop — where waiting is preferable to a fixed sleep or a lost race.

    :param condition: Called repeatedly; polling stops as soon as it returns ``True``.
    :param timeout: Seconds to keep polling before returning regardless (the caller then asserts).
    """
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not condition() and loop.time() < deadline:
        await asyncio.sleep(0.01)


async def test_start_stop_lifecycle_round_trips_through_a_real_server(
    nats_server: str, episode_root: str
) -> None:
    """A real Environment/Agent start+stop exchange completes over an actual NATS server.

    ``Environment.start`` broadcasts ``StartEntity`` and waits for the agent's ``Ack``; ``stop``
    does the same with ``StopEntity``. Both completing without timing out proves the request/reply
    path — subject routing, JSON encode/decode, and the ack — works end to end against the binary.
    """
    agent = RecordingAgent(episode_root, "worker", nats_server)
    await agent.start()
    env = Environment(episode_root, "worker", servers=nats_server, timeout=5.0)

    await env.start()
    assert agent.task is not None  # StartEntity launched the run loop.

    # env.stop() broadcasts StopEntity and waits for the agent's Ack; it returning without a
    # timeout is the round trip completing. The agent handles StopEntity by cancelling its run loop
    # and clearing the task reference before it Acks, so once stop() returns the task is gone.
    await env.stop()
    await _eventually(lambda: agent.task is None)
    assert agent.task is None


async def test_a_plain_message_is_delivered_and_processed(
    nats_server: str, episode_root: str
) -> None:
    """A plain (non-command) message broadcast to an agent reaches ``process_message``.

    Exercises the in-domain path: the message is queued by ``handle_incoming_message`` and drained
    by the run loop into ``process_message``, whose reply the broadcast awaits.
    """
    agent = RecordingAgent(episode_root, "worker", nats_server)
    await agent.start()
    env = Environment(episode_root, "worker", servers=nats_server, timeout=5.0)
    await env.start()
    try:
        [reply] = await env.broadcast_to_agents(Ping(label="hello"))

        assert Message.model_validate_json(reply.data) == Ack()
        processed = await asyncio.wait_for(agent.processed.get(), timeout=5.0)
        assert processed == Ping(label="hello")
    finally:
        await env.stop()


async def test_two_episode_roots_cannot_see_each_others_messages(nats_server: str) -> None:
    """Two entities on distinct ``episode_root`` subjects, on the *same* server, are isolated.

    This is the acceptance criterion for per-test subject-root isolation: the ``nats_server``
    fixture is shared across tests, so tests must be kept apart by their ``episode_root`` alone. A
    message published under one root must never reach a subscriber under another. Here two agents
    named identically live under two different roots; a message sent to the first root's agent is
    processed only there, and the second agent's queue stays empty.
    """
    alpha_agent = RecordingAgent("episode.alpha", "worker", nats_server)
    beta_agent = RecordingAgent("episode.beta", "worker", nats_server)
    await alpha_agent.start()
    await beta_agent.start()

    sender = Entity(nats_server, "episode.alpha")
    try:
        await sender.start()
        # Launch each agent's run loop so queued plain messages are drained into process_message.
        await sender.request("episode.alpha.agents.worker", StartEntity(), timeout=5.0)
        await sender.request("episode.beta.agents.worker", StartEntity(), timeout=5.0)

        # A plain message addressed to alpha's worker; its Ack reply confirms alpha processed it.
        reply = await sender.request(
            "episode.alpha.agents.worker", Ping(label="for-alpha"), timeout=5.0
        )
        assert Message.model_validate_json(reply.data) == Ack()

        processed = await asyncio.wait_for(alpha_agent.processed.get(), timeout=5.0)
        assert processed == Ping(label="for-alpha")

        # The other root's agent must never have seen it. Give delivery a real chance to (wrongly)
        # happen before concluding it didn't.
        await asyncio.sleep(0.2)
        assert beta_agent.processed.empty()
    finally:
        await sender.stop()
        await alpha_agent.stop()
        await beta_agent.stop()


async def test_episode_root_fixture_is_unique_per_test(episode_root: str) -> None:
    """The ``episode_root`` fixture embeds the test's node id, so each test gets its own root.

    A companion to the isolation test: it pins *why* two tests don't collide — the fixture derives
    from ``request.node.nodeid``, which is unique per test — without needing two live servers.
    """
    assert episode_root.startswith("episode.")
    assert "test_episode_root_fixture_is_unique_per_test" in episode_root
