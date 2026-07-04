"""A module defining a ``Collider`` Message, to pair with :mod:`collision_b`.

The two modules define a :class:`~freeagent.sdk.message.Message` subclass with the *same* bare
class name from *different* modules, so importing both exercises the cross-module duplicate-name
guard in :meth:`Message.__init_subclass__`.
"""

from __future__ import annotations

from freeagent.sdk.message import Message


class Collider(Message):
    """A Message whose bare name collides with :class:`collision_b.Collider`."""
