"""The agent fold: determinism, the outbox, the think queue, and the runtime.

Semantics pinned here: the outbox is published only after the handler
returns; on a handler exception the outbox is **discarded** (and the error
logged) while think messages posted by the same handler are still delivered
-- the think queue is internal state, not an effect on the world.
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

import pytest
from freeagent_testing import ROOT, connected_transport, decoded, publish, published, wait_for

from freeagent import Agent, Envelope, MemoryTransport
from freeagent.agent import MessageEvent, ThinkEvent

if TYPE_CHECKING:
    from collections.abc import Mapping

PUBLIC = f"{ROOT}.public"
CONTROL = f"{ROOT}.control"


def world_event(payload: Any, sender: str = "someone") -> MessageEvent:
    return MessageEvent("public", Envelope(episode_id="ep1", sender=sender, payload=payload))


async def attach(agent: Agent) -> MemoryTransport:
    """Give an agent a transport without running its event loop (white-box)."""
    transport = await connected_transport()
    agent._transport = transport
    return transport


class Recording(Agent):
    """Collects everything it perceives and thinks."""

    def __init__(
        self, subject_root: str, agent_id: str, config: Mapping[str, Any] | None = None
    ) -> None:
        super().__init__(subject_root, agent_id, config)
        self.perceived: list[tuple[str, Any]] = []
        self.thoughts: list[Any] = []

    async def perceive(self, message: Envelope) -> None:
        self.perceived.append((message.sender, message.payload))

    async def handle_think(self, payload: Any) -> None:
        self.thoughts.append(payload)


# ---------------------------------------------------------------------------
# Constructor validation
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("agent_id", ["alice", "Agent_7", "a-b"])
def test_valid_agent_ids_accepted(agent_id: str) -> None:
    assert Agent(ROOT, agent_id).id == agent_id


@pytest.mark.parametrize("agent_id", ["", "a b", "a.b", "café", "no/slash"])
def test_invalid_agent_ids_rejected(agent_id: str) -> None:
    with pytest.raises(ValueError, match="must match"):
        Agent(ROOT, agent_id)


def test_env_is_reserved() -> None:
    with pytest.raises(ValueError, match="reserved"):
        Agent(ROOT, "env")


# ---------------------------------------------------------------------------
# The fold
# ---------------------------------------------------------------------------


class Accumulator(Agent):
    """Deterministic fold: state is a running total; every event acts the total."""

    def __init__(
        self, subject_root: str, agent_id: str, config: Mapping[str, Any] | None = None
    ) -> None:
        super().__init__(subject_root, agent_id, config)
        self.total = 0

    async def perceive(self, message: Envelope) -> None:
        self.total += int(message.payload)
        self.act({"total": self.total})


async def test_fold_is_deterministic_same_events_same_state_and_outbox() -> None:
    events = [world_event(n) for n in (3, 1, 4, 1, 5)]
    runs: list[tuple[int, list[tuple[str, Any]]]] = []
    for _ in range(2):
        agent = Accumulator(ROOT, "acc")
        transport = await attach(agent)
        for event in events:
            await agent._process_event(event)
        wire = [(subject, envelope.payload) for subject, envelope in decoded(transport)]
        runs.append((agent.total, wire))
    assert runs[0] == runs[1]
    assert runs[0][0] == 14
    assert runs[0][1] == [(PUBLIC, {"total": t}) for t in (3, 4, 8, 9, 14)]


async def test_outbox_is_published_only_after_the_handler_returns() -> None:
    seen_mid_handler: list[int] = []

    class MidCheck(Agent):
        async def perceive(self, message: Envelope) -> None:
            self.act("out")
            seen_mid_handler.append(len(transport.history))

    agent = MidCheck(ROOT, "mid")
    transport = await attach(agent)
    await agent._process_event(world_event("in"))
    assert seen_mid_handler == [0]  # nothing on the wire while the handler runs
    assert [e.payload for _, e in decoded(transport)] == ["out"]


async def test_outbox_is_discarded_when_the_handler_raises() -> None:
    class ActsThenFails(Agent):
        async def perceive(self, message: Envelope) -> None:
            self.act("never published")
            raise RuntimeError("boom")

    agent = ActsThenFails(ROOT, "failer")
    transport = await attach(agent)
    await agent._process_event(world_event("in"))
    assert transport.history == []
    # The agent survives: the next event is processed normally.
    await agent._process_event(ThinkEvent("still alive"))
    assert transport.history == []


async def test_think_queue_is_not_behind_the_outbox() -> None:
    class ThinksActsAndFails(Recording):
        async def perceive(self, message: Envelope) -> None:
            self.think("noted")
            self.act("never published")
            raise RuntimeError("boom")

    agent = ThinksActsAndFails(ROOT, "thinker")
    transport = await attach(agent)
    await agent._process_event(world_event("in"))
    # The outbox was discarded with the exception...
    assert transport.history == []
    # ...but the think message was written directly and is already queued.
    queued = agent._events.get_nowait()
    assert isinstance(queued, ThinkEvent)
    assert queued.payload == "noted"
    await agent._process_event(queued)
    assert agent.thoughts == ["noted"]


async def test_act_recipients_map_to_inboxes_and_env_never_raw_subjects() -> None:
    class Sender(Agent):
        async def handle_think(self, payload: Any) -> None:
            self.act(payload, recipients=["bob", "env"])

    agent = Sender(ROOT, "alice")
    transport = await attach(agent)
    await agent._process_event(ThinkEvent("hello"))
    wire = decoded(transport)
    assert [subject for subject, _ in wire] == [f"{ROOT}.agent.bob", f"{ROOT}.env"]
    # One act to several recipients is one message: same message_id everywhere.
    assert len({envelope.message_id for _, envelope in wire}) == 1
    assert all(envelope.sender == "alice" for _, envelope in wire)


async def test_act_default_is_broadcast_on_public() -> None:
    class Broadcaster(Agent):
        async def handle_think(self, payload: Any) -> None:
            self.act(payload)

    agent = Broadcaster(ROOT, "alice")
    transport = await attach(agent)
    await agent._process_event(ThinkEvent("to the room"))
    assert [(s, e.payload) for s, e in decoded(transport)] == [(PUBLIC, "to the room")]


def test_act_validates_recipient_ids() -> None:
    agent = Agent(ROOT, "alice")
    with pytest.raises(ValueError, match="recipient"):
        agent.act("x", recipients=["not a name"])


# ---------------------------------------------------------------------------
# The runtime: serve() on the in-memory transport
# ---------------------------------------------------------------------------


async def shutdown_and_join(transport: MemoryTransport, task: asyncio.Task[None]) -> None:
    await publish(transport, CONTROL, "env", {"type": "shutdown"})
    await asyncio.wait_for(task, timeout=2.0)


async def test_one_handler_at_a_time_even_when_handlers_yield() -> None:
    log: list[tuple[str, Any]] = []

    class Slow(Agent):
        async def perceive(self, message: Envelope) -> None:
            log.append(("enter", message.payload))
            await asyncio.sleep(0.02)
            log.append(("exit", message.payload))

    agent = Slow(ROOT, "slow", config={"grace_period": 0.01})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    await publish(transport, PUBLIC, "a", 1)
    await publish(transport, PUBLIC, "b", 2)
    await wait_for(lambda: len(log) == 4, message="both perceives to finish")
    assert log == [("enter", 1), ("exit", 1), ("enter", 2), ("exit", 2)]
    await shutdown_and_join(transport, task)


async def test_start_and_shutdown_lifecycle_phases() -> None:
    agent = Recording(ROOT, "alice", config={"grace_period": 0.02})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    assert agent.phase == "idle"
    await publish(transport, CONTROL, "env", {"type": "start"})
    await wait_for(lambda: agent.phase == "active", message="start to land")
    await publish(transport, CONTROL, "env", {"type": "shutdown"})
    await wait_for(lambda: agent.phase == "stopping", message="shutdown to land")
    await asyncio.wait_for(task, timeout=2.0)  # grace period expires, run() returns
    # Control traffic is lifecycle, not in-world: perceive never saw it.
    assert agent.perceived == []


async def test_presence_request_is_answered_by_the_runtime_not_user_code() -> None:
    agent = Recording(ROOT, "alice", config={"grace_period": 0.01})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    inbox = f"{ROOT}.agent.alice"
    await publish(
        transport, inbox, "env", {"type": "freeagent.presence_request", "request_id": "r1"}
    )
    await publish(
        transport, inbox, "env", {"type": "freeagent.presence_request", "request_id": "r2"}
    )
    await wait_for(
        lambda: (
            len(published(transport, f"{ROOT}.reply.r1")) == 1
            and len(published(transport, f"{ROOT}.reply.r2")) == 1
        ),
        message="presence replies",
    )
    replies = [published(transport, f"{ROOT}.reply.{r}")[0] for r in ("r1", "r2")]
    for reply, request_id in zip(replies, ("r1", "r2"), strict=True):
        assert reply.payload["type"] == "freeagent.presence_reply"
        assert reply.payload["request_id"] == request_id
        assert reply.payload["agent"] == "alice"
    # The nonce is per-process: generated once, identical across replies.
    assert replies[0].payload["nonce"] == replies[1].payload["nonce"]
    # The runtime intercepted the freeagent.* payloads; user code never saw them.
    assert agent.perceived == []
    await shutdown_and_join(transport, task)


async def test_agents_own_broadcast_is_not_perceived_back() -> None:
    class Speaker(Recording):
        async def handle_think(self, payload: Any) -> None:
            await super().handle_think(payload)
            self.act("my own words")

    agent = Speaker(ROOT, "alice", config={"grace_period": 0.01})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    agent.think("speak")
    await wait_for(lambda: len(published(transport, PUBLIC)) == 1, message="broadcast")
    await asyncio.sleep(0.02)  # give the echo a chance to (wrongly) arrive
    assert agent.perceived == []
    await shutdown_and_join(transport, task)


async def test_schedule_think_and_periodic_think_enter_the_merged_stream() -> None:
    agent = Recording(ROOT, "alice", config={"grace_period": 0.01})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    await asyncio.sleep(0)  # let serve() start
    agent.schedule_think(0.01, "later")
    periodic = agent.schedule_periodic_think(0.01, "tick")
    await wait_for(
        lambda: "later" in agent.thoughts and agent.thoughts.count("tick") >= 2,
        message="scheduled thinks",
    )
    periodic.cancel()
    await shutdown_and_join(transport, task)


async def test_log_event_publishes_immediately_to_the_log_subject() -> None:
    agent = Agent(ROOT, "alice")
    transport = await attach(agent)
    await agent.log_event({"considered": "speaking", "chose": "silence"})
    assert [(s, e.payload["chose"]) for s, e in decoded(transport)] == [
        (f"{ROOT}.log.alice", "silence")
    ]


async def test_spawned_task_exceptions_are_surfaced_as_logged_errors(
    caplog: pytest.LogCaptureFixture,
) -> None:
    agent = Agent(ROOT, "alice", config={"grace_period": 0.01})
    transport = await connected_transport()
    task = asyncio.create_task(agent.serve(transport))
    await asyncio.sleep(0)

    async def fails() -> None:
        raise RuntimeError("llm exploded")

    with caplog.at_level("ERROR", logger="freeagent.agent"):
        spawned = agent.spawn(fails())
        await asyncio.wait([spawned])
        await asyncio.sleep(0)  # let the done-callback run
    assert any("spawned task failed" in record.message for record in caplog.records)
    await shutdown_and_join(transport, task)
