"""End-to-end episodes on the in-memory transport -- no NATS, no network.

The fake-LLM episode is driven by the very scripts shipped in
``examples/fake/``, so the test also validates the example configs' canned
game for the runner e2e. The real-model smoke test runs only when a provider
API key is present.
"""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from freeagent import Envelope, EpisodeState, MemoryTransport
from twentyquestions import Host, Player, TwentyQuestionsEnvironment

if TYPE_CHECKING:
    from collections.abc import Sequence

    from freeagent import LLMAgent

REPO_ROOT = Path(__file__).resolve().parents[3]
FAKE_SCRIPTS = REPO_ROOT / "examples" / "fake"

PLAYERS = ("alice", "bob", "carol")
ROSTER = ("host", *PLAYERS)


def fake_model(name: str) -> str:
    return f"fake:{FAKE_SCRIPTS / name}"


def decoded(transport: MemoryTransport) -> list[tuple[str, Envelope]]:
    return [(subject, Envelope.from_bytes(data)) for subject, data in transport.history]


async def run_episode(
    env: TwentyQuestionsEnvironment,
    agents: Sequence[LLMAgent],
    transport: MemoryTransport,
    deadline_s: float,
) -> EpisodeState:
    """Serve the environment and every agent against one shared transport."""
    env_task = asyncio.create_task(env.serve(transport))
    agent_tasks = [asyncio.create_task(agent.serve(transport)) for agent in agents]
    state = await asyncio.wait_for(env_task, timeout=deadline_s)
    await asyncio.wait_for(asyncio.gather(*agent_tasks), timeout=deadline_s)
    return state


async def test_environment_reacts_only_to_the_hosts_game_over_signal() -> None:
    env = TwentyQuestionsEnvironment("twentyquestions", ["host"], episode_id="unit")
    await env.perceive(Envelope(episode_id="unit", sender="alice", payload="just chatter"))
    with pytest.raises(asyncio.QueueEmpty):
        env._events.get_nowait()
    payload = {"type": "game_over", "outcome": "win", "secret": "x", "questions_asked": 2}
    await env.perceive(Envelope(episode_id="unit", sender="host", payload=payload))
    assert env.outcome == payload
    event = env._events.get_nowait()
    assert event.name == "freeagent.shutdown"
    assert event.payload == "game over"


async def test_full_episode_with_the_canned_example_scripts() -> None:
    transport = MemoryTransport()
    env = TwentyQuestionsEnvironment(
        "twentyquestions",
        list(ROSTER),
        episode_id="e2e",
        config={"setup_timeout": 2.0, "episode_timeout": 10.0, "grace_period": 0.2},
    )
    host = Host(
        env.subject_root,
        "host",
        config={"model": fake_model("host.yml"), "secret": "an octopus", "grace_period": 0.2},
    )
    players = [
        Player(
            env.subject_root,
            name,
            config={"model": fake_model(f"{name}.yml"), "grace_period": 0.2},
        )
        for name in PLAYERS
    ]

    state = await run_episode(env, [host, *players], transport, deadline_s=10.0)

    # The episode ended because the Host signalled game over.
    assert state is EpisodeState.ENDED
    assert env.shutdown_reason == "game over"
    assert env.outcome == {
        "type": "game_over",
        "outcome": "win",
        "secret": "an octopus",
        "questions_asked": 2,
    }
    # The question count behaved: classified by LLM, counted in code.
    assert host.questions_asked == 2
    assert host.game_over

    wire = decoded(transport)
    public = f"{env.subject_root}.public"
    speech = [(e.sender, e.payload) for s, e in wire if s == public]
    host_lines = [text for sender, text in speech if sender == "host"]
    assert "Welcome to Twenty Questions" in host_lines[0]
    assert any("question 1 of 20" in line for line in host_lines)
    assert any("question 2 of 20" in line for line in host_lines)

    # The win announcement appeared on the wire (the episode's outcome record).
    announcement = next(line for line in host_lines if line.startswith("GAME OVER"))
    assert "win" in announcement
    assert "an octopus" in announcement

    # Every player said goodbye -- after the shutdown broadcast (inside the
    # grace period) and after the announcement.
    goodbye_indices = {
        e.sender: i
        for i, (s, e) in enumerate(wire)
        if s == public and isinstance(e.payload, str) and "Goodbye" in e.payload
    }
    assert set(goodbye_indices) == set(PLAYERS)
    announcement_index = next(
        i for i, (s, e) in enumerate(wire) if s == public and e.payload == announcement
    )
    control = f"{env.subject_root}.control"
    shutdown_index = next(
        i
        for i, (s, e) in enumerate(wire)
        if s == control and isinstance(e.payload, dict) and e.payload.get("type") == "shutdown"
    )
    assert all(i > announcement_index for i in goodbye_indices.values())
    assert all(i > shutdown_index for i in goodbye_indices.values())


_KEY_VARS = ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY")
_TEST_LLM_VAR = "FREEAGENT_TEST_LLM"


@pytest.mark.skipif(
    not os.environ.get(_TEST_LLM_VAR),
    reason=f"real-model test is opt-in: set {_TEST_LLM_VAR} to run it (it calls a real LLM)",
)
@pytest.mark.skipif(
    not any(os.environ.get(var) for var in _KEY_VARS),
    reason=f"real-model smoke test needs a provider API key ({', '.join(_KEY_VARS)})",
)
async def test_real_model_smoke() -> None:
    """A short real-LLM game: small budget, episode timeout as the backstop."""
    transport = MemoryTransport()
    env = TwentyQuestionsEnvironment(
        "twentyquestions",
        list(ROSTER),
        episode_id="smoke",
        config={"setup_timeout": 10.0, "episode_timeout": 90.0, "grace_period": 2.0},
    )
    host = Host(
        env.subject_root,
        "host",
        config={"secret": "a bicycle", "max_questions": 5, "grace_period": 2.0},
    )
    players = [Player(env.subject_root, name, config={"grace_period": 2.0}) for name in PLAYERS]
    state = await run_episode(env, [host, *players], transport, deadline_s=120.0)
    assert state is EpisodeState.ENDED
    public = f"{env.subject_root}.public"
    host_lines = [e.payload for s, e in decoded(transport) if s == public and e.sender == "host"]
    assert any("Welcome to Twenty Questions" in str(line) for line in host_lines)
