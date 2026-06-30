"""Tests for the Free Agent SDK base Agent class.

These exercise the connect/subscribe/unsubscribe/disconnect lifecycle against a
fake NATS client, so no live server is required.
"""

from __future__ import annotations

import asyncio
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


class RecordingAgent(Agent):
    """A concrete agent whose process() records the messages it handles."""

    def __init__(self, name: str, *subjects: str) -> None:
        super().__init__(name, *subjects)
        self.processed: list[Message] = []

    async def process(self, message: Message) -> None:
        self.processed.append(message)


def test_agent_is_abstract() -> None:
    with pytest.raises(TypeError):
        Agent("a", "subject")  # type: ignore[abstract]


async def test_callback_decodes_payload_onto_queue() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.callback(FakeMsg(b'{"type": "PERCEPTION"}'))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 1
    message = agent.queue.get_nowait()
    assert message == Message(type=MessageType.PERCEPTION)


async def test_callback_rejects_invalid_payload() -> None:
    agent = RecordingAgent("worker", "jobs")
    with pytest.raises(ValueError):
        await agent.callback(FakeMsg(b'{"type": "NONSENSE"}'))  # type: ignore[arg-type]
    assert agent.queue.empty()


async def test_run_processes_queued_messages_in_order() -> None:
    agent = RecordingAgent("worker", "jobs")
    sent = [Message(type=t) for t in (MessageType.PERCEPTION, MessageType.THOUGHT)]
    for message in sent:
        agent.queue.put_nowait(message)

    task = asyncio.create_task(agent.run())
    await agent.queue.join()  # wait until every queued message is processed
    task.cancel()

    assert agent.processed == sent


async def test_run_is_cancellable() -> None:
    agent = RecordingAgent("worker", "jobs")
    task = asyncio.create_task(agent.run())

    # With an empty queue the loop is parked on queue.get(); cancelling ends it.
    await asyncio.sleep(0)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task


async def test_run_marks_task_done_even_when_process_raises() -> None:
    class Boom(Agent):
        async def process(self, message: Message) -> None:
            raise ValueError("boom")

    agent = Boom("worker", "jobs")
    agent.queue.put_nowait(Message(type=MessageType.EVENT))

    task = asyncio.create_task(agent.run())
    # run() re-raises out of the loop, but task_done must still have fired so
    # join() does not hang.
    with pytest.raises(ValueError, match="boom"):
        await task
    await asyncio.wait_for(agent.queue.join(), timeout=1)


async def test_start_connects_subscribes_and_launches_run_loop(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs", "events")
    await agent.start()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == ["jobs", "events"]
    # Each subscription uses the agent name as the queue group and the agent's
    # callback as the handler.
    for sub in client.subscriptions:
        assert sub.queue == "worker"
        assert sub.cb == agent.callback

    # start() launches the run loop as a background task that is still running.
    assert agent.task is not None
    assert not agent.task.done()

    await agent.stop()


async def test_stop_cancels_run_loop_unsubscribes_and_disconnects(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs", "events")
    await agent.start()
    client = fake_connect()
    task = agent.task
    assert task is not None

    await agent.stop()

    # The run loop is stopped and its handle cleared.
    assert task.cancelled()
    assert agent.task is None
    # NATS is torn down.
    assert all(s.unsubscribed for s in client.subscriptions)
    assert client.closed is True
    assert agent.subscriptions == []
    assert agent.client is None


async def test_stop_drains_a_started_agents_messages() -> None:
    # End to end: a message delivered via callback is processed by the run loop
    # that start() launches, then stop() shuts everything down.
    agent = RecordingAgent("worker")
    agent.task = asyncio.create_task(agent.run())
    await agent.callback(FakeMsg(b'{"type": "EVENT"}'))  # type: ignore[arg-type]

    await agent.queue.join()
    assert agent.processed == [Message(type=MessageType.EVENT)]

    await agent.stop()
    assert agent.task is None


async def test_stop_without_start_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. stop() must not raise.
    await agent.stop()
