"""Tests for the Free Agent SDK base Agent class.

These exercise the connect/subscribe/unsubscribe/disconnect lifecycle against a fake NATS client, so
no live server is required.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import pytest
from freeagent.sdk import Agent
from freeagent.sdk.agent import CONTROL_SUBJECT, START_AGENT, STOP_AGENT, Ack, Message
from nats.aio.msg import Msg

Handler = Callable[[Msg], Awaitable[None]]


class FakeSubscription:
    """Records whether it was unsubscribed, and how many times."""

    def __init__(self, subject: str, queue: str, cb: Handler) -> None:
        self.subject = subject
        self.queue = queue
        self.cb = cb
        self.unsubscribed = False
        self.unsubscribe_calls = 0

    async def unsubscribe(self) -> None:
        self.unsubscribed = True
        self.unsubscribe_calls += 1


class FakeClient:
    """A stand-in for nats.aio.client.Client that records interactions."""

    def __init__(self) -> None:
        self.subscriptions: list[FakeSubscription] = []
        self.closed = False
        self.close_calls = 0

    async def subscribe(
        self, subject: str, queue: str = "", cb: Handler | None = None
    ) -> FakeSubscription:
        assert cb is not None
        sub = FakeSubscription(subject, queue, cb)
        self.subscriptions.append(sub)
        return sub

    async def close(self) -> None:
        self.closed = True
        self.close_calls += 1


@pytest.fixture
def created_clients(monkeypatch: pytest.MonkeyPatch) -> list[FakeClient]:
    """Patch nats.connect to hand out a FakeClient, and record every one created."""
    created: list[FakeClient] = []

    async def _connect(_: str | list[str], **__: object) -> FakeClient:
        client = FakeClient()
        created.append(client)
        return client

    monkeypatch.setattr("freeagent.sdk.agent.nats.connect", _connect)
    return created


@pytest.fixture
def fake_connect(created_clients: list[FakeClient]) -> Callable[[], FakeClient]:
    """Expose the most recently created FakeClient."""
    return lambda: created_clients[-1]


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload on a subject."""

    def __init__(self, data: bytes, subject: str = "jobs", reply: str = "") -> None:
        self.data = data
        self.subject = subject
        self.reply = reply
        self.responses: list[bytes] = []

    async def respond(self, data: bytes) -> None:
        self.responses.append(data)


def _control(message_type: str, reply: str = "") -> FakeMsg:
    """Build a control-subject FakeMsg carrying the given message type."""
    return FakeMsg(f'{{"type": "{message_type}"}}'.encode(), subject=CONTROL_SUBJECT, reply=reply)


async def _until(predicate: Callable[[], bool]) -> None:
    """Yield to the event loop until ``predicate`` holds."""
    while not predicate():
        await asyncio.sleep(0)


class RecordingAgent(Agent):
    """A concrete agent whose process_message() records the messages it handles."""

    def __init__(self, name: str, *subjects: str) -> None:
        super().__init__("episode-root", name, *subjects)
        self.processed: list[Message] = []

    async def process_message(self, message: Message) -> None:
        self.processed.append(message)


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
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    sent = [Message(type=t) for t in ("PERCEPTION", "THOUGHT")]
    for message in sent:
        agent.queue.put_nowait(message)

    await agent.queue.join()  # wait until every queued message is processed
    assert agent.processed == sent

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_run_stops_the_agent_on_a_stop_message(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # A STOP control message tears the whole agent down.
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    client = fake_connect()

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]

    # The control message tears the agent down: task cleared, NATS disconnected.
    await asyncio.wait_for(_until(lambda: agent.client is None), timeout=1)
    assert all(s.unsubscribed for s in client.subscriptions)
    assert client.closed is True
    assert agent.client is None
    # STOP is consumed by callback itself, not handed to process_message().
    assert agent.processed == []


async def test_start_connects_subscribes_and_launches_run_loop(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs", "events")
    await agent.start()

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
    await agent.start()
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
    await agent.start()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    await agent.callback(FakeMsg(b'{"type": "THOUGHT"}'))  # type: ignore[arg-type]

    await agent.queue.join()
    assert agent.processed == [Message(type="THOUGHT")]

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]
    assert agent.task is None


async def test_run_loop_marks_task_done_even_when_process_raises(
    fake_connect: Callable[[], FakeClient],
) -> None:
    class Boom(Agent):
        async def process_message(self, message: Message) -> None:
            raise ValueError("boom")

    agent = Boom("episode-root", "worker", "jobs")
    await agent.start()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    assert agent.task is not None
    agent.queue.put_nowait(Message(type="PERCEPTION"))

    # The loop task fails out, but task_done must still have fired so a join()
    # does not hang.
    with pytest.raises(ValueError, match="boom"):
        await agent.task
    await asyncio.wait_for(agent.queue.join(), timeout=1)


async def test_stop_without_start_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. STOP_AGENT must not raise.
    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_start_is_idempotent(created_clients: list[FakeClient]) -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.start()
    await agent.start()

    # The second start() is a no-op: only one NATS connection and subscription set.
    assert len(created_clients) == 1
    assert len(agent.subscriptions) == 2

    await agent.stop()


async def test_stop_is_idempotent(created_clients: list[FakeClient]) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    client = created_clients[-1]
    subscriptions = list(client.subscriptions)

    await agent.stop()
    await agent.stop()

    # The second stop() is a no-op: unsubscribe/close were not called again.
    assert client.close_calls == 1
    assert all(sub.unsubscribe_calls == 1 for sub in subscriptions)
    assert agent.client is None
    assert agent.subscriptions == []


async def test_control_command_without_reply_subject_gets_no_response(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    msg = _control(START_AGENT)

    await agent.callback(msg)  # type: ignore[arg-type]

    assert msg.responses == []

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_start_command_sent_as_request_is_acked_running_true(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    msg = _control(START_AGENT, reply="reply-inbox")

    await agent.callback(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack(running=True).model_dump_json().encode()]

    await agent.callback(_control(STOP_AGENT))  # type: ignore[arg-type]


async def test_stop_command_sent_as_request_is_acked_running_false(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    await agent.callback(_control(START_AGENT))  # type: ignore[arg-type]
    msg = _control(STOP_AGENT, reply="reply-inbox")

    await agent.callback(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack(running=False).model_dump_json().encode()]
