"""Typed wire payloads for Twenty Questions.

The framework envelope (:class:`freeagent.Envelope`) is payload-opaque. This
module gives the *one* structured payload the app puts on the wire a real
Pydantic model, so it is single-sourced into the viewer's TypeScript types
(ADR-0001) instead of an ad-hoc dict. Plain Player and Host speech rides as a
bare string payload and needs no model.
"""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict


class GameOver(BaseModel):
    """The Host's end-of-episode signal to the environment's inbox.

    Sent to ``env`` when the game is won, lost, or the question budget is
    exhausted; the environment reads it and initiates shutdown. ``type`` is a
    constant discriminator so a viewer can tell this payload apart from speech.
    """

    model_config = ConfigDict(extra="forbid")

    type: Literal["game_over"] = "game_over"
    outcome: Literal["win", "loss"]
    secret: str
    questions_asked: int
