"""LLM infrastructure: model resolution, the async client, and the fake LLM.

The client is an async wrapper over litellm used **only via the
spawn-don't-block pattern**: an LLM call never happens inside a handler; the
call runs as a spawned task (:meth:`freeagent.agent.Agent.spawn`) and its
completion re-enters the agent's fold as a think message.

Model resolution order: explicit argument -> configured (runner yml) ->
``FREEAGENT_MODEL`` env var -> auto-detect from whichever provider API key is
present -> a clear error naming the env vars checked.

The deterministic :class:`FakeLLM` is selected through the same resolution
mechanism via the model strings ``"fake"`` (responses registered
programmatically) and ``"fake:<path>"`` (canned YAML file), so everything
downstream runs without network or keys.
"""

from __future__ import annotations

import json
import os
import re
import time
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any, ClassVar

import yaml

if TYPE_CHECKING:
    from pydantic import BaseModel

MODEL_ENV_VAR = "FREEAGENT_MODEL"

KEY_MODELS: tuple[tuple[str, str], ...] = (
    ("ANTHROPIC_API_KEY", "anthropic/claude-haiku-4-5"),
    ("OPENAI_API_KEY", "openai/gpt-4o-mini"),
    ("GEMINI_API_KEY", "gemini/gemini-2.0-flash"),
)
"""Auto-detection order: the cheapest tier of whichever provider key is present."""

Messages = list[dict[str, str]]
"""Chat messages: ``[{"role": ..., "content": ...}, ...]``."""

TelemetryCallback = Callable[[dict[str, Any]], Awaitable[None]]
"""Awaited with a record of each call's prompt, completion, and timing."""


class ModelResolutionError(RuntimeError):
    """No model could be resolved from arguments, config, or environment."""


class FakeLLMError(RuntimeError):
    """The fake LLM had no scripted response applicable to a prompt."""


def resolve_model(explicit: str | None = None, configured: str | None = None) -> str:
    """Resolve the litellm model string.

    Order: *explicit* argument -> *configured* (runner yml) -> the
    ``FREEAGENT_MODEL`` env var -> auto-detect from provider API keys
    (``ANTHROPIC_API_KEY``, ``OPENAI_API_KEY``, ``GEMINI_API_KEY``, in that
    order, picking each provider's cheap tier). Raises
    :class:`ModelResolutionError` naming every env var checked when nothing
    applies.
    """
    if explicit:
        return explicit
    if configured:
        return configured
    from_env = os.environ.get(MODEL_ENV_VAR)
    if from_env:
        return from_env
    for key, model in KEY_MODELS:
        if os.environ.get(key):
            return model
    checked = ", ".join(key for key, _ in KEY_MODELS)
    raise ModelResolutionError(
        "no LLM model configured: pass a model explicitly, set it in the runner "
        f"config, set {MODEL_ENV_VAR}, or export a provider API key ({checked})"
    )


class LLM:
    """Async LLM client over litellm.

    Use only via the spawn-don't-block pattern: never call
    :meth:`complete` inside a handler -- spawn it and post the result back to
    the think queue. With ``schema``, the completion is requested as JSON and
    validated with the Pydantic model, returning the model instance.

    ``telemetry``, if given, is awaited with a record of each call's prompt,
    completion, and timing -- typically published to the agent's log-only
    subject.
    """

    def __init__(
        self,
        model: str | None = None,
        *,
        configured: str | None = None,
        telemetry: TelemetryCallback | None = None,
    ) -> None:
        self.model = resolve_model(model, configured)
        self._telemetry = telemetry

    async def complete(
        self,
        messages: Messages | str,
        *,
        schema: type[BaseModel] | None = None,
    ) -> str | BaseModel:
        """Run one completion; returns text, or a validated *schema* instance."""
        normalized: Messages = (
            [{"role": "user", "content": messages}]
            if isinstance(messages, str)
            else [dict(m) for m in messages]
        )
        started = time.perf_counter()
        text = await self._invoke(normalized, schema)
        duration = time.perf_counter() - started
        result: str | BaseModel = text if schema is None else schema.model_validate_json(text)
        if self._telemetry is not None:
            await self._telemetry(
                {
                    "type": "freeagent.llm_call",
                    "model": self.model,
                    "messages": normalized,
                    "response": text,
                    "schema": None if schema is None else schema.__name__,
                    "duration_s": duration,
                }
            )
        return result

    async def _invoke(self, messages: Messages, schema: type[BaseModel] | None) -> str:
        """Perform the provider call, returning the raw completion text."""
        import litellm  # deferred: litellm is heavy and the fake path never needs it

        kwargs: dict[str, Any] = {}
        if schema is not None:
            kwargs["response_format"] = schema
        response = await litellm.acompletion(model=self.model, messages=messages, **kwargs)
        content = response.choices[0].message.content  # type: ignore[union-attr,index]
        if not isinstance(content, str):
            raise RuntimeError(f"model {self.model} returned an empty completion")
        return content


# ---------------------------------------------------------------------------
# Fake LLM
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class _ScriptEntry:
    response: str | Mapping[str, Any]
    match: re.Pattern[str] | None = None
    repeat: bool = False
    consumed: bool = False


