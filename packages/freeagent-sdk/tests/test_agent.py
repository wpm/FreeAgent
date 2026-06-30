"""Tests for the Free Agent SDK base Agent class.

These exercise the connect/subscribe/unsubscribe/disconnect lifecycle against a fake NATS client, so
no live server is required.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import pytest
from freeagent.sdk import Agent
from freeagent.sdk.agent import CONTROL_SUBJECT, START_AGENT, STOP_AGENT, Message
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

    async def _connect(_: str | list[str], **__: object) -> FakeClient:
        client = FakeClient()
        created.append(client)
        return client

    monkeypatch.setattr("freeagent.sdk.agent.nats.connect", _connect)
    return lambda: created[-1]


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload on a subject."""

    def __init__(self, data: bytes, subject: str = "jobs") -> None:
        self.data = data
        self.subject = subject


def _control(message_type: str) -> FakeMsg:
    """Build a control-subject FakeMsg carrying the given message type."""
    return FakeMsg(f'{{"type": "{message_type}"}}'.encode(), subject=CONTROL_SUBJECT)


async def _until(predicate: Callable[[], bool]) -> None:
    """Yield to the event loop until ``predicate`` holds."""
    while not predicate():
        await asyncio.sleep(0)


class RecordingAgent(Agent):
    """A concrete agent whose process() records the messages it handles."""

    def __init__(self, name: str, *subjects: str) -> None:
        super().__init__("episode-root", name, *subjects)
        self.processed: list[tuple[Message, bool]] = []

    async def process(self, message: Message, is_control: bool) -> None:
        self.processed.append((message, is_control))


async def test_callback_decodes_payload_onto_queue() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.callback(FakeMsg(b'{"type": "PERCEPTION"}'))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 1
    message, is_control = agent.queue.get_nowait()
    assert message == Message(type="PERCEPTION")
    assert is_control is False


async def test_run_loop_processes_queued_messages_in_order(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.connect()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    sent = [Message(type=t) for t in ("PERCEPTION", "THOUGHT")]
    for message in sent:
        agent.queue.put_nowait((message, False))

    await agent.queue.join()  # wait until every queued message is processed
    assert agent.processed == [(message, False) for message in sent]

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_run_stops_the_agent_on_a_stop_message(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # A STOP control message tears the whole agent down.
    agent = RecordingAgent("worker", "jobs")
    await agent.connect()
    client = fake_connect()

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]

    # The control message tears the agent down: task cleared, NATS disconnected.
    await asyncio.wait_for(_until(lambda: agent.client is None), timeout=1)
    assert all(s.unsubscribed for s in client.subscriptions)
    assert client.closed is True
    assert agent.client is None
    # STOP is consumed by callback itself, not handed to process().
    assert agent.processed == []


async def test_start_connects_subscribes_and_launches_run_loop(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs", "events")
    await agent.connect()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == [
        "episode-root.worker.jobs",
        "episode-root.worker.events",
        "episode-root.worker.control",
    ]
    # Each subscription uses the agent name as the queue group and the agent's
    # callback as the handler.
    for sub in client.subscriptions:
        assert sub.queue == "worker"
        assert sub.cb == agent.callback

    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]

    # START_AGENT launches the run loop as a background task that is still running.
    assert agent.task is not None
    assert not agent.task.done()

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_stop_cancels_run_loop_unsubscribes_and_disconnects(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs", "events")
    await agent.connect()
    client = fake_connect()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    task = agent.task
    assert task is not None

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]

    # The run loop's handle is cleared.
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
    # that START_AGENT launches, then STOP_AGENT shuts everything down.
    agent = RecordingAgent("worker", "jobs")
    await agent.connect()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    await agent.callback(FakeMsg(b'{"type": "THOUGHT"}'))  # type: ignore[arg-type]

    await agent.queue.join()
    assert agent.processed == [(Message(type="THOUGHT"), False)]

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]
    assert agent.task is None


async def test_run_loop_marks_task_done_even_when_process_raises(
    fake_connect: Callable[[], FakeClient],
) -> None:
    class Boom(Agent):
        async def process(self, message: Message, is_control: bool) -> None:
            raise ValueError("boom")

    agent = Boom("episode-root", "worker", "jobs")
    await agent.connect()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    assert agent.task is not None
    agent.queue.put_nowait((Message(type="PERCEPTION"), False))

    # The loop task fails out, but task_done must still have fired so a join()
    # does not hang.
    with pytest.raises(ValueError, match="boom"):
        await agent.task
    await asyncio.wait_for(agent.queue.join(), timeout=1)


async def test_stop_without_start_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. STOP_AGENT must not raise.
    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]
