"""The Twenty Questions environment: lifecycle only, no game state.

It reacts to exactly one message -- the Host's ``game_over`` signal on the
environment's inbox -- by initiating shutdown. The Host's public announcement
is the episode's outcome record; goodbyes happen inside the stopping grace
period, which is the framework's job, not this class's.

As the atemporal episode (ADR-0003) makes an episode a *named* object, this env
also contributes the application-specific friendly name -- a finished game is
titled by its secret -- and reports the won/lost outcome, both of which land in
the episode's stream metadata when it is sealed.
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
            secret = payload.get("secret")
            if isinstance(secret, str) and secret:
                # App-contributed name: title the finished game by its secret.
                self.set_name(secret)
            self.initiate_shutdown("game over")

    def outcome_label(self) -> str | None:
        """Report the game result (``won``/``lost``), else the framework outcome."""
        if isinstance(self.outcome, dict):
            result = self.outcome.get("outcome")
            if result == "win":
                return "won"
            if result == "loss":
                return "lost"
        return super().outcome_label()
