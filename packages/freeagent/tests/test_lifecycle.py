"""The episode lifecycle state machine, end to end on the in-memory transport.

setup -> running -> stopping -> ended, plus aborted: presence confirmation by
request/reply with per-process nonces, setup timeout, episode timeout, the
agent-inbox shutdown signal, and grace-period ordering -- all without NATS.
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

import pytest
from freeagent_testing import connected_transport, decoded, published, wait_for

from freeagent import Agent, Envelope, Environment, EpisodeState

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

    from freeagent import MemoryTransport

APP = "demo"
EPISODE = "e1"

FAST = {"setup_timeout": 0.5, "episode_timeout": 5.0, "grace_period": 0.05}
AGENT_FAST = {"grace_period": 0.02}


class SignalEnv(Environment):
    """Records what it perceives; shuts down on an agent's 'done' inbox signal."""

    def __init__(
        self,
        app: str,
        roster: Sequence[str],
        episode_id: str | None = None,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(app, roster, episode_id, config)
        self.seen: list[tuple[str, Any, EpisodeState]] = []

    async def perceive(self, message: Envelope) -> None:
        self.seen.append((message.sender, message.payload, self.state))
        if message.payload == "done":
            self.initiate_shutdown("agent signal")


class SignalingAgent(Agent):
    """Tells the environment's inbox the episode is over as soon as it starts."""

    async def control(self, message: Envelope) -> None:
        await super().control(message)
        if isinstance(message.payload, dict) and message.payload.get("type") == "start":
            self.act("done", recipients=["env"])


class GoodbyeAgent(Agent):
    """Broadcasts a goodbye during the stopping grace period."""

    async def control(self, message: Envelope) -> None:
        await super().control(message)
        if isinstance(message.payload, dict) and message.payload.get("type") == "shutdown":
            self.act("goodbye")


async def run_episode(
    env: Environment,
    agents: Sequence[Agent],
    transport: MemoryTransport,
    deadline_s: float = 5.0,
) -> EpisodeState:
    agent_tasks = [asyncio.create_task(agent.serve(transport)) for agent in agents]
    try:
        state = await asyncio.wait_for(env.serve(transport), timeout=deadline_s)
        await asyncio.wait_for(asyncio.gather(*agent_tasks), timeout=deadline_s)
        return state
    finally:
        for task in agent_tasks:
            task.cancel()


# ---------------------------------------------------------------------------
# Constructor validation
# ---------------------------------------------------------------------------


def test_duplicate_roster_names_rejected() -> None:
    with pytest.raises(ValueError, match="duplicate"):
        Environment(APP, ["alice", "bob", "alice"])


def test_env_reserved_in_roster() -> None:
    with pytest.raises(ValueError, match="reserved"):
        Environment(APP, ["alice", "env"])


def test_invalid_names_rejected() -> None:
    with pytest.raises(ValueError, match="application name"):
        Environment("bad app", ["alice"])
    with pytest.raises(ValueError, match="agent id"):
        Environment(APP, ["bad agent"])
    with pytest.raises(ValueError, match="episode id"):
        Environment(APP, ["alice"], episode_id="bad id")


def test_episode_id_auto_generated_and_subject_safe() -> None:
    env = Environment(APP, ["alice"])
    assert env.episode_id
    assert env.subject_root == f"{APP}.episode.{env.episode_id}"


# ---------------------------------------------------------------------------
# Lifecycle
# ---------------------------------------------------------------------------


async def test_happy_path_setup_running_stopping_ended() -> None:
    transport = await connected_transport()
    env = SignalEnv(APP, ["alice", "bob"], episode_id=EPISODE, config=FAST)
    agents = [
        SignalingAgent(env.subject_root, "alice", config=AGENT_FAST),
        Agent(env.subject_root, "bob", config=AGENT_FAST),
    ]
    state = await run_episode(env, agents, transport)

    assert state is EpisodeState.ENDED
    assert env.state_history == [
        EpisodeState.SETUP,
        EpisodeState.RUNNING,
        EpisodeState.STOPPING,
        EpisodeState.ENDED,
    ]
    assert env.shutdown_reason == "agent signal"
    # The environment created the episode's stream (idempotently) during setup.
    assert transport.streams[f"{APP}_episode_{EPISODE}"] == (f"{APP}.episode.{EPISODE}.>",)
    # The gun fired before the wind-down: start precedes shutdown on control.
    control = [e.payload["type"] for e in published(transport, f"{APP}.episode.{EPISODE}.control")]
    assert control == ["start", "shutdown"]
    # The inbox signal reached the environment's perceive while running.
    assert ("alice", "done", EpisodeState.RUNNING) in env.seen
    # Nothing in the episode ever leaves its subject root (no _INBOX.> replies).
    assert all(s.startswith(f"{APP}.episode.{EPISODE}.") for s, _ in decoded(transport))
    # Both agents wound down.
    assert all(agent.phase == "stopping" for agent in agents)


async def test_missing_agent_aborts_at_setup_timeout() -> None:
    transport = await connected_transport()
    env = Environment(
        APP, ["alice", "ghost"], episode_id=EPISODE, config={**FAST, "setup_timeout": 0.05}
    )
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    state = await run_episode(env, [alice], transport)

    assert state is EpisodeState.ABORTED
    assert env.state_history == [EpisodeState.SETUP, EpisodeState.ABORTED]
    assert env.abort_reason is not None
    assert "ghost" in env.abort_reason
    # No start was ever broadcast; the abort's cooperative shutdown was.
    control = [e.payload["type"] for e in published(transport, f"{APP}.episode.{EPISODE}.control")]
    assert control == ["shutdown"]


async def test_duplicate_name_with_two_nonces_aborts() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice", "bob"], episode_id=EPISODE, config=FAST)
    # Two processes launched under the same name: two Agent instances, two
    # per-process nonces, both answering alice's presence request. bob never
    # shows up, so the episode is still in setup when the clash is detected.
    impostors = [
        Agent(env.subject_root, "alice", config=AGENT_FAST),
        Agent(env.subject_root, "alice", config=AGENT_FAST),
    ]
    state = await run_episode(env, impostors, transport)

    assert state is EpisodeState.ABORTED
    assert env.abort_reason is not None
    assert "duplicate agent name 'alice'" in env.abort_reason


async def test_episode_timeout_initiates_shutdown() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config={**FAST, "episode_timeout": 0.05})
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    state = await run_episode(env, [alice], transport)

    assert state is EpisodeState.ENDED
    assert env.shutdown_reason == "episode timeout"
    assert env.state_history == [
        EpisodeState.SETUP,
        EpisodeState.RUNNING,
        EpisodeState.STOPPING,
        EpisodeState.ENDED,
    ]


async def test_grace_period_lets_goodbyes_land_before_ended() -> None:
    transport = await connected_transport()
    env = SignalEnv(
        APP,
        ["alice"],
        episode_id=EPISODE,
        config={**FAST, "episode_timeout": 0.05, "grace_period": 0.2},
    )
    alice = GoodbyeAgent(env.subject_root, "alice", config={"grace_period": 0.05})
    state = await run_episode(env, [alice], transport)

    assert state is EpisodeState.ENDED
    # The goodbye was broadcast after the shutdown control message and
    # perceived by the environment while STOPPING -- i.e. inside the grace
    # period, before ENDED.
    assert ("alice", "goodbye", EpisodeState.STOPPING) in env.seen


async def test_agents_start_idle_and_activate_on_the_gun() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config={**FAST, "episode_timeout": 0.1})
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    assert alice.phase == "idle"
    task = asyncio.create_task(alice.serve(transport))
    env_task = asyncio.create_task(env.serve(transport))
    await wait_for(lambda: alice.phase == "active", message="the start broadcast")
    assert env.state is EpisodeState.RUNNING
    assert await asyncio.wait_for(env_task, timeout=5.0) is EpisodeState.ENDED
    await asyncio.wait_for(task, timeout=5.0)
