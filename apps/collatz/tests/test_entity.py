"""Unit tests for :mod:`freeagent.app.collatz.entity`.

Drive the Collatz agent and environment with the fakes in :mod:`fakes` -- no NATS server -- to cover
the acceptance criteria at the logic level: the agent extends a chain by one step and returns it;
the environment stops exactly the finished agent; the episode completes only when every agent
finishes; and agents with different starting numbers finish in different orders without interfering.
The full over-the-wire episode is a separate integration test.
"""

from __future__ import annotations

import asyncio

from fakes import FakeClient, FakeMsg
from freeagent.app.collatz.entity import REPLIES, CollatzAgent, CollatzEnvironment
from freeagent.app.collatz.message import Chain
from freeagent.sdk.entity import AGENTS, ENVIRONMENT
from freeagent.sdk.message import Ack, EpisodeComplete, Message, StopAgent, StopEntity


def reply_subject(episode_root: str, agent: str) -> str:
    """The subject an agent sends its extended chains back to the environment on."""
    return f"{episode_root}.{ENVIRONMENT}.{REPLIES}.{agent}"


async def drain(agent: CollatzAgent) -> None:
    """Await every background counter-request the agent has launched.

    :meth:`~freeagent.app.collatz.entity.CollatzAgent.process_message` fires its counter-request as
    a background task and returns without awaiting it, so a test that wants to observe the request
    on the fake client must first let that task run. Awaits the tasks in
    :attr:`~freeagent.app.collatz.entity.CollatzAgent.pending` (copied, since each clears itself
    from the set on completion).

    :param agent: The agent whose in-flight counter-requests to await.
    """
    await asyncio.gather(*list(agent.pending))


def decode_requests(client: FakeClient) -> list[tuple[str, Message]]:
    """Decode every request a fake client recorded into ``(subject, message)`` pairs."""
    return [(subject, Message.model_validate_json(payload)) for subject, payload in client.requests]


# --- CollatzAgent ---------------------------------------------------------------------------------


async def test_agent_extends_a_chain_and_sends_it_to_its_reply_subject() -> None:
    agent = CollatzAgent("episode", "agent-0")
    agent.client = FakeClient()  # type: ignore[assignment]

    reply = await agent.process_message(Chain(numbers=[3]))
    await drain(agent)  # let the background counter-request run to completion

    # The reply to the environment's request is a bare Ack (None -> the run loop acks); the extended
    # chain went out as the agent's own counter-request on its reply subject.
    assert reply is None
    [(subject, message)] = decode_requests(agent.client)  # type: ignore[arg-type]
    assert subject == reply_subject("episode", "agent-0")
    assert isinstance(message, Chain)
    assert message.numbers == [3, 10]


async def test_agent_sends_the_counter_request_without_awaiting_it() -> None:
    # process_message must return before the counter-request round-trips, so the run loop's ack to
    # the environment isn't gated behind it -- the property that keeps a multi-agent episode from
    # deadlocking. Until the background task is drained, no request has been recorded yet.
    agent = CollatzAgent("episode", "agent-0")
    agent.client = FakeClient()  # type: ignore[assignment]

    await agent.process_message(Chain(numbers=[3]))

    assert len(agent.pending) == 1  # the counter-request is in flight, not yet sent
    assert agent.client.requests == []  # type: ignore[union-attr]
    await drain(agent)
    assert agent.pending == set()  # and it clears itself once complete
    assert len(agent.client.requests) == 1  # type: ignore[union-attr]


async def test_agent_stop_cancels_an_outstanding_counter_request() -> None:
    # When the environment stops the agent its reply subject goes away; a counter-request still in
    # flight is cancelled by stop() rather than left to hang.
    agent = CollatzAgent("episode", "agent-0")
    agent.client = FakeClient()  # type: ignore[assignment]

    async def slow_request(*_: object, **__: object) -> FakeMsg:
        await asyncio.sleep(3600)
        return FakeMsg.for_message(Ack())

    agent.client.request = slow_request  # type: ignore[method-assign, union-attr]
    await agent.process_message(Chain(numbers=[3]))
    assert len(agent.pending) == 1

    await agent.stop()

    assert agent.pending == set()  # the outstanding counter-request was cancelled
    assert agent.client is None  # and the base teardown ran


async def test_agent_ignores_a_non_chain_message() -> None:
    agent = CollatzAgent("episode", "agent-0")
    agent.client = FakeClient()  # type: ignore[assignment]

    reply = await agent.process_message(Ack())
    await drain(agent)

    assert reply is None
    assert agent.pending == set()  # a non-chain message launches no counter-request
    assert agent.client.requests == []  # type: ignore[union-attr]


