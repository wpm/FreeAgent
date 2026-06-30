"""Tests for the Free Agent SDK base Agent class.

These exercise the connect/subscribe/unsubscribe/disconnect lifecycle against a
fake NATS client, so no live server is required.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import pytest
from freeagent.sdk import Agent
from freeagent.sdk.agent import STOP_AGENT, Message
from nats.aio.msg import Msg

Handler = Callable[[Msg], Awaitable[None]]


class FakeSubscription:
    """Records whether it was unsubscribed."""

    def __init__(self, subject: str, queue: str, cb: Handler) -> None:
        self.subject = subject
        self.queue = queue
        self.cb = cb
        self.unsubscribed = False

    async def unsubscribe(self) -> None:
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

    async def _connect(_, **__) -> FakeClient:
        client = FakeClient()
        created.append(client)
        return client

    monkeypatch.setattr("freeagent.sdk.agent.nats.connect", _connect)
    return lambda: created[-1]


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload."""

    def __init__(self, data: bytes) -> None:
        self.data = data


async def _until(predicate: Callable[[], bool]) -> None:
    """Yield to the event loop until ``predicate`` holds."""
    while not predicate():
        await asyncio.sleep(0)


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
    assert message == Message(type="PERCEPTION")


async def test_run_loop_processes_queued_messages_in_order(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    sent = [Message(type=t) for t in ("PERCEPTION", "THOUGHT")]
    for message in sent:
        agent.queue.put_nowait(message)

    await agent.queue.join()  # wait until every queued message is processed
    assert agent.processed == sent

    await agent.stop()


async def test_run_stops_the_agent_on_a_stop_message(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # A STOP message tells the run loop to shut the whole agent down.
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    client = fake_connect()

    await agent.callback(FakeMsg(b'{"type": "STOP"}'))  # type: ignore[arg-type]

    # The run loop tears the agent down: task cleared, NATS disconnected.
    await asyncio.wait_for(_until(lambda: agent.task is None), timeout=1)
    assert all(s.unsubscribed for s in client.subscriptions)
    assert client.closed is True
    assert agent.client is None
    # STOP is consumed by the loop itself, not handed to process().
    assert agent.processed == []


async def test_stop_message_is_not_processed_before_a_real_one(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # A message ahead of STOP is processed; STOP halts the loop after it.
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    agent.queue.put_nowait(Message(type="PERCEPTION"))
    agent.queue.put_nowait(Message(type=STOP_AGENT))

    await asyncio.wait_for(_until(lambda: agent.task is None), timeout=1)
    assert agent.processed == [Message(type="PERCEPTION")]


async def test_stop_path_is_final_and_cannot_be_overridden() -> None:
    # The shutdown sequence is final: a subclass cannot redefine start/stop.
    assert getattr(Agent.start, "__final__", False) is True
    assert getattr(Agent.stop, "__final__", False) is True


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


async def test_started_agent_processes_a_delivered_message(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # End to end: a message delivered via callback is processed by the run loop
    # that start() launches, then stop() shuts everything down.
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    await agent.callback(FakeMsg(b'{"type": "THOUGHT"}'))  # type: ignore[arg-type]

    await agent.queue.join()
    assert agent.processed == [Message(type="THOUGHT")]

    await agent.stop()
    assert agent.task is None


async def test_run_loop_marks_task_done_even_when_process_raises(
    fake_connect: Callable[[], FakeClient],
) -> None:
    class Boom(Agent):
        async def process(self, message: Message) -> None:
            raise ValueError("boom")

    agent = Boom("worker", "jobs")
    await agent.start()
    assert agent.task is not None
    agent.queue.put_nowait(Message(type="PERCEPTION"))

    # The loop task fails out, but task_done must still have fired so a join()
    # does not hang.
    with pytest.raises(ValueError, match="boom"):
        await agent.task
    await asyncio.wait_for(agent.queue.join(), timeout=1)


async def test_stop_without_start_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. stop() must not raise.
    await agent.stop()
