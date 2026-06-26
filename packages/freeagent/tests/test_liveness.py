"""Mid-episode liveness: a lost role aborts the episode (ADR-0005, #55).

While RUNNING the environment periodically re-requests presence from every role
that was present at start. A role that was present but stops answering past the
liveness timeout is *lost* -- its child vanished -- and a lost role drives a clean
graceful abort (``stopping`` -> ``aborted``). No role is ever re-run or
re-recruited into a live episode. All on the in-memory transport, no NATS.
"""

from __future__ import annotations

import asyncio
import contextlib
from typing import TYPE_CHECKING

from freeagent_testing import connected_transport, published, wait_for

from freeagent import Agent, Environment, EpisodeState

if TYPE_CHECKING:
    from freeagent import MemoryTransport

APP = "demo"
EPISODE = "e1"

# Liveness fast enough to fire well inside a test, with a healthy round-trip
# comfortably under the per-probe deadline.
LIVE = {
    "setup_timeout": 1.0,
    "episode_timeout": 30.0,
    "grace_period": 0.05,
    "liveness_interval": 0.05,
    "liveness_timeout": 0.1,
}
AGENT_FAST = {"grace_period": 0.02}


async def _drive(env: Environment, transport: MemoryTransport) -> EpisodeState:
    """Run the environment to its terminal state."""
    return await asyncio.wait_for(env.serve(transport), timeout=10.0)


async def test_present_role_that_goes_silent_aborts_the_episode() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config=LIVE)
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    alice_task = asyncio.create_task(alice.serve(transport))
    env_task = asyncio.create_task(env.serve(transport))

    # Let the episode start with alice present.
    await wait_for(lambda: env.state is EpisodeState.RUNNING, message="running")
    # Kill the child: cancel its serve loop so it stops answering presence
    # re-probes. From the environment's side this is exactly a vanished child.
    alice_task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await alice_task

    state = await asyncio.wait_for(env_task, timeout=10.0)

    assert state is EpisodeState.ABORTED
    assert env.abort_reason is not None
    assert "alice" in env.abort_reason
    assert "lost role" in env.abort_reason
    # The lost role drove the *graceful* path: a shutdown broadcast went out and
    # the episode passed through STOPPING before ABORTED.
    assert EpisodeState.STOPPING in env.state_history
    assert env.state_history[-1] is EpisodeState.ABORTED
    control = [e.payload["type"] for e in published(transport, f"{APP}.episode.{EPISODE}.control")]
    assert control == ["start", "shutdown"]


async def test_a_live_episode_with_responsive_agents_is_not_aborted() -> None:
    """Liveness never fires on agents that keep answering -- it ends normally."""
    transport = await connected_transport()

    class ShortEnv(Environment):
        async def perceive(self, message: object) -> None:
            return

    env = ShortEnv(APP, ["alice"], episode_id=EPISODE, config={**LIVE, "episode_timeout": 0.4})
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    alice_task = asyncio.create_task(alice.serve(transport))
    try:
        state = await _drive(env, transport)
    finally:
        alice_task.cancel()

    # Several liveness probes fired during the run, yet the episode ended on the
    # episode timeout, not on a false-positive liveness abort.
    assert state is EpisodeState.ENDED
    assert env.abort_reason is None


async def test_liveness_can_be_disabled() -> None:
    """``liveness_interval`` 0 means no probe is ever scheduled."""
    transport = await connected_transport()
    env = Environment(
        APP,
        ["alice"],
        episode_id=EPISODE,
        config={**LIVE, "liveness_interval": 0.0, "episode_timeout": 0.3},
    )
    alice = Agent(env.subject_root, "alice", config=AGENT_FAST)
    alice_task = asyncio.create_task(alice.serve(transport))
    env_task = asyncio.create_task(env.serve(transport))
    # Kill the agent right after start: with liveness off, this never aborts the
    # episode -- it simply runs to its episode timeout.
    await wait_for(lambda: env.state is EpisodeState.RUNNING, message="running")
    alice_task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await alice_task

    state = await asyncio.wait_for(env_task, timeout=10.0)
    assert state is EpisodeState.ENDED
    assert env.abort_reason is None


async def test_probe_only_targets_roles_present_at_start() -> None:
    """A probe round asks exactly the members present, and a reply marks them seen."""
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config=LIVE)
    # Hand-drive into RUNNING with alice present.
    env._state = EpisodeState.RUNNING
    env._present = {"alice"}
    env._transport = transport

    await env._run_liveness_probe()
    assert env._liveness_probed == {"alice"}
    assert env._liveness_seen == set()
    # A presence request went to alice's inbox.
    inbox = env.subjects.agent("alice")
    assert any(s == inbox for s, _ in transport.history)

    # Simulate alice answering: she is marked seen, so the deadline finds no loss.
    env._liveness_seen.add("alice")
    env._check_liveness_deadline()
    assert env.state is EpisodeState.RUNNING  # no abort initiated
    assert env.abort_reason is None
