"""The Twenty Questions environment: lifecycle only, no game state.

It reacts to exactly one message -- the Host's ``game_over`` signal on the
environment's inbox -- by initiating shutdown. The Host's public announcement
is the episode's outcome record; goodbyes happen inside the stopping grace
period, which is the framework's job, not this class's.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from freeagent import Environment

if TYPE_CHECKING:
    from freeagent import Envelope


class TwentyQuestionsEnvironment(Environment):
    """Base environment plus one rule: the Host's game-over signal ends the episode."""

    async def perceive(self, message: Envelope) -> None:
        payload = message.payload
        if isinstance(payload, dict) and payload.get("type") == "game_over":
            self.outcome = payload
            self.initiate_shutdown("game over")
