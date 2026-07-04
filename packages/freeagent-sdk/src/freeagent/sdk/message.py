"""Serializable messages exchanged between entities over NATS.

Every value sent over NATS is a :class:`Message`, encoded as JSON.
:class:`~freeagent.sdk.entity.Entity` subclasses tell messages apart by their concrete pydantic
type when they arrive, via :meth:`Message.model_validate_json` and a ``match`` statement (see
:meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`) rather than by which subject they were
received on. Every :class:`Message` carries its concrete class name as a ``message_type`` tag so
that decoding via the base :class:`Message` class (rather than the original concrete subclass) still
recovers the right subclass; see :meth:`Message.model_validate_json`.

Messages come in two flavors, distinguished by how :class:`~freeagent.sdk.entity.Agent` handles
them:

- Plain :class:`Message` (and any subclass that isn't a :class:`Command`) is in-domain: it is
  pushed onto the agent's internal queue and drained one at a time by
  :meth:`~freeagent.sdk.entity.Agent.process_message`.
- :class:`Command` and its subclasses are out-of-domain: they are handled immediately, inline in
  :meth:`~freeagent.sdk.entity.Agent.handle_incoming_message`, ahead of anything already queued.
  :class:`StartEntity`, :class:`StopEntity`, and :class:`StopAgent` are the commands every entity
  understands; subclass :class:`Command` and override
  :meth:`~freeagent.sdk.entity.Agent.process_command` to add more.

Alongside those commands the SDK defines :class:`EpisodeComplete`, a plain :class:`Message` (not a
:class:`Command`) the :class:`~freeagent.sdk.entity.Environment` publishes as the last message of an
episode to mark its definite end for observers.
"""

from __future__ import annotations

from typing import Any, ClassVar, Literal

from pydantic import BaseModel, ConfigDict


class UnknownMessageType(ValueError):
    """Raised when a :class:`Message`'s ``message_type`` tag names no known subclass.

    A subclass of :class:`ValueError` so existing ``except ValueError`` handlers around
    :meth:`Message.model_validate_json` keep working, but distinct enough for
    :meth:`Message.try_decode` to catch *only* the unknown-type case and turn it into ``None``,
    without also swallowing genuine validation failures.
    """


