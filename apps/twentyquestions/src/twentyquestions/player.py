"""The Twenty Questions Player: an :class:`~freeagent.LLMAgent`, nothing more.

Everything a Player does -- deliberating openly on the public channel to
choose the next question, addressing the Host to spend a question or make a
guess, hearing the game-over announcement, saying goodbye, and leaving --
lives in the system prompt, not in code. Goodbyes fit inside the stopping
grace period; that timing is the framework's job.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from freeagent import Decision, LLMAgent

from .logging import log_utterance
from .prompts import PLAYER_SYSTEM_PROMPT

if TYPE_CHECKING:
    from collections.abc import Mapping


class Player(LLMAgent):
    """A Player is the base LLMAgent plus a game-specific default prompt."""

    def __init__(
        self,
        subject_root: str,
        agent_id: str,
        config: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(subject_root, agent_id, config)
        if not self.config.get("system_prompt"):
            self.system_prompt = PLAYER_SYSTEM_PROMPT.format(agent_id=agent_id)

    async def on_decision(self, decision: Decision) -> None:
        """Speak as usual, then log the utterance for debugging."""
        await super().on_decision(decision)
        if decision.speak and decision.message:
            log_utterance(self.id, decision.message)