# --- CollatzEnvironment bookkeeping (pure) --------------------------------------------------------


def test_environment_seeds_one_chain_per_agent_from_its_start() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 6, "agent-1": 27})

    assert env.chains == {"agent-0": Chain(numbers=[6]), "agent-1": Chain(numbers=[27])}
    assert env.finished == set()


def test_record_marks_an_agent_finished_when_its_chain_reaches_one() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2})

    just_finished = env.record("agent-0", Chain(numbers=[2, 1]))

    assert just_finished
    assert env.finished == {"agent-0"}
    assert env.chains["agent-0"] == Chain(numbers=[2, 1])


def test_record_does_not_mark_a_still_running_chain() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 6})

    just_finished = env.record("agent-0", Chain(numbers=[6, 3]))

    assert not just_finished
    assert env.finished == set()


def test_record_reports_completion_only_the_first_time() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2})

    first = env.record("agent-0", Chain(numbers=[2, 1]))
    again = env.record("agent-0", Chain(numbers=[2, 1]))

    assert first
    assert not again  # already finished -> not "just finished" again


def test_episode_complete_only_once_every_agent_finished() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2, "agent-1": 4})

    env.record("agent-0", Chain(numbers=[2, 1]))
    assert not env.episode_complete()

    env.record("agent-1", Chain(numbers=[4, 2, 1]))
    assert env.episode_complete()


def test_agent_of_reads_the_name_off_the_reply_subject() -> None:
    env = CollatzEnvironment("episode", {"agent-3": 6})

    assert env.agent_of(reply_subject("episode", "agent-3")) == "agent-3"


# --- CollatzEnvironment.handle_incoming_message ---------------------------------------------------


async def test_environment_acks_then_sends_the_chain_back_while_running() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 6})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]
    msg = FakeMsg.for_message(Chain(numbers=[6, 3]), subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    # Acked (receipt only), the running chain recorded, and sent back to the agent to extend again.
    assert [Message.model_validate_json(r) for r in msg.responses] == [Ack()]
    assert env.chains["agent-0"] == Chain(numbers=[6, 3])
    [(subject, message)] = decode_requests(client)
    assert subject == f"episode.{AGENTS}.agent-0"
    assert message == Chain(numbers=[6, 3])


async def test_environment_stops_exactly_the_finished_agent() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2, "agent-1": 6})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]
    # agent-0's chain reaches 1; agent-1 is untouched and must keep running.
    msg = FakeMsg.for_message(Chain(numbers=[2, 1]), subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    stop_targets = [
        subject for subject, message in decode_requests(client) if isinstance(message, StopAgent)
    ]
    assert stop_targets == [f"episode.{AGENTS}.agent-0"]
    assert env.stopped_agents == {"agent-0"}
    # No chain was sent back to the finished agent, and agent-1 was never touched.
    sent_chains = [
        subject for subject, message in decode_requests(client) if isinstance(message, Chain)
    ]
    assert sent_chains == []


async def test_environment_completes_the_episode_when_the_last_agent_finishes() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]
    msg = FakeMsg.for_message(Chain(numbers=[2, 1]), subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    # The sole agent finishing completes the episode: EpisodeComplete is published and the
    # environment tears itself down (client closed, then cleared by Entity.stop).
    published = [Message.model_validate_json(payload) for _, payload in client.published]
    assert EpisodeComplete() in published
    assert client.closed
    assert env.client is None


async def test_completion_teardown_does_not_re_stop_the_already_stopped_agent() -> None:
    # The agent that just finished was stopped via StopAgent; teardown must not also broadcast
    # StopEntity to it (its subject has no responder once it has torn down).
    env = CollatzEnvironment("episode", {"agent-0": 2})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]
    msg = FakeMsg.for_message(Chain(numbers=[2, 1]), subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    stop_entity_targets = [
        subject for subject, message in decode_requests(client) if isinstance(message, StopEntity)
    ]
    assert stop_entity_targets == []  # only the StopAgent went out, no StopEntity re-broadcast
    assert env.agents == ("agent-0",)  # the narrowed agent list is restored after teardown


async def test_completion_teardown_still_stops_agents_that_were_not_individually_stopped() -> None:
    # If an agent somehow reached completion without a prior StopAgent, teardown must still stop it.
    env = CollatzEnvironment("episode", {"agent-0": 2, "agent-1": 6})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]
    # Force the "all finished" condition while leaving agent-1 un-stopped, then trigger stop().
    env.finished = {"agent-0", "agent-1"}
    env.stopped_agents = {"agent-0"}

    await env.stop()

    stop_entity_targets = [
        subject for subject, message in decode_requests(client) if isinstance(message, StopEntity)
    ]
    assert stop_entity_targets == [f"episode.{AGENTS}.agent-1"]


