"""A module defining a ``Collider`` Message, to pair with :mod:`collision_a`.

See :mod:`collision_a` for why these two modules exist.
"""

from __future__ import annotations

from freeagent.sdk.message import Message


class Collider(Message):
    """A Message whose bare name collides with :class:`collision_a.Collider`."""
