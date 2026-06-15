"""Twenty Questions: the FreeAgent sample application.

Several Players and one Host, all :class:`~freeagent.LLMAgent` subclasses
defined almost entirely by their prompts, plus a minimal environment that
holds no game state. See DESIGN.md's "Sample application: Twenty Questions".
"""

from __future__ import annotations

from .environment import TwentyQuestionsEnvironment
from .host import CANNED_SECRETS, Host, HostDecision
from .player import Player

__all__ = [
    "CANNED_SECRETS",
    "Host",
    "HostDecision",
    "Player",
    "TwentyQuestionsEnvironment",
]
