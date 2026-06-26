"""Model resolution and the deterministic fake LLM."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any

import pytest
from pydantic import BaseModel, ValidationError

from freeagent import FakeLLM, FakeLLMError, ModelResolutionError, create_llm, resolve_model
from freeagent.llm import KEY_MODELS, MODEL_ENV_VAR

if TYPE_CHECKING:
    from pathlib import Path

ALL_VARS = (MODEL_ENV_VAR, *(key for key, _ in KEY_MODELS))


@pytest.fixture
def clean_env(monkeypatch: pytest.MonkeyPatch) -> pytest.MonkeyPatch:
    """No model env var, no provider keys -- whatever the host shell has set."""
    for var in ALL_VARS:
        monkeypatch.delenv(var, raising=False)
    return monkeypatch


# ---------------------------------------------------------------------------
# resolve_model: explicit -> configured -> env var -> key auto-detect -> error
# ---------------------------------------------------------------------------


def test_explicit_argument_wins(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv(MODEL_ENV_VAR, "env/model")
    assert resolve_model("explicit/model", "configured/model") == "explicit/model"


def test_configured_beats_env_var(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv(MODEL_ENV_VAR, "env/model")
    assert resolve_model(None, "configured/model") == "configured/model"


def test_env_var_beats_auto_detect(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv(MODEL_ENV_VAR, "env/model")
    clean_env.setenv("ANTHROPIC_API_KEY", "sk-ant")
    assert resolve_model() == "env/model"


def test_auto_detect_anthropic(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv("ANTHROPIC_API_KEY", "sk-ant")
    assert resolve_model() == "anthropic/claude-haiku-4-5"


def test_auto_detect_openai(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv("OPENAI_API_KEY", "sk-oai")
    assert resolve_model() == "openai/gpt-4o-mini"


def test_auto_detect_gemini(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv("GEMINI_API_KEY", "g-key")
    assert resolve_model() == "gemini/gemini-2.0-flash"


def test_auto_detect_order_anthropic_first(clean_env: pytest.MonkeyPatch) -> None:
    clean_env.setenv("OPENAI_API_KEY", "sk-oai")
    clean_env.setenv("GEMINI_API_KEY", "g-key")
    clean_env.setenv("ANTHROPIC_API_KEY", "sk-ant")
    assert resolve_model() == "anthropic/claude-haiku-4-5"


def test_no_model_raises_a_clear_error_naming_every_env_var(
    clean_env: pytest.MonkeyPatch,
) -> None:
    with pytest.raises(ModelResolutionError) as excinfo:
        resolve_model()
    message = str(excinfo.value)
    for var in ALL_VARS:
        assert var in message


# ---------------------------------------------------------------------------
# FakeLLM: programmatic mode
# ---------------------------------------------------------------------------


async def test_queued_responses_are_consumed_in_order() -> None:
    fake = FakeLLM().queue("first").queue("second")
    assert await fake.complete("anything") == "first"
    assert await fake.complete("anything") == "second"
    with pytest.raises(FakeLLMError, match="no scripted response"):
        await fake.complete("anything")


async def test_first_unconsumed_matching_entry_wins() -> None:
    fake = FakeLLM().queue("about cats", match="cat").queue("about dogs", match="dog")
    assert await fake.complete("tell me about dogs") == "about dogs"
    assert await fake.complete("tell me about cats") == "about cats"
    with pytest.raises(FakeLLMError):
        await fake.complete("tell me about dogs")  # consumed


async def test_repeat_entries_are_never_consumed() -> None:
    fake = FakeLLM().queue("evergreen", repeat=True)
    assert await fake.complete("one") == "evergreen"
    assert await fake.complete("two") == "evergreen"


async def test_default_is_the_fallback_when_nothing_matches() -> None:
    fake = FakeLLM().queue("specific", match="rare").queue("fallback", default=True)
    assert await fake.complete("ordinary prompt") == "fallback"
    assert await fake.complete("a rare prompt") == "specific"


async def test_prompt_text_is_all_message_contents_concatenated() -> None:
    fake = FakeLLM().queue("found", match="needle")
    messages = [
        {"role": "system", "content": "you are a needle finder"},
        {"role": "user", "content": "go"},
    ]
    assert await fake.complete(messages) == "found"


async def test_shared_script_selected_via_the_model_string_fake() -> None:
    FakeLLM.respond("scripted reply")
    llm = create_llm("fake")
    assert isinstance(llm, FakeLLM)
    assert llm.model == "fake"
    assert await llm.complete("hello") == "scripted reply"


async def test_shared_default_and_reset() -> None:
    FakeLLM.respond("the default", default=True)
    assert await create_llm("fake").complete("x") == "the default"
    FakeLLM.reset()
    with pytest.raises(FakeLLMError):
        await create_llm("fake").complete("x")


# ---------------------------------------------------------------------------
# FakeLLM: structured output
# ---------------------------------------------------------------------------


class Verdict(BaseModel):
    speak: bool
    message: str


async def test_mapping_response_serializes_to_json_for_plain_calls() -> None:
    fake = FakeLLM().queue({"speak": True, "message": "hi"})
    text = await fake.complete("plain")
    assert isinstance(text, str)
    assert json.loads(text) == {"speak": True, "message": "hi"}


async def test_mapping_response_validates_against_the_schema() -> None:
    fake = FakeLLM().queue({"speak": True, "message": "hi"})
    verdict = await fake.complete("structured", schema=Verdict)
    assert isinstance(verdict, Verdict)
    assert verdict.speak is True
    assert verdict.message == "hi"


async def test_string_json_response_also_validates_against_the_schema() -> None:
    fake = FakeLLM().queue('{"speak": false, "message": ""}')
    verdict = await fake.complete("structured", schema=Verdict)
    assert isinstance(verdict, Verdict)
    assert verdict.speak is False


async def test_schema_violations_raise_validation_errors() -> None:
    fake = FakeLLM().queue({"speak": "not a bool at all", "message": 7})
    with pytest.raises(ValidationError):
        await fake.complete("structured", schema=Verdict)


# ---------------------------------------------------------------------------
# FakeLLM: canned YAML files, selected via "fake:<path>"
# ---------------------------------------------------------------------------

CANNED = """\
default: "fallback answer"
responses:
  - match: "weather"
    response: "sunny"
  - match: "verdict"
    response:
      speak: true
      message: "guilty"
    repeat: true
  - response: "consumed once"
