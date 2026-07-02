"""Tests for :class:`freeagent.sdk.entity.Agent`.

These exercise message dispatch (:class:`~freeagent.sdk.message.StartEntity`,
:class:`~freeagent.sdk.message.StopEntity`, other :class:`~freeagent.sdk.message.Command`
subclasses, and plain messages) and the run loop, against a fake NATS client and fake incoming
messages, so no live server is required.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import pytest
from freeagent.sdk.entity import AGENTS, Agent
from freeagent.sdk.message import Ack, Command, Message, StartEntity, StopEntity
from nats.aio.msg import Msg

Handler = Callable[[Msg], Awaitable[None]]


class FakeSubscription:
    """Records whether it was unsubscribed, and how many times."""

    def __init__(self, subject: str, cb: Handler) -> None:
        self.subject = subject
        self.cb = cb
        self.unsubscribe_calls = 0

    async def unsubscribe(self) -> None:
        self.unsubscribe_calls += 1


class FakeClient:
    """A stand-in for nats.aio.client.Client that records interactions."""

    def __init__(self) -> None:
        self.subscriptions: list[FakeSubscription] = []
        self.closed = False
        self.close_calls = 0

    async def subscribe(self, subject: str, cb: Handler | None = None) -> FakeSubscription:
        assert cb is not None
        sub = FakeSubscription(subject, cb)
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

    monkeypatch.setattr("freeagent.sdk.entity.nats.connect", _connect)
    return created


@pytest.fixture
def fake_connect(created_clients: list[FakeClient]) -> Callable[[], FakeClient]:
    """Expose the most recently created FakeClient."""
    return lambda: created_clients[-1]


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload."""

    def __init__(self, data: bytes, reply: str = "") -> None:
        self.data = data
        self.reply = reply
        self.responses: list[bytes] = []

    async def respond(self, data: bytes) -> None:
        self.responses.append(data)


def _msg(message: Message, reply: str = "") -> FakeMsg:
    """Build a FakeMsg carrying the given message, optionally as a NATS request."""
    return FakeMsg(message.to_bytes(), reply=reply)


class Ping(Message):
    """A plain, in-domain message used to exercise the queue."""

    label: str = ""


class Shout(Command):
    """An application-defined command, used to exercise process_command()."""

    label: str = ""


class RecordingAgent(Agent):
    """A concrete agent whose process_message() and process_command() record what they handle."""

    def __init__(self, name: str, *subjects: str) -> None:
        super().__init__("episode-root", name, *subjects)
        self.processed: list[Message] = []
        self.commands: list[Command] = []

    async def process_message(self, message: Message) -> None:
        self.processed.append(message)

    async def process_command(self, msg: Msg, command: Command) -> None:
        self.commands.append(command)


async def test_init_subscribes_under_agents_and_the_agent_name() -> None:
    agent = RecordingAgent("worker", "jobs", "events")

    assert agent.subjects == [
        f"episode-root.{AGENTS}.worker",
        f"episode-root.{AGENTS}.worker.jobs",
        f"episode-root.{AGENTS}.worker.events",
    ]


async def test_init_with_no_extra_subjects_still_subscribes_under_the_agent_name() -> None:
    agent = RecordingAgent("worker")

    assert agent.subjects == [f"episode-root.{AGENTS}.worker"]


async def test_start_subscribes_to_every_subject(fake_connect: Callable[[], FakeClient]) -> None:
    agent = RecordingAgent("worker", "jobs", "events")

    await agent.start()

    client = fake_connect()
    assert [s.subject for s in client.subscriptions] == [
        f"episode-root.{AGENTS}.worker",
        f"episode-root.{AGENTS}.worker.jobs",
        f"episode-root.{AGENTS}.worker.events",
    ]
    for sub in client.subscriptions:
        assert sub.cb == agent.handle_incoming_message


async def test_handle_incoming_message_decodes_and_queues_a_plain_message() -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.handle_incoming_message(_msg(Ping(label="a")))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 1
    msg, message = agent.queue.get_nowait()
    assert message == Ping(label="a")


async def test_start_entity_launches_the_run_loop_as_a_background_task() -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]

    assert agent.task is not None
    assert not agent.task.done()

    agent.task.cancel()


