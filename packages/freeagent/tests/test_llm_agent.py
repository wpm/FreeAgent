"""The LLMAgent decide loop, driven by the FakeLLM on the in-memory transport."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

import pytest
from freeagent_testing import ROOT, connected_transport, decoded, publish, published, wait_for

from freeagent import Decision, FakeLLM, LLMAgent, ModelResolutionError
from freeagent.llm import KEY_MODELS, MODEL_ENV_VAR

if TYPE_CHECKING:
    from pydantic import BaseModel

    from freeagent import MemoryTransport
    from freeagent.llm import LLM, Messages

PUBLIC = f"{ROOT}.public"
CONTROL = f"{ROOT}.control"
LOG = f"{ROOT}.log.speaker"

FAKE_CONFIG = {"model": "fake", "grace_period": 0.01}


async def start_agent(agent: LLMAgent, transport: MemoryTransport) -> asyncio.Task[None]:
    task = asyncio.create_task(agent.serve(transport))
    await publish(transport, CONTROL, "env", {"type": "start"})
    return task


async def finish(transport: MemoryTransport, task: asyncio.Task[None]) -> None:
    await publish(transport, CONTROL, "env", {"type": "shutdown"})
    await asyncio.wait_for(task, timeout=2.0)


def spoken(transport: MemoryTransport, agent_id: str = "speaker") -> list[Any]:
    return [e.payload for e in published(transport, PUBLIC) if e.sender == agent_id]


def llm_calls(transport: MemoryTransport) -> list[Any]:
    return [e.payload for e in published(transport, LOG)]


async def test_decision_to_speak_is_broadcast_and_transcribed() -> None:
    FakeLLM.respond({"speak": True, "message": "hello there"})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "hi speaker")
    await wait_for(lambda: spoken(transport) == ["hello there"], message="the agent to speak")
    # State is the transcript of perceived messages plus the agent's own words.
    assert agent.transcript == [("user", "hi speaker"), ("speaker", "hello there")]
    await finish(transport, task)


async def test_decision_to_stay_silent_publishes_nothing() -> None:
    FakeLLM.respond({"speak": False, "message": ""})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "anyone there?")
    # Wait for the call to complete via its telemetry record, then check silence.
    await wait_for(lambda: len(llm_calls(transport)) == 1, message="the decision call")
    await asyncio.sleep(0.02)
    assert spoken(transport) == []
    assert agent.transcript == [("user", "anyone there?")]
    await finish(transport, task)


async def test_llm_call_telemetry_goes_to_the_log_subject_by_default() -> None:
    FakeLLM.respond({"speak": True, "message": "logged"})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "say something")
    await wait_for(lambda: len(llm_calls(transport)) == 1, message="telemetry")
    record = llm_calls(transport)[0]
    assert record["type"] == "freeagent.llm_call"
    assert record["model"] == "fake"
    assert record["schema"] == "Decision"
    assert "say something" in record["messages"][1]["content"]
    await finish(transport, task)


async def test_llm_telemetry_can_be_disabled() -> None:
    FakeLLM.respond({"speak": True, "message": "quiet logs"})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config={**FAKE_CONFIG, "llm_telemetry": False})
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "go")
    await wait_for(lambda: spoken(transport) == ["quiet logs"], message="the agent to speak")
    assert not any(s.startswith(f"{ROOT}.log.") for s, _ in decoded(transport))
    await finish(transport, task)


class _SlowLLM:
    """Wraps the agent's LLM with a delay so a call can be in flight."""

    def __init__(self, inner: LLM, delay: float) -> None:
        self._inner = inner
        self._delay = delay
        self.calls = 0

    async def complete(
        self, messages: Messages | str, *, schema: type[BaseModel] | None = None
    ) -> Any:
        self.calls += 1
        await asyncio.sleep(self._delay)
        return await self._inner.complete(messages, schema=schema)