async def test_environment_does_not_complete_the_episode_while_an_agent_still_runs() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 2, "agent-1": 6})
    env.client = FakeClient()  # type: ignore[assignment]
    msg = FakeMsg.for_message(Chain(numbers=[2, 1]), subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    published = [Message.model_validate_json(payload) for _, payload in env.client.published]  # type: ignore[union-attr]
    assert EpisodeComplete() not in published
    assert not env.client.closed  # type: ignore[union-attr]


async def test_environment_acks_a_non_chain_request_but_does_not_drive_the_game() -> None:
    env = CollatzEnvironment("episode", {"agent-0": 6})
    env.client = FakeClient()  # type: ignore[assignment]
    # A non-chain request arriving on the environment's own subject (reply set = it is a request).
    msg = FakeMsg.for_message(Ack(), subject=f"episode.{ENVIRONMENT}", reply="inbox")

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    # Acked (empty) and otherwise ignored: no request went out, nothing was marked finished.
    assert msg.responses == [b""]
    assert env.client.requests == []  # type: ignore[union-attr]
    assert env.finished == set()


async def test_environment_does_not_respond_to_its_self_received_episode_complete() -> None:
    # Environment.stop() publishes EpisodeComplete on {episode_root}.environment, which the
    # environment self-receives. That publish has no reply subject, so responding to it would raise
    # over the wire; the handler must leave it alone.
    env = CollatzEnvironment("episode", {"agent-0": 6})
    env.client = FakeClient()  # type: ignore[assignment]
    msg = FakeMsg.for_message(
        EpisodeComplete(),
        subject=f"episode.{ENVIRONMENT}",
        reply="",  # publish: no reply subject
    )

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == []  # never responded -- a real Msg.respond here would raise
    assert env.client.requests == []  # type: ignore[union-attr]
    assert env.finished == set()


async def test_environment_ignores_an_undecodable_reply_without_driving_the_game() -> None:
    # A reply on a replies.* subject whose type tag names no known Message subclass decodes to None
    # via try_decode; it must be acked (it was a request) but must not advance any chain.
    env = CollatzEnvironment("episode", {"agent-0": 6})
    env.client = FakeClient()  # type: ignore[assignment]
    msg = FakeMsg(b'{"message_type": "Nonesuch"}', subject=reply_subject("episode", "agent-0"))

    await env.handle_incoming_message(msg)  # type: ignore[arg-type]

    assert msg.responses == [b""]  # acked (it was a request) ...
    assert env.client.requests == []  # type: ignore[union-attr]  # ... but the game did not advance
    assert env.finished == set()
    assert env.chains["agent-0"] == Chain(numbers=[6])  # unchanged


# --- Multiple agents, different starts, no interference -------------------------------------------


async def test_agents_with_different_starts_finish_in_different_orders_without_interference() -> (
    None
):
    # agent-fast starts at 2 (finishes in one step); agent-slow starts at 6 (nine-long chain).
    env = CollatzEnvironment("episode", {"agent-fast": 2, "agent-slow": 6})
    client = FakeClient()
    env.client = client  # type: ignore[assignment]

    # Drive agent-slow one running step first: it must be sent back, not stopped, and must not
    # complete the episode or disturb agent-fast's bookkeeping.
    slow_msg = FakeMsg.for_message(
        Chain(numbers=[6, 3]), subject=reply_subject("episode", "agent-slow")
    )
    await env.handle_incoming_message(slow_msg)  # type: ignore[arg-type]
    assert env.finished == set()
    assert not client.closed

    # Now agent-fast finishes. Only agent-fast is stopped; agent-slow keeps running; the episode is
    # not yet complete.
    fast_msg = FakeMsg.for_message(
        Chain(numbers=[2, 1]), subject=reply_subject("episode", "agent-fast")
    )
    await env.handle_incoming_message(fast_msg)  # type: ignore[arg-type]
    assert env.finished == {"agent-fast"}
    assert env.stopped_agents == {"agent-fast"}
    assert not client.closed

    # Finally agent-slow finishes: now the episode completes.
    done_msg = FakeMsg.for_message(
        Chain(numbers=[4, 2, 1]), subject=reply_subject("episode", "agent-slow")
    )
    await env.handle_incoming_message(done_msg)  # type: ignore[arg-type]
    assert env.finished == {"agent-fast", "agent-slow"}
    assert env.stopped_agents == {"agent-fast", "agent-slow"}
    assert client.closed
    assert env.client is None
