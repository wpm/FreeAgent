"""Tests for :class:`freeagent.sdk.entity.Environment`.

These pin the subject-naming contract between :class:`~freeagent.sdk.entity.Environment` and
:class:`~freeagent.sdk.entity.Agent`: :meth:`Environment.broadcast_to_agents` must send to exactly
the subjects an :class:`Agent` subscribes to. Expected subjects are built from the same ``AGENTS``
/ ``ENVIRONMENT`` constants the implementation uses on the *Agent* side, so a send/subscribe
divergence (e.g. dropping the ``agents.`` segment on the send side) fails a test here rather than
both drifting together.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable

import pytest
from fixtures import FakeClient
from freeagent.sdk.entity import AGENTS, ENVIRONMENT, Agent, Environment
from freeagent.sdk.message import Ack, Message, StartEntity, StopEntity


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

    async def hangs(subject: str, payload: bytes, **_: object) -> None:
        await asyncio.sleep(10)

    client.request = hangs  # type: ignore[assignment]

    with pytest.raises(TimeoutError):
        await env.stop()

    assert client.closed is True
    assert env.client is None


async def test_environment_subscribes_under_environment_subject(
    fake_connect: Callable[[], FakeClient],
) -> None:
    env = Environment("episode-root", "alice")

    await env.start()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == [f"episode-root.{ENVIRONMENT}"]