class Message(BaseModel):
    """A message passed between entities, or queued internally by an entity for its own consumption.

    Subclass this to define an application's in-domain message types. Instances are serialized to
    JSON with :meth:`to_bytes` and reconstructed on the receiving end with
    :meth:`Message.model_validate_json`, which uses pydantic's normal validation to pick the right
    subclass.

    :ivar message_type: The concrete class's name, set automatically and used to pick the right
        subclass on decode. Subclasses should not set or override this field themselves.
    """

    model_config = ConfigDict(polymorphic_serialization=True)

    _by_type: ClassVar[dict[str, type[Message]]] = {}

    message_type: str = ""
    protocol: str | None = None

    def __init_subclass__(cls, **kwargs: Any) -> None:
        """Register the subclass under its bare class name, rejecting cross-module collisions.

        Every :class:`Message` subclass is indexed in :attr:`_by_type` by its bare class name, which
        is also its ``message_type`` tag on the wire. Two subclasses from *different* modules
        sharing a name (e.g. two applications both defining ``Chain``) would silently mis-decode
        each other's messages, so that is rejected at class-definition time. A name already
        registered from the *same* module is treated as a benign re-definition (e.g.
        ``importlib.reload``) and simply re-registers.

        :raises TypeError: If a different module already registered a subclass with this name.
        """
        super().__init_subclass__(**kwargs)
        cls.model_fields["message_type"].default = cls.__name__
        existing = Message._by_type.get(cls.__name__)
        if existing is not None and existing.__module__ != cls.__module__:
            raise TypeError(
                f'Duplicate Message subclass name "{cls.__name__}": already registered by '
                f"{existing.__module__}.{existing.__qualname__}. Message type tags are bare class "
                f"names and must be unique across all loaded applications."
            )
        Message._by_type[cls.__name__] = cls

    def __init__(self, **data: Any) -> None:
        data.setdefault("message_type", self.__class__.__name__)
        super().__init__(**data)

    @classmethod
    def model_validate_json(
        cls,
        json_data: str | bytes | bytearray,
        *,
        strict: bool | None = None,
        extra: Literal["allow", "ignore", "forbid"] | None = None,
        context: Any | None = None,
        by_alias: bool | None = None,
        by_name: bool | None = None,
    ) -> Message:
        """Decode JSON into an instance of whichever :class:`Message` subclass it was encoded from.

        The JSON's ``message_type`` tag (see :attr:`message_type`) is looked up among all known
        :class:`Message` subclasses, so calling this on the base :class:`Message` class returns the
        right concrete subclass rather than a plain :class:`Message`.

        An unknown ``message_type`` tag is an error here; callers that expect unknown types (e.g.
        the control-plane API relaying application messages opaquely) should use :meth:`try_decode`
        instead.

        :param json_data: The JSON, as produced by :meth:`to_bytes`.
        :param strict: As for :meth:`pydantic.BaseModel.model_validate_json`.
        :param extra: As for :meth:`pydantic.BaseModel.model_validate_json`.
        :param context: As for :meth:`pydantic.BaseModel.model_validate_json`.
        :param by_alias: As for :meth:`pydantic.BaseModel.model_validate_json`.
        :param by_name: As for :meth:`pydantic.BaseModel.model_validate_json`.
        :return: The decoded message, as an instance of its original concrete subclass.
        :raises UnknownMessageType: If the JSON's ``message_type`` tag doesn't name a known
            :class:`Message` subclass.
        """
        # The declared type restates model_validate_json's Self return as Message: mypy narrows it
        # already, but some IDE checkers bind Self to BaseModel across the super() proxy and would
        # otherwise flag tagged.message_type below as an unresolved attribute.
        tagged: Message = super().model_validate_json(
            json_data,
            strict=strict,
            extra=extra,
            context=context,
            by_alias=by_alias,
            by_name=by_name,
        )
        subclass = Message._by_type.get(tagged.message_type)
        if subclass is None:
            raise UnknownMessageType(f'Unknown message type "{tagged.message_type}"')
        if subclass is cls:
            return tagged
        return subclass.model_validate_json(
            json_data,
            strict=strict,
            extra=extra,
            context=context,
            by_alias=by_alias,
            by_name=by_name,
        )

    @classmethod
    def try_decode(cls, json_data: str | bytes | bytearray) -> Message | None:
        """Decode JSON like :meth:`model_validate_json`, but return ``None`` for an unknown type.

        Where :meth:`model_validate_json` raises on a ``message_type`` tag that names no known
        :class:`Message` subclass, this returns ``None`` instead — for callers that meet unknown
        types *by design*, such as the control-plane API decoding only SDK messages and relaying
        application-defined ones opaquely. Malformed JSON and validation failures still raise; only
        the "not a known type" case is turned into ``None``.

        :param json_data: The JSON, as produced by :meth:`to_bytes`.
        :return: The decoded message as its concrete subclass, or ``None`` if its ``message_type``
            tag names no known :class:`Message` subclass.
        """
        # Always dispatch through the base Message so the ``message_type`` tag is read leniently and
        # matched against the whole registry. Going through ``cls`` would validate strictly as
        # ``cls`` while reading the tag, so ``Product.try_decode`` of an unknown/other type would
        # raise a validation error instead of returning None.
        try:
            return Message.model_validate_json(json_data)
        except UnknownMessageType:
            return None

    def to_bytes(self) -> bytes:
        """Serialize this message to the JSON bytes sent over NATS.

        :return: This message, encoded as UTF-8 JSON.
        """
        return self.model_dump_json(exclude_none=True).encode()


Message._by_type[Message.__name__] = Message


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

    Broadcast to every agent by :meth:`~freeagent.sdk.entity.Environment.stop` as the episode-wide
    teardown; to stop a single agent while the rest of the episode continues, use :class:`StopAgent`
    instead.
    """


class StopAgent(Command):
    """Command a single :class:`~freeagent.sdk.entity.Agent` to stop, while the episode continues.

    Sent by the :class:`~freeagent.sdk.entity.Environment` to *one* agent (via
    :meth:`~freeagent.sdk.entity.Environment.stop_agent`), unlike the broadcast :class:`StopEntity`
    teardown that ends the whole episode. On receipt the agent cancels its run loop and tears itself
    down exactly as for :class:`StopEntity` — the difference is targeting, not teardown semantics —
    and, like :class:`StopEntity`, doing so is idempotent and replied to with an :class:`Ack`.
    """


class EpisodeComplete(Message):
    """The last message of an episode, marking its definite end for any observer.

    Published (not broadcast as a request) by the :class:`~freeagent.sdk.entity.Environment` on its
    own ``{episode_root}.environment`` subject as the final message of an episode, immediately
    before the environment broadcasts :class:`StopEntity` and tears itself down; see
    :meth:`~freeagent.sdk.entity.Environment.stop`.

    A plain :class:`Message` rather than a :class:`Command`: it is not directed at an agent's run
    loop but observed off the wire by the control-plane API (and, later, archives), for which an
    episode record without a definite end marker cannot be trusted as training data (ADR-0008).
    """
