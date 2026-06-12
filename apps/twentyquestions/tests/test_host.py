"""Unit tests for the Host's programmatic side: counting, win, loss, kickoff.

These drive :meth:`Host.on_decision` directly with :class:`HostDecision`
instances -- no transport, no LLM call -- and inspect the unflushed outbox.
"""

from __future__ import annotations

from typing import Any

from freeagent import Envelope
from freeagent.agent import ThinkEvent
from twentyquestions import CANNED_SECRETS, Host, HostDecision
from twentyquestions.host import _KICKOFF

ROOT = "twentyquestions.episode.unit"


def make_host(**config: Any) -> Host:
    return Host(ROOT, "host", config={"model": "fake", "secret": "an octopus", **config})


def outbox(host: Host) -> list[tuple[Any, tuple[str, ...] | None]]:
    return host._outbox


def question(n: int) -> HostDecision:
    return HostDecision(
        speak=True, message=f"Yes. That was question {n} of 20.", classification="question"
    )


async def test_question_verdict_increments_the_count() -> None:
    host = make_host()
    await host.on_decision(question(1))
    assert host.questions_asked == 1
    assert not host.game_over
    assert outbox(host) == [("Yes. That was question 1 of 20.", None)]


async def test_deliberation_and_other_verdicts_do_not_count() -> None:
    host = make_host()
    await host.on_decision(HostDecision(speak=False, classification="deliberation"))
    await host.on_decision(HostDecision(speak=False, classification="other"))
    assert host.questions_asked == 0
    assert outbox(host) == []


async def test_wrong_guess_neither_counts_nor_ends_the_game() -> None:
    host = make_host()
    await host.on_decision(
        HostDecision(speak=True, message="No, it is not a squid.", classification="guess")
    )
    assert host.questions_asked == 0
    assert not host.game_over


async def test_correct_guess_wins_announces_and_signals_the_environment() -> None:
    host = make_host()
    await host.on_decision(question(1))
    await host.on_decision(
        HostDecision(speak=True, message="Correct!", classification="guess", guess_correct=True)
    )
    assert host.game_over
    payloads = outbox(host)
    announcement = payloads[-2][0]
    assert payloads[-2][1] is None  # the announcement is public
    assert "GAME OVER" in announcement
    assert "win" in announcement
    assert "an octopus" in announcement
    assert payloads[-1] == (
        {"type": "game_over", "outcome": "win", "secret": "an octopus", "questions_asked": 1},
        ("env",),
    )
    # The announcement is also part of the Host's own transcript.
    assert ("host", announcement) in host.transcript


async def test_budget_exhausted_is_a_loss() -> None:
    host = make_host(max_questions=2)
    await host.on_decision(question(1))
    assert not host.game_over
    await host.on_decision(question(2))
    assert host.game_over
    assert host.questions_asked == 2
    payloads = outbox(host)
    announcement = payloads[-2][0]
    assert "GAME OVER" in announcement
    assert "lose" in announcement
    assert payloads[-1][0]["outcome"] == "loss"
    assert payloads[-1][1] == ("env",)


async def test_after_game_over_nothing_counts_and_nothing_is_announced_again() -> None:
    host = make_host(max_questions=1)
    await host.on_decision(question(1))
    assert host.game_over
    before = len(outbox(host))
    await host.on_decision(HostDecision(speak=True, message="Goodbye!", classification="other"))
    await host.on_decision(HostDecision(speak=False, classification="question"))
    await host.on_decision(HostDecision(speak=False, classification="guess", guess_correct=True))
    assert host.questions_asked == 1  # question verdicts no longer count
    # Speech (goodbyes) still flows, but no second announcement or env signal.
    assert outbox(host)[before:] == [("Goodbye!", None)]


async def test_secret_comes_from_config_or_the_canned_list() -> None:
    assert make_host().secret == "an octopus"
    host = Host(ROOT, "host", config={"model": "fake"})
    assert host.secret in CANNED_SECRETS


async def test_start_control_schedules_the_kickoff_welcome() -> None:
    host = make_host()
    await host.control(Envelope(episode_id="unit", sender="env", payload={"type": "start"}))
    assert host.phase == "active"
    event = host._events.get_nowait()
    assert isinstance(event, ThinkEvent)
    assert event.payload == _KICKOFF
    await host.handle_think(event.payload)
    welcome = outbox(host)[0][0]
    assert "Welcome to Twenty Questions" in welcome
    assert "20" in welcome
    assert ("host", welcome) in host.transcript


async def test_decision_prompt_carries_the_private_game_state() -> None:
    host = make_host()
    host.questions_asked = 3
    system = host.decision_messages()[0]["content"]
    assert "an octopus" in system
    assert "3 of 20" in system
