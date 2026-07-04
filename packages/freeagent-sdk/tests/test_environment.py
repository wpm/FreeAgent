"""Tests for :class:`freeagent.sdk.entity.Environment`.

These pin the subject-naming contract between :class:`~freeagent.sdk.entity.Environment` and
:class:`~freeagent.sdk.entity.Agent`: :meth:`Environment.broadcast_to_agents` must send to exactly
the subjects an :class:`Agent` subscribes to. Expected subjects are built from the same ``AGENTS`` /
``ENVIRONMENT`` constants the implementation uses on the *Agent* side, so a send/subscribe
divergence (e.g. dropping the ``agents.`` segment on the send side) fails a test here rather than
both drifting together.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable

import pytest
from fixtures import FakeClient, FakeMsg
from freeagent.sdk.entity import AGENTS, ENVIRONMENT, UNBOUNDED_TIMEOUT, Agent, Environment
from freeagent.sdk.message import (
    Ack,
    EpisodeComplete,
    Message,
    StartEntity,
    StopAgent,
    StopEntity,
)


def agent_subject(episode_root: str, name: str) -> str:
    """The subject an Agent with this episode_root and name subscribes to, per Agent.__init__."""
    return f"{episode_root}.{AGENTS}.{name}"


async def test_broadcast_to_agents_sends_to_exactly_the_subjects_agents_subscribe_to(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()

    subjects = [subject for subject, _ in client.requests]

    assert subjects == [
        agent_subject("episode-root", "alice"),
        agent_subject("episode-root", "bob"),
    ]
    # Cross-check against subjects real Agent instances would actually subscribe to.
    assert subjects == [Agent("episode-root", name).subjects[0] for name in ("alice", "bob")]


async def test_broadcast_to_agents_postfix_is_appended_to_each_agent_subject(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()
    client.requests.clear()

    await env.broadcast_to_agents(Ack(), postfix="events")

    subjects = [subject for subject, _ in client.requests]
    assert subjects == [
        f"{agent_subject('episode-root', 'alice')}.events",
        f"{agent_subject('episode-root', 'bob')}.events",
    ]


async def test_start_broadcasts_start_entity_to_every_agent(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")

    await env.start()

    client = fake_connect()
    assert [subject for subject, _ in client.requests] == [
        agent_subject("episode-root", "alice"),
        agent_subject("episode-root", "bob"),
    ]
    for _, payload in client.requests:
        assert Message.model_validate_json(payload) == StartEntity()


async def test_stop_broadcasts_stop_entity_to_every_agent(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()
    client.requests.clear()

    await env.stop()

    assert [subject for subject, _ in client.requests] == [
        agent_subject("episode-root", "alice"),
        agent_subject("episode-root", "bob"),
    ]
    for _, payload in client.requests:
        assert Message.model_validate_json(payload) == StopEntity()


async def test_stop_disconnects_even_when_the_broadcast_times_out(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", timeout=0.01)
    await env.start()
    client = fake_connect()

    async def hangs(_: str, __: bytes, **___: object) -> None:
        await asyncio.sleep(10)

    client.request = hangs  # type: ignore[assignment]

    with pytest.raises(TimeoutError):
        await env.stop()

    assert client.closed is True
    assert env.client is None


async def test_broadcast_requests_carry_the_broadcast_timeout(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob", timeout=0.25)
    await env.start()
    client = fake_connect()

    # Each broadcast request is bounded by the broadcast timeout, never nats-py's silent default.
    assert client.request_timeouts == [0.25, 0.25]


async def test_broadcast_requests_are_unbounded_when_timeout_is_none(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice")
    await env.start()
    client = fake_connect()

    # Default broadcast timeout is None (wait indefinitely); the request is given the unbounded
    # sentinel value rather than nats-py's default.
    assert client.request_timeouts == [UNBOUNDED_TIMEOUT]


async def test_environment_subscribes_under_environment_subject(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice")

    await env.start()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == [f"episode-root.{ENVIRONMENT}"]


async def test_stop_agent_sends_stop_agent_to_exactly_the_targeted_agent(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()
    client.requests.clear()

    await env.stop_agent("alice")

    # Exactly the one targeted agent's subject is requested; bob (still running) is untouched.
    assert [subject for subject, _ in client.requests] == [agent_subject("episode-root", "alice")]
    for _, payload in client.requests:
        assert Message.model_validate_json(payload) == StopAgent()


async def test_stop_agent_records_the_stopped_agent(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()

    await env.stop_agent("alice")

    assert env.stopped_agents == {"alice"}


async def test_stop_agent_is_idempotent(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()
    client.requests.clear()

    await env.stop_agent("alice")
    await env.stop_agent("alice")

    # The second stop of an already-stopped agent sends nothing, matching agent-side idempotency.
    assert [subject for subject, _ in client.requests] == [agent_subject("episode-root", "alice")]
    assert env.stopped_agents == {"alice"}


async def test_stop_agent_request_carries_the_broadcast_timeout(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob", timeout=0.25)
    await env.start()
    client = fake_connect()
    client.request_timeouts.clear()

    await env.stop_agent("alice")

    # The targeted request is bounded exactly like a broadcast request, never nats-py's default.
    assert client.request_timeouts == [0.25]


async def test_stop_publishes_episode_complete_on_the_environment_subject(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()

    await env.stop()

    assert [subject for subject, _ in client.published] == [f"episode-root.{ENVIRONMENT}"]
    ((_, payload),) = client.published
    assert Message.model_validate_json(payload) == EpisodeComplete()


async def test_stop_publishes_episode_complete_exactly_once(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice", "bob")
    await env.start()
    client = fake_connect()

    await env.stop()

    episode_completes = [
        payload
        for _, payload in client.published
        if isinstance(Message.model_validate_json(payload), EpisodeComplete)
    ]
    assert len(episode_completes) == 1


async def test_stop_emits_episode_complete_before_broadcasting_stop_entity(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # The end marker must precede teardown so an observer sees the definite end of the episode ahead
    # of the agents stopping. Record the interleaving of publishes and requests as they happen.
    env = Environment("episode-root", "alice")
    await env.start()
    client = fake_connect()

    order: list[str] = []
    real_publish = client.publish
    real_request = client.request

    async def recording_publish(subject: str, payload: bytes) -> None:
        order.append("publish")
        await real_publish(subject, payload)

    async def recording_request(
        subject: str, payload: bytes, timeout: float = 0.5, **kwargs: object
    ) -> FakeMsg:
        order.append("request")
        return await real_request(subject, payload, timeout=timeout, **kwargs)

    client.publish = recording_publish  # type: ignore[method-assign]
    client.request = recording_request  # type: ignore[method-assign]

    await env.stop()

    assert order == ["publish", "request"]
