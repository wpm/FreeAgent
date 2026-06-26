"""LLMAgent: an agent defined primarily by text prompts.

State is the transcript of perceived messages (sender + payload text). On
each perceived in-world message the agent appends to the transcript and -- if
no LLM call is in flight -- spawns a "decide" call (system prompt +
transcript) with the pinned structured-output schema :class:`Decision`
(``{speak: bool, message: str}``). If messages arrive during the call, it
reconsiders once the completion's think event lands (the drain-the-inbox
pattern). If the decision is to speak, the message is broadcast via ``act``.

The LLM call itself follows the hard rule: it never happens inside a
handler. ``perceive`` only updates state and spawns; the spawned task's
completion re-enters the fold as a think message, where the decision is
applied. Per-call telemetry (prompt, completion, timing) goes to the agent's
log-only subject, on by default (config key ``llm_telemetry``).

This class is the road to the "applications from text prompts alone" goal:
subclasses customize the decision prompt (:meth:`decision_messages`), the
schema (``decision_schema``), and structured side-decisions
(:meth:`on_decision`).
"""

from __future__ import annotations

import json
import logging
from typing import TYPE_CHECKING, Any, ClassVar

from pydantic import BaseModel

from .agent import Agent
from .llm import LLM, create_llm

if TYPE_CHECKING:
    from collections.abc import Mapping

    from .envelope import Envelope
    from .llm import Messages

logger = logging.getLogger("freeagent.llm_agent")

_DECISION_THINK = "freeagent.llm_agent.decision"
_ERROR_THINK = "freeagent.llm_agent.error"
_NUDGE_THINK = "freeagent.llm_agent.nudge"

_NUDGE_HINT = (
    "The room has been quiet for a while. If the conversation is waiting on "
    "someone -- perhaps you -- take the initiative now instead of waiting; if "
    "there is genuinely nothing to add, stay silent."
)

_DEFAULT_SYSTEM_PROMPT = (
    "You are {agent_id}, one agent in a real-time multi-agent conversation with no "
    "turn-taking. You may speak or stay silent at any moment. Stay silent unless you "
    "have something worth saying right now."
)


class Decision(BaseModel):
    """The pinned structured-output schema for LLMAgent decisions."""

    speak: bool
    message: str = ""