async def test_start_entity_replies_with_ack_when_sent_as_a_request() -> None:
    agent = RecordingAgent("worker", "jobs")
    msg = _msg(StartEntity(), reply="reply-inbox")

    await agent.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack().to_bytes()]

    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_processes_queued_messages_in_order() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    sent = [Ping(label=label) for label in ("a", "b")]
    for message in sent:
        agent.queue.put_nowait((None, message))

    # StartEntity is expected to have launched the run loop, which should drain the queue; if it
    # didn't, this would otherwise hang forever, so bound the wait instead.
    await asyncio.wait_for(agent.queue.join(), timeout=1)

    assert agent.processed == sent
    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_replies_with_process_message_return_value() -> None:
    class Echoing(Agent):
        async def process_message(self, message: Message) -> Message | None:
            return Ack()

    agent = Echoing("episode-root", "worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    msg = _msg(Ping(), reply="reply-inbox")
    agent.queue.put_nowait((msg, Ping()))  # type: ignore[arg-type]

    await asyncio.wait_for(agent.queue.join(), timeout=1)

    assert msg.responses == [Ack().to_bytes()]
    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_replies_with_ack_when_process_message_returns_none() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    msg = _msg(Ping(), reply="reply-inbox")
    agent.queue.put_nowait((msg, Ping()))  # type: ignore[arg-type]

    await asyncio.wait_for(agent.queue.join(), timeout=1)

    assert msg.responses == [Ack().to_bytes()]
    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_marks_task_done_even_when_process_message_raises() -> None:
    class Boom(Agent):
        async def process_message(self, message: Message) -> None:
            raise ValueError("boom")

    agent = Boom("episode-root", "worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    assert agent.task is not None
    agent.queue.put_nowait((None, Ping()))

    with pytest.raises(ValueError, match="boom"):
        await agent.task
    await asyncio.wait_for(agent.queue.join(), timeout=1)


async def test_stop_entity_cancels_the_run_loop_unsubscribes_and_disconnects(
    fake_connect: Callable[[], FakeClient],
) -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.start()
    client = fake_connect()
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    task = agent.task
    assert task is not None

    await agent.handle_incoming_message(_msg(StopEntity()))  # type: ignore[arg-type]

    assert agent.task is None
    assert task.cancelled()
    assert all(sub.unsubscribe_calls == 1 for sub in client.subscriptions)
    assert client.closed is True
    assert agent.client is None


async def test_stop_entity_replies_with_ack_when_sent_as_a_request() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]
    msg = _msg(StopEntity(), reply="reply-inbox")

    await agent.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack().to_bytes()]


async def test_stop_entity_without_a_running_task_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. StopEntity must not raise.
    await agent.handle_incoming_message(_msg(StopEntity()))  # type: ignore[arg-type]

    assert agent.task is None


async def test_stop_entity_is_not_handed_to_process_message() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]

    await agent.handle_incoming_message(_msg(StopEntity()))  # type: ignore[arg-type]

    assert agent.processed == []


async def test_command_is_dispatched_to_process_command() -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.handle_incoming_message(_msg(Shout(label="hi")))  # type: ignore[arg-type]

    assert agent.commands == [Shout(label="hi")]
    # Commands are handled directly, never queued for process_message().
    assert agent.queue.qsize() == 0


async def test_process_command_default_implementation_is_a_no_op() -> None:
    agent = Agent("episode-root", "worker", "jobs")

    await agent.handle_incoming_message(_msg(Shout()))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 0


async def test_process_message_default_implementation_is_a_no_op() -> None:
    agent = Agent("episode-root", "worker", "jobs")
    await agent.handle_incoming_message(_msg(StartEntity()))  # type: ignore[arg-type]

    agent.queue.put_nowait((None, Ping()))
    await asyncio.wait_for(agent.queue.join(), timeout=1)

    assert agent.task is not None
    agent.task.cancel()


async def test_start_is_idempotent(created_clients: list[FakeClient]) -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.start()
    await agent.start()

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

    assert client.close_calls == 1
    assert all(sub.unsubscribe_calls == 1 for sub in subscriptions)
    assert agent.client is None
    assert agent.subscriptions == []


async def test_respond_sends_the_reply_to_the_original_message() -> None:
    msg = FakeMsg(b"", reply="reply-inbox")

    await Agent.respond(msg, Ack())  # type: ignore[arg-type]

    assert msg.responses == [Ack().to_bytes()]


async def test_respond_does_nothing_when_there_is_no_original_message() -> None:
    # Should not raise.
    await Agent.respond(None, Ack())
