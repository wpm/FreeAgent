"""The minimal message envelope.

There is no fixed message format beyond this envelope; specific environments
define richer formats inside ``payload``. The envelope deliberately carries
**no timestamp**: all timing comes from JetStream's server-assigned metadata
-- one clock for the whole episode, immune to skew across agent machines.
"""

from __future__ import annotations

import uuid
from typing import Any

from pydantic import BaseModel, ConfigDict, Field


def _new_message_id() -> str:
    return uuid.uuid4().hex


class Envelope(BaseModel):
    """Wire envelope for every FreeAgent message. JSON bytes on the wire.

    Exactly four fields, no timestamps:

    - ``message_id``: unique per message (uuid4 hex).
    - ``episode_id``: the episode this message belongs to.
    - ``sender``: agent name, or ``"env"`` for the environment.
    - ``payload``: app-defined and opaque to the framework (JSON-serializable).
    """

    model_config = ConfigDict(extra="forbid")

    message_id: str = Field(default_factory=_new_message_id)
    episode_id: str
    sender: str
    payload: Any = None

    def to_bytes(self) -> bytes:
        """Serialize to the wire format (JSON bytes)."""
        return self.model_dump_json().encode("utf-8")

    @classmethod
    def from_bytes(cls, data: bytes) -> Envelope:
        """Parse the wire format (JSON bytes)."""
        return cls.model_validate_json(data)