@dataclass(slots=True)
class _Script:
    entries: list[_ScriptEntry] = field(default_factory=list)
    default: str | Mapping[str, Any] | None = None

    def add(
        self,
        response: str | Mapping[str, Any],
        match: str | None = None,
        repeat: bool = False,
    ) -> None:
        pattern = re.compile(match) if match is not None else None
        self.entries.append(_ScriptEntry(response=response, match=pattern, repeat=repeat))

    def take(self, prompt: str) -> str | Mapping[str, Any]:
        for entry in self.entries:
            if entry.consumed:
                continue
            if entry.match is not None and not entry.match.search(prompt):
                continue
            if not entry.repeat:
                entry.consumed = True
            return entry.response
        if self.default is not None:
            return self.default
        snippet = prompt if len(prompt) <= 200 else prompt[:200] + "..."
        raise FakeLLMError(f"no scripted response applies to prompt: {snippet!r}")

    def clear(self) -> None:
        self.entries.clear()
        self.default = None


def _load_script(path: Path) -> _Script:
    """Load a canned-response YAML file.

    Format: a top-level mapping with optional ``default:`` (used when nothing
    matches) and ``responses:`` -- a list of entries
    ``{match: <python regex, optional>, response: <string or mapping>,
    repeat: <bool, default false>}``.
    """
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    if data is None:
        data = {}
    if not isinstance(data, dict):
        raise FakeLLMError(f"canned response file {path} must be a mapping")
    script = _Script()
    default = data.get("default")
    if default is not None:
        if not isinstance(default, str | dict):
            raise FakeLLMError(f"{path}: 'default' must be a string or mapping")
        script.default = default
    responses = data.get("responses") or []
    if not isinstance(responses, list):
        raise FakeLLMError(f"{path}: 'responses' must be a list")
    for i, entry in enumerate(responses):
        if not isinstance(entry, dict) or "response" not in entry:
            raise FakeLLMError(f"{path}: responses[{i}] must be a mapping with a 'response' key")
        response = entry["response"]
        if not isinstance(response, str | dict):
            raise FakeLLMError(f"{path}: responses[{i}].response must be a string or mapping")
        match = entry.get("match")
        if match is not None and not isinstance(match, str):
            raise FakeLLMError(f"{path}: responses[{i}].match must be a regex string")
        script.add(response, match=match, repeat=bool(entry.get("repeat", False)))
    return script


class FakeLLM(LLM):
    """Deterministic, scriptable LLM. No network, no keys.

    Selected through the same resolution mechanism as real models via the
    model strings ``"fake"`` (a process-wide shared script, registered with
    the classmethod :meth:`respond` and cleared with :meth:`reset`) and
    ``"fake:<path>"`` (a canned YAML file -- see the format in
    ``freeagent.llm._load_script``).

    For each call, the prompt text (all message contents concatenated) is
    tested against scripted entries in order; the first unconsumed matching
    entry wins (entries without ``match`` always match; ``repeat`` entries
    are never consumed); otherwise the ``default`` applies; otherwise
    :class:`FakeLLMError` is raised. Mapping responses are serialized to JSON
    for plain-text calls and validated against the schema for structured
    calls.
    """

    _shared_script: ClassVar[_Script] = _Script()

    def __init__(
        self,
        source: str | Path | None = None,
        *,
        shared: bool = False,
        telemetry: TelemetryCallback | None = None,
    ) -> None:
        """Create a fake LLM.

        ``source`` loads a canned YAML file; ``shared=True`` attaches to the
        process-wide script fed by :meth:`respond` (what the model string
        ``"fake"`` selects); with neither, the instance has its own empty
        script -- feed it with :meth:`queue`.
        """
        if source is not None and shared:
            raise ValueError("FakeLLM: 'source' and 'shared' are mutually exclusive")
        model = "fake" if source is None else f"fake:{source}"
        super().__init__(model, telemetry=telemetry)
        if shared:
            self._script = FakeLLM._shared_script
        elif source is not None:
            self._script = _load_script(Path(source))
        else:
            self._script = _Script()

    @classmethod
    def respond(
        cls,
        response: str | Mapping[str, Any],
        *,
        match: str | None = None,
        repeat: bool = False,
        default: bool = False,
    ) -> None:
        """Register a response on the shared script (the ``"fake"`` model string).

        ``match`` is a Python regex tested against the prompt text;
        ``repeat`` entries are never consumed; ``default=True`` registers the
        fallback used when nothing matches.
        """
        if default:
            cls._shared_script.default = response
        else:
            cls._shared_script.add(response, match=match, repeat=repeat)

    @classmethod
    def reset(cls) -> None:
        """Clear the shared script (call between tests)."""
        cls._shared_script.clear()

    def queue(
        self,
        response: str | Mapping[str, Any],
        *,
        match: str | None = None,
        repeat: bool = False,
        default: bool = False,
    ) -> FakeLLM:
        """Register a response on this instance's script; chainable."""
        if default:
            self._script.default = response
        else:
            self._script.add(response, match=match, repeat=repeat)
        return self

    async def _invoke(self, messages: Messages, schema: type[BaseModel] | None) -> str:  # noqa: ARG002
        prompt = "\n".join(str(m.get("content", "")) for m in messages)
        response = self._script.take(prompt)
        if isinstance(response, str):
            return response
        return json.dumps(dict(response))


def create_llm(
    model: str | None = None,
    *,
    configured: str | None = None,
    telemetry: TelemetryCallback | None = None,
) -> LLM:
    """Create the right client for a resolved model string.

    ``"fake"`` -> the shared-script :class:`FakeLLM`; ``"fake:<path>"`` -> a
    canned-file :class:`FakeLLM`; anything else -> the real litellm-backed
    :class:`LLM`.
    """
    resolved = resolve_model(model, configured)
    if resolved == "fake":
        return FakeLLM(shared=True, telemetry=telemetry)
    if resolved.startswith("fake:"):
        return FakeLLM(resolved[len("fake:") :], telemetry=telemetry)
    return LLM(resolved, telemetry=telemetry)