async def test_messages_arriving_mid_call_trigger_one_reconsideration() -> None:
    # First decision (on the first message alone): stay silent. The
    # reconsideration (with both messages in the transcript): speak.
    FakeLLM.respond({"speak": False, "message": ""})
    FakeLLM.respond({"speak": True, "message": "final answer"})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    slow = _SlowLLM(agent.llm, delay=0.05)
    agent.llm = slow  # type: ignore[assignment]
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "first message")
    await publish(transport, PUBLIC, "user", "second message")
    await wait_for(lambda: spoken(transport) == ["final answer"], message="the reconsideration")
    # Two messages, but only two calls total: the second message did not
    # start a concurrent call, it marked the in-flight one for reconsideration.
    assert slow.calls == 2
    assert agent.transcript == [
        ("user", "first message"),
        ("user", "second message"),
        ("speaker", "final answer"),
    ]
    await finish(transport, task)


async def test_failed_decision_calls_are_absorbed_and_the_agent_recovers(
    caplog: pytest.LogCaptureFixture,
) -> None:
    # Nothing scripted: the first decision call raises FakeLLMError inside the
    # spawned task; the error re-enters the fold and is logged; the next
    # perceived message decides normally.
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    task = await start_agent(agent, transport)

    with caplog.at_level("ERROR", logger="freeagent.llm_agent"):
        await publish(transport, PUBLIC, "user", "unscripted")
        await wait_for(
            lambda: any("decision call failed" in r.message for r in caplog.records),
            message="the error to be logged",
        )
    FakeLLM.respond({"speak": True, "message": "recovered"})
    await publish(transport, PUBLIC, "user", "try again")
    await wait_for(lambda: spoken(transport) == ["recovered"], message="recovery")
    await finish(transport, task)


async def test_nudge_breaks_a_stalled_conversation() -> None:
    # The stall: the agent perceives a message and decides to stay silent;
    # nothing else ever arrives, so without a nudge it would never decide
    # again. The periodic nudge re-runs the decision with the lull hint.
    FakeLLM.respond({"speak": False, "message": ""})
    FakeLLM.respond(
        {"speak": True, "message": "breaking the silence"},
        match="quiet for a while",
    )
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config={**FAKE_CONFIG, "nudge_interval": 0.05})
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "someone should go first")
    await wait_for(
        lambda: spoken(transport) == ["breaking the silence"],
        message="the nudged decision to speak",
    )
    # The nudge hint reached the prompt of the second call only.
    calls = llm_calls(transport)
    assert len(calls) == 2
    assert "quiet for a while" not in calls[0]["messages"][1]["content"]
    assert "quiet for a while" in calls[1]["messages"][1]["content"]
    await finish(transport, task)


async def test_no_nudge_by_default() -> None:
    FakeLLM.respond({"speak": False, "message": ""})
    transport = await connected_transport()
    agent = LLMAgent(ROOT, "speaker", config=FAKE_CONFIG)
    task = await start_agent(agent, transport)

    await publish(transport, PUBLIC, "user", "anyone?")
    await wait_for(lambda: len(llm_calls(transport)) == 1, message="the decision call")
    await asyncio.sleep(0.15)  # several would-be nudge intervals
    assert len(llm_calls(transport)) == 1
    assert spoken(transport) == []
    await finish(transport, task)


def test_llm_agent_model_resolution_uses_the_standard_chain(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for var in (MODEL_ENV_VAR, *(key for key, _ in KEY_MODELS)):
        monkeypatch.delenv(var, raising=False)
    with pytest.raises(ModelResolutionError):
        LLMAgent(ROOT, "speaker")  # no model anywhere -> the clear error
    monkeypatch.setenv(MODEL_ENV_VAR, "fake")
    agent = LLMAgent(ROOT, "speaker")
    assert isinstance(agent.llm, FakeLLM)


def test_llm_agent_passes_config_api_key_to_create_llm(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_create_llm(**kwargs: Any) -> FakeLLM:
        captured.update(kwargs)
        return FakeLLM()

    monkeypatch.setattr("freeagent.llm_agent.create_llm", fake_create_llm)
    LLMAgent(ROOT, "speaker", {"model": "fake", "api_key": "sk-from-config"})
    assert captured["api_key"] == "sk-from-config"


async def test_decision_schema_is_the_pinned_speak_message_shape() -> None:
    assert set(Decision.model_fields) == {"speak", "message"}
    decision = Decision.model_validate({"speak": True, "message": "x"})
    assert decision.speak is True
