"""Tests for :mod:`freeagent.sdk.message`.

These exercise encoding to bytes and decoding back, for the base :class:`Message` and its built-in
subclasses, plus an application-defined subclass with its own fields. Every :class:`Message` carries
a ``message_type`` tag naming its concrete class, so decoding via the base class recovers the
original subclass rather than a plain :class:`Message`.
"""

from __future__ import annotations

import importlib

import pytest
from fixtures import Product
from freeagent.sdk.message import (
    Ack,
    Command,
    EpisodeComplete,
    Message,
    StartEntity,
    StopAgent,
    StopEntity,
)


def test_to_bytes_encodes_as_utf8_json() -> None:
    message = Product(x=2.0, y=3.0)
    assert message.to_bytes() == b'{"message_type":"Product","x":2.0,"y":3.0}'
    message = message()
    assert message.to_bytes() == b'{"message_type":"Product","x":2.0,"y":3.0,"x_times_y":6.0}'


def test_to_bytes_returns_bytes_not_str() -> None:
    assert isinstance(Ack().to_bytes(), bytes)


@pytest.mark.parametrize(
    "cls", [Message, Ack, Command, StartEntity, StopEntity, StopAgent, EpisodeComplete]
)
def test_type_defaults_to_the_concrete_class_name(cls: type[Message]) -> None:
    assert cls().message_type == cls.__name__


@pytest.mark.parametrize(
    "cls", [Message, Ack, Command, StartEntity, StopEntity, StopAgent, EpisodeComplete]
)
def test_fieldless_message_round_trips_through_its_own_class(cls: type[Message]) -> None:
    message = cls()

    decoded = cls.model_validate_json(message.to_bytes())

    assert decoded == message


def test_message_with_fields_round_trips_through_its_own_class() -> None:
    message = Product(x=2.0, y=3.0)

    decoded = Product.model_validate_json(message.to_bytes())

    assert decoded == message
    assert decoded.x == 2.0
    assert decoded.y == 3.0


def test_decoding_via_the_base_class_recovers_the_concrete_subclass() -> None:
    message = Product(x=2.0, y=3.0)

    decoded = Message.model_validate_json(message.to_bytes())

    assert type(decoded) is Product
    assert decoded == message


@pytest.mark.parametrize("cls", [StopAgent, EpisodeComplete])
def test_control_plane_types_decode_via_the_base_class(cls: type[Message]) -> None:
    # Both new control-plane types must be recoverable through the polymorphic registry, so an
    # observer decoding via the base Message class gets back the concrete subclass.
    decoded = Message.model_validate_json(cls().to_bytes())

    assert type(decoded) is cls


def test_stop_agent_is_a_command() -> None:
    # StopAgent is handled inline like other commands, not queued as an in-domain message.
    assert issubclass(StopAgent, Command)


def test_episode_complete_is_a_plain_message_not_a_command() -> None:
    # EpisodeComplete is observed off the wire, not directed at an agent's run loop, so it must not
    # be a Command (which the Agent would otherwise hand to process_command).
    assert issubclass(EpisodeComplete, Message)
    assert not issubclass(EpisodeComplete, Command)


def test_model_validate_json_rejects_malformed_json() -> None:
    with pytest.raises(ValueError):
        Message.model_validate_json(b"not json")


def test_model_validate_json_rejects_json_missing_required_fields() -> None:
    with pytest.raises(ValueError):
        Product.model_validate_json(b'{"x": 1.0}')


def test_model_validate_json_rejects_an_unknown_type_tag() -> None:
    with pytest.raises(ValueError, match="Bogus"):
        Message.model_validate_json(b'{"message_type": "Bogus"}')


def test_duplicate_subclass_name_across_modules_raises_at_class_definition_time(
    isolated_message_registry: None,
) -> None:
    # collision_a and collision_b each define a Message subclass named "Collider" from a different
    # module; importing the second triggers the cross-module duplicate-name guard.
    importlib.import_module("collision_a")

    with pytest.raises(TypeError, match="Collider"):
        importlib.import_module("collision_b")


def test_reimporting_the_same_module_does_not_raise(
    isolated_message_registry: None,
) -> None:
    # A reload re-defines a subclass with the same name from the same module: benign, not a
    # cross-application collision, so it must re-register rather than raise.
    module = importlib.import_module("collision_a")

    importlib.reload(module)  # must not raise


def test_try_decode_returns_none_for_an_unknown_type_tag() -> None:
    assert Message.try_decode(b'{"message_type": "Bogus"}') is None


def test_subclass_try_decode_returns_none_for_an_unknown_type_tag() -> None:
    # try_decode called on a subclass must still decode the tag against the whole registry, not
    # validate strictly as that subclass, so an unknown tag yields None rather than raising.
    assert Product.try_decode(b'{"message_type": "Bogus"}') is None


def test_try_decode_returns_the_concrete_subclass_for_a_known_type_tag() -> None:
    message = Product(x=2.0, y=3.0)

    decoded = Message.try_decode(message.to_bytes())

    assert type(decoded) is Product
    assert decoded == message


def test_try_decode_still_raises_on_malformed_json() -> None:
    with pytest.raises(ValueError):
        Message.try_decode(b"not json")


def test_try_decode_still_raises_when_a_known_type_fails_validation() -> None:
    # A known type tag whose payload is missing required fields is a validation failure, not an
    # unknown type: try_decode must let it raise rather than swallowing it into None.
    with pytest.raises(ValueError):
        Message.try_decode(b'{"message_type": "Product", "x": 1.0}')
