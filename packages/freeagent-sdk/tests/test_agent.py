"""Tests for the Free Agent SDK base Agent class.

These exercise the connect/subscribe/unsubscribe/disconnect lifecycle against a
fake NATS client, so no live server is required.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable

import pytest
from freeagent.sdk import Agent
from freeagent.sdk.agent import Message, MessageType
from nats.aio.msg import Msg

Handler = Callable[[Msg], Awaitable[None]]


class FakeSubscription:
    """Records whether it was unsubscribed."""

    def __init__(self, subject: str, queue: str, cb: Handler) -> None:
        self.subject = subject
        self.queue = queue
        self.cb = cb
        self.unsubscribed = False

    async def unsubscribe(self, limit: int = 0) -> None:
        self.unsubscribed = True


class FakeClient:
    """A stand-in for nats.aio.client.Client that records interactions."""

    def __init__(self) -> None:
        self.subscriptions: list[FakeSubscription] = []
        self.closed = False

    async def subscribe(
        self, subject: str, queue: str = "", cb: Handler | None = None
    ) -> FakeSubscription:
        assert cb is not None
        sub = FakeSubscription(subject, queue, cb)
        self.subscriptions.append(sub)
        return sub

    async def close(self) -> None:
        self.closed = True


@pytest.fixture
def fake_connect(monkeypatch: pytest.MonkeyPatch) -> Callable[[], FakeClient]:
    """Patch nats.connect to hand out a FakeClient, and expose the latest one."""
    created: list[FakeClient] = []

    async def _connect(servers: object, **options: object) -> FakeClient:
        client = FakeClient()
        created.append(client)
        return client

    monkeypatch.setattr("freeagent.sdk.agent.nats.connect", _connect)
    return lambda: created[-1]


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload."""

    def __init__(self, data: bytes) -> None:
        self.data = data


async def test_callback_decodes_payload_onto_queue() -> None:
    agent = Agent("worker", "jobs")
    await agent.callback(FakeMsg(b'{"type": "PERCEPTION"}'))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 1
    message = agent.queue.get_nowait()
    assert message == Message(type=MessageType.PERCEPTION)


async def test_callback_rejects_invalid_payload() -> None:
    agent = Agent("worker", "jobs")
    with pytest.raises(ValueError):
        await agent.callback(FakeMsg(b'{"type": "NONSENSE"}'))  # type: ignore[arg-type]
    assert agent.queue.empty()


async def test_start_connects_and_subscribes(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = Agent("worker", "jobs", "events")
    await agent.start()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == ["jobs", "events"]
    # Each subscription uses the agent name as the queue group and the agent's
    # callback as the handler.
    for sub in client.subscriptions:
        assert sub.queue == "worker"
        assert sub.cb == agent.callback


async def test_stop_unsubscribes_and_disconnects(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = Agent("worker", "jobs", "events")
    await agent.start()
    client = fake_connect()

    await agent.stop()

    assert all(s.unsubscribed for s in client.subscriptions)
    assert client.closed is True
    assert agent.subscriptions == []
    assert agent.client is None


async def test_stop_without_start_is_safe() -> None:
    agent = Agent("worker", "jobs")
    # Never started: no client, no subscriptions. stop() must not raise.
    await agent.stop()