class LLMAgent(Agent):
    """An agent whose behavior is delegated to prompts.

    Config keys (from the runner yml per-agent config): ``model`` (litellm
    model string, resolved via :func:`freeagent.llm.resolve_model`, so
    ``"fake"`` / ``"fake:<path>"`` select the fake LLM), ``system_prompt``,
    ``llm_telemetry`` (bool, default True: publish each call's record to the
    agent's log-only subject), ``nudge_interval`` (seconds, default off: see
    below), plus the base :class:`Agent` keys.

    **The nudge.** A conversation of LLMAgents can stall: everyone decides to
    stay silent and wait, and since decisions are only triggered by perceived
    messages, no one ever speaks again. With ``nudge_interval`` set, the agent
    schedules a periodic think message that re-runs the decide call during a
    lull -- the prompt gains a hint that taking the initiative is allowed.
    This is the design's "periodic think messages invoke an 'act or stay
    silent?'" gatekeeper pattern; each agent still decides for itself --
    there is no turn-taking. Silence checks cost LLM calls, so the interval
    bounds the spend.
    """

    decision_schema: ClassVar[type[Decision]] = Decision
    """Subclasses may pin a richer :class:`Decision` subclass here."""

    def __init__(
        self,
        subject_root: str,
        agent_id: str,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(subject_root, agent_id, config)
        telemetry_on = bool(self.config.get("llm_telemetry", True))
        self.llm: LLM = create_llm(
            configured=self.config.get("model"),
            api_key=self.config.get("api_key"),
            telemetry=self.log_event if telemetry_on else None,
        )
        prompt = self.config.get("system_prompt")
        self.system_prompt: str = (
            str(prompt) if prompt else _DEFAULT_SYSTEM_PROMPT.format(agent_id=agent_id)
        )
        nudge = self.config.get("nudge_interval")
        self.nudge_interval: float | None = float(nudge) if nudge is not None else None
        self.transcript: list[tuple[str, str]] = []
        self._deciding = False
        self._reconsider = False
        self._nudged = False
        self._nudge_scheduled = False

    # ------------------------------------------------------------------
    # The fold
    # ------------------------------------------------------------------

    async def control(self, message: Envelope) -> None:
        """Handle lifecycle messages; arms the periodic nudge on ``start``."""
        await super().control(message)
        if self.phase == "active" and self.nudge_interval is not None and not self._nudge_scheduled:
            self._nudge_scheduled = True
            self.schedule_periodic_think(self.nudge_interval, {"type": _NUDGE_THINK})

    async def perceive(self, message: Envelope) -> None:
        """Append to the transcript and (maybe) spawn a decision.

        Fast and non-blocking: the LLM call happens in a spawned task, never
        here. If a call is already in flight, just mark for reconsideration
        -- the inbox is drained into one call (the drain-the-inbox pattern).
        """
        self.transcript.append((message.sender, self.payload_text(message.payload)))
        if self._deciding:
            self._reconsider = True
        else:
            self._begin_decision()

    async def handle_think(self, payload: Any) -> None:
        """Apply a completed decision (or note a failed call), then reconsider if stale."""
        if isinstance(payload, dict) and payload.get("type") == _DECISION_THINK:
            self._deciding = False
            await self.on_decision(payload["decision"])
            if self._reconsider:
                self._begin_decision()
            return
        if isinstance(payload, dict) and payload.get("type") == _ERROR_THINK:
            self._deciding = False
            logger.error("agent %s: decision call failed: %s", self.id, payload.get("error"))
            if self._reconsider:
                self._begin_decision()
            return
        if isinstance(payload, dict) and payload.get("type") == _NUDGE_THINK:
            if self.phase == "active" and not self._deciding and self.transcript:
                self._begin_decision(nudged=True)
            return
        await super().handle_think(payload)

    async def on_decision(self, decision: Decision) -> None:
        """Apply one decision: broadcast the message if ``speak``.

        Subclasses override this to handle structured side-decisions from a
        richer ``decision_schema`` (call ``await super().on_decision(...)``
        to keep the speak behavior). Runs inside the fold -- fast and
        non-blocking only.
        """
        if decision.speak and decision.message:
            self.transcript.append((self.id, decision.message))
            self.act(decision.message)

    # ------------------------------------------------------------------
    # Prompt construction -- subclass hooks
    # ------------------------------------------------------------------

    def payload_text(self, payload: Any) -> str:
        """Render a perceived payload as transcript text."""
        if isinstance(payload, str):
            return payload
        if isinstance(payload, dict):
            for key in ("message", "text"):
                value = payload.get(key)
                if isinstance(value, str):
                    return value
        return json.dumps(payload)

    def decision_messages(self) -> Messages:
        """Build the chat messages for one decision call (system prompt + transcript)."""
        transcript = "\n".join(f"{sender}: {text}" for sender, text in self.transcript)
        nudge = f"{_NUDGE_HINT}\n\n" if self._nudged else ""
        instructions = (
            "Here is the conversation so far:\n\n"
            f"{transcript or '(nothing yet)'}\n\n"
            f"{nudge}"
            "Decide whether to speak right now, and what to say if so. "
            "Respond with JSON: speak (boolean), message (string, empty when silent)."
        )
        return [
            {"role": "system", "content": self.system_prompt},
            {"role": "user", "content": instructions},
        ]

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _begin_decision(self, *, nudged: bool = False) -> None:
        self._deciding = True
        self._reconsider = False
        self._nudged = nudged
        # Snapshot the prompt inside the handler so the spawned call is a
        # pure function of the state at this point in the fold.
        messages = self.decision_messages()
        self._nudged = False
        self.spawn(self._decide(messages))

    async def _decide(self, messages: Messages) -> None:
        """The spawned LLM call; its completion re-enters the fold as a think message."""
        try:
            decision = await self.llm.complete(messages, schema=type(self).decision_schema)
        except Exception as exc:  # surfaced into the fold as an error think event
            self.think({"type": _ERROR_THINK, "error": f"{type(exc).__name__}: {exc}"})
            return
        self.think({"type": _DECISION_THINK, "decision": decision})
