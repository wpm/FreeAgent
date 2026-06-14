"""The Twenty Questions Host: an :class:`~freeagent.LLMAgent` that knows the secret.

Stochastic vs. programmatic, per DESIGN.md: the LLM classifies each perceived
utterance (question / guess / deliberation / other) and judges guesses via the
:class:`HostDecision` schema; code counts questions, detects the win and the
loss, makes the game-over announcement on the public channel (the episode's
outcome record), and signals the environment via its inbox.
"""

from __future__ import annotations

import random
from typing import TYPE_CHECKING, Any, Literal

from freeagent import Decision, LLMAgent

from .logging import log_utterance
from .prompts import HOST_SYSTEM_PROMPT, LOSS_ANNOUNCEMENT, WELCOME, WIN_ANNOUNCEMENT

if TYPE_CHECKING:
    from collections.abc import Mapping

    from freeagent import Envelope
    from freeagent.llm import Messages

CANNED_SECRETS: tuple[str, ...] = (
    "an octopus",
    "a lighthouse",
    "a bicycle",
    "a teaspoon",
    "a volcano",
    "a chess piece",
    "a snowflake",
    "a hot air balloon",
)

_KICKOFF = "twentyquestions.kickoff"


class HostDecision(Decision):
    """The Host's structured verdict on each utterance it perceives."""

    classification: Literal["question", "guess", "deliberation", "other"] = "other"
    guess_correct: bool = False


class Host(LLMAgent):
    """The Host knows the secret; everything judgment-shaped is the LLM's call.

    Config keys on top of :class:`~freeagent.LLMAgent`'s: ``secret`` (default:
    random choice from :data:`CANNED_SECRETS`) and ``max_questions``
    (default 20).
    """

    decision_schema = HostDecision

    def __init__(
        self,
        subject_root: str,
        agent_id: str,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(subject_root, agent_id, config)
        if not self.config.get("system_prompt"):
            self.system_prompt = HOST_SYSTEM_PROMPT
        secret = self.config.get("secret")
        self.secret: str = str(secret) if secret else random.choice(CANNED_SECRETS)
        self.max_questions: int = int(self.config.get("max_questions", 20))
        self.questions_asked: int = 0
        self.game_over: bool = False

    async def control(self, message: Envelope) -> None:
        """Kick the episode off: on ``start``, schedule the welcome.

        Nobody has perceived anything yet, so without this the room would
        stay silent forever.
        """
        await super().control(message)
        payload = message.payload
        if isinstance(payload, dict) and payload.get("type") == "start":
            self.think(_KICKOFF)

    async def handle_think(self, payload: Any) -> None:
        if payload == _KICKOFF:
            self._say(WELCOME.format(max_questions=self.max_questions))
            return
        await super().handle_think(payload)

    async def on_decision(self, decision: Decision) -> None:
        """The programmatic half: count questions, detect win and loss."""
        await super().on_decision(decision)  # speak (or not), as the LLM decided
        if decision.speak and decision.message:
            log_utterance(self.id, decision.message)
        if self.game_over or not isinstance(decision, HostDecision):
            return
        # A question costs budget only when the Host actually answers it
        # (a silently classified question was dropped, not spent).
        if decision.classification == "question" and decision.speak:
            self.questions_asked += 1
        won = decision.classification == "guess" and decision.guess_correct
        if not won and self.questions_asked < self.max_questions:
            return
        self.game_over = True
        template = WIN_ANNOUNCEMENT if won else LOSS_ANNOUNCEMENT
        self._say(
            template.format(
                secret=self.secret,
                questions_asked=self.questions_asked,
                max_questions=self.max_questions,
            )
        )
        self.act(
            {
                "type": "game_over",
                "outcome": "win" if won else "loss",
                "secret": self.secret,
                "questions_asked": self.questions_asked,
            },
            recipients=["env"],
        )

    def decision_messages(self) -> Messages:
        """The standard prompt plus the Host's private game state."""
        messages = super().decision_messages()
        state = (
            f"[Host private game state] The secret is {self.secret}. "
            f"Questions used so far: {self.questions_asked} of {self.max_questions}."
        )
        messages[0] = {"role": "system", "content": f"{messages[0]['content']}\n\n{state}"}
        return messages

    def _say(self, message: str) -> None:
        """Broadcast *message*, keeping it in the transcript so the LLM sees its own words."""
        self.transcript.append((self.id, message))
        self.act(message)
        log_utterance(self.id, message)
