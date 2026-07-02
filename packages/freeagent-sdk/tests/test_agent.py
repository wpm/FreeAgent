"""Tests for :class:`freeagent.sdk.entity.Agent`.

These exercise message dispatch (:class:`~freeagent.sdk.message.StartEntity`,
:class:`~freeagent.sdk.message.StopEntity`, other :class:`~freeagent.sdk.message.Command`
subclasses, and plain messages) and the run loop, against a fake NATS client and fake incoming
messages, so no live server is required.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable

import pytest
from fixtures import FakeClient, FakeMsg, Ping, RecordingAgent, Shout
from freeagent.sdk.entity import AGENTS, Agent
from freeagent.sdk.message import Ack, Message, StartEntity, StopEntity


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

    await agent.handle_incoming_message(FakeMsg.for_message(Ping(label="a")))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 1
    msg, message = agent.queue.get_nowait()
    assert message == Ping(label="a")


async def test_start_entity_launches_the_run_loop_as_a_background_task() -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]

    assert agent.task is not None
    assert not agent.task.done()

    agent.task.cancel()


async def test_start_entity_replies_with_ack_when_sent_as_a_request() -> None:
    agent = RecordingAgent("worker", "jobs")
    msg = FakeMsg.for_message(StartEntity(), reply="reply-inbox")

    await agent.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack().to_bytes()]

    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_processes_queued_messages_in_order() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
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
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
    msg = FakeMsg.for_message(Ping(), reply="reply-inbox")
    agent.queue.put_nowait((msg, Ping()))  # type: ignore[arg-type]

    await asyncio.wait_for(agent.queue.join(), timeout=1)

    assert msg.responses == [Ack().to_bytes()]
    assert agent.task is not None
    agent.task.cancel()


async def test_run_loop_replies_with_ack_when_process_message_returns_none() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
    msg = FakeMsg.for_message(Ping(), reply="reply-inbox")
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
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
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
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
    task = agent.task
    assert task is not None

    await agent.handle_incoming_message(FakeMsg.for_message(StopEntity()))  # type: ignore[arg-type]

    assert agent.task is None
    assert task.cancelled()
    assert all(sub.unsubscribe_calls == 1 for sub in client.subscriptions)
    assert client.closed is True
    assert agent.client is None


async def test_stop_entity_replies_with_ack_when_sent_as_a_request() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]
    msg = FakeMsg.for_message(StopEntity(), reply="reply-inbox")

    await agent.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == [Ack().to_bytes()]


async def test_stop_entity_without_a_running_task_is_safe() -> None:
    agent = RecordingAgent("worker", "jobs")
    # Never started: no task, no client, no subscriptions. StopEntity must not raise.
    await agent.handle_incoming_message(FakeMsg.for_message(StopEntity()))  # type: ignore[arg-type]

    assert agent.task is None


async def test_stop_entity_is_not_handed_to_process_message() -> None:
    agent = RecordingAgent("worker", "jobs")
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]

    await agent.handle_incoming_message(FakeMsg.for_message(StopEntity()))  # type: ignore[arg-type]

    assert agent.processed == []


async def test_command_is_dispatched_to_process_command() -> None:
    agent = RecordingAgent("worker", "jobs")

    await agent.handle_incoming_message(FakeMsg.for_message(Shout(label="hi")))  # type: ignore[arg-type]

    assert agent.commands == [Shout(label="hi")]
    # Commands are handled directly, never queued for process_message().
    assert agent.queue.qsize() == 0


async def test_process_command_default_implementation_is_a_no_op() -> None:
    agent = Agent("episode-root", "worker", "jobs")

    await agent.handle_incoming_message(FakeMsg.for_message(Shout()))  # type: ignore[arg-type]

    assert agent.queue.qsize() == 0


async def test_process_message_default_implementation_is_a_no_op() -> None:
    agent = Agent("episode-root", "worker", "jobs")
    await agent.handle_incoming_message(FakeMsg.for_message(StartEntity()))  # type: ignore[arg-type]

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