"""


@pytest.fixture
def canned_file(tmp_path: Path) -> Path:
    path = tmp_path / "responses.yml"
    path.write_text(CANNED, encoding="utf-8")
    return path


async def test_canned_file_match_consume_repeat_default_semantics(canned_file: Path) -> None:
    llm = create_llm(f"fake:{canned_file}")
    assert isinstance(llm, FakeLLM)
    assert llm.model == f"fake:{canned_file}"
    # match wins over the earlier-but-nonmatching and later entries
    assert await llm.complete("what's the weather?") == "sunny"
    # entries without match always match; consumed once
    assert await llm.complete("anything") == "consumed once"
    # repeat entries are never consumed
    verdict1 = await llm.complete("verdict please", schema=Verdict)
    verdict2 = await llm.complete("verdict again", schema=Verdict)
    assert isinstance(verdict1, Verdict)
    assert isinstance(verdict2, Verdict)
    assert verdict1.message == verdict2.message == "guilty"
    # everything else falls back to default
    assert await llm.complete("anything else") == "fallback answer"


async def test_canned_file_errors_are_clear(tmp_path: Path) -> None:
    bad = tmp_path / "bad.yml"
    bad.write_text("responses:\n  - match: x\n", encoding="utf-8")
    with pytest.raises(FakeLLMError, match="response"):
        FakeLLM(bad)


# ---------------------------------------------------------------------------
# Telemetry
# ---------------------------------------------------------------------------


async def test_telemetry_callback_receives_prompt_completion_and_timing() -> None:
    records: list[dict[str, Any]] = []

    async def telemetry(record: dict[str, Any]) -> None:
        records.append(record)

    fake = FakeLLM(telemetry=telemetry).queue({"speak": True, "message": "yo"})
    await fake.complete("the prompt", schema=Verdict)
    assert len(records) == 1
    record = records[0]
    assert record["type"] == "freeagent.llm_call"
    assert record["model"] == "fake"
    assert record["messages"] == [{"role": "user", "content": "the prompt"}]
    assert json.loads(record["response"]) == {"speak": True, "message": "yo"}
    assert record["schema"] == "Verdict"
    assert record["duration_s"] >= 0


# ---------------------------------------------------------------------------
# api_key threading into litellm.acompletion
# ---------------------------------------------------------------------------


class _FakeMessage:
    def __init__(self, content: str) -> None:
        self.content = content


class _FakeChoice:
    def __init__(self, content: str) -> None:
        self.message = _FakeMessage(content)


class _FakeResponse:
    def __init__(self, content: str) -> None:
        self.choices = [_FakeChoice(content)]


@pytest.fixture
def capture_acompletion(monkeypatch: pytest.MonkeyPatch) -> list[dict[str, Any]]:
    """Replace ``litellm.acompletion`` with a stub recording its kwargs."""
    import litellm

    calls: list[dict[str, Any]] = []

    async def fake_acompletion(**kwargs: Any) -> _FakeResponse:
        calls.append(kwargs)
        return _FakeResponse("ok")

    monkeypatch.setattr(litellm, "acompletion", fake_acompletion)
    return calls


async def test_config_api_key_reaches_litellm(
    clean_env: pytest.MonkeyPatch, capture_acompletion: list[dict[str, Any]]
) -> None:
    llm = create_llm("anthropic/claude-haiku-4-5", api_key="sk-from-config")
    assert await llm.complete("hi") == "ok"
    assert capture_acompletion[0]["api_key"] == "sk-from-config"


async def test_no_api_key_omits_the_kwarg_so_env_var_path_still_works(
    clean_env: pytest.MonkeyPatch, capture_acompletion: list[dict[str, Any]]
) -> None:
    # No api_key supplied: litellm must fall back to the environment, so the
    # kwarg is omitted entirely rather than passed as None.
    llm = create_llm("anthropic/claude-haiku-4-5")
    assert await llm.complete("hi") == "ok"
    assert "api_key" not in capture_acompletion[0]
