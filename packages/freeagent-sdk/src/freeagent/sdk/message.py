"""Serializable messages exchanged between entities over NATS.

Every value sent over NATS is a :class:`Message`, encoded as JSON.
:class:`~freeagent.sdk.entity.Entity` subclasses tell messages apart by their concrete pydantic
type when they arrive, via :meth:`Message.model_validate_json` and a ``match`` statement (see
:meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`) rather than by which subject they were
received on.

Messages come in two flavors, distinguished by how :class:`~freeagent.sdk.entity.Agent` handles
them:

- Plain :class:`Message` (and any subclass that isn't a :class:`Command`) is in-domain: it is
  pushed onto the agent's internal queue and drained one at a time by
  :meth:`~freeagent.sdk.entity.Agent.process_message`.
- :class:`Command` and its subclasses are out-of-domain: they are handled immediately, inline in
  :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`, ahead of anything already queued.
  :class:`StartEntity` and :class:`StopEntity` are the commands every entity understands; subclass
  :class:`Command` and override :meth:`~freeagent.sdk.entity.Agent.process_command` to add more.
"""

from __future__ import annotations

from pydantic import BaseModel


class Message(BaseModel):
    """A message passed between entities, or queued internally by an entity for its own
    consumption.

    Subclass this to define an application's in-domain message types. Instances are serialized to
    JSON with :meth:`to_bytes` and reconstructed on the receiving end with
    :meth:`Message.model_validate_json`, which uses pydantic's normal validation to pick the right
    subclass.
    """

    def to_bytes(self) -> bytes:
        """Serialize this message to the JSON bytes sent over NATS.

        :return: This message, encoded as UTF-8 JSON.
        """
        return self.model_dump_json().encode()


class Ack(Message):
    """Reply to a control command, carrying no content beyond acknowledging receipt.

    Sent in response to NATS requests such as :class:`StartEntity` and :class:`StopEntity`; see
    :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message` and
    :meth:`~freeagent.sdk.entity.Agent.respond`.
    """


class Command(Message):
    """Base class for out-of-domain messages, e.g. lifecycle management.

    Unlike a plain :class:`Message`, a :class:`Command` is handled synchronously as soon as it
    arrives rather than being queued; see
    :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message` and
    :meth:`~freeagent.sdk.entity.Agent.process_command`.
    """


class StartEntity(Command):
    """Command an :class:`~freeagent.sdk.entity.Agent` to start its run loop.

    Handled directly by :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`, which
    launches :meth:`~freeagent.sdk.entity.Agent.run` as a background task.
    """


class StopEntity(Command):
    """Command an :class:`~freeagent.sdk.entity.Agent` to stop its run loop and tear itself down.

    Handled directly by :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`, which cancels
    the running task (if any) and calls :meth:`~freeagent.sdk.entity.Entity.stop`.
    """
