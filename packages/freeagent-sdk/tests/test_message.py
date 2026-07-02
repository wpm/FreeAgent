"""Tests for :mod:`freeagent.sdk.message`.

These exercise encoding to bytes and decoding back, for the base :class:`Message` and its built-in
subclasses, plus an application-defined subclass with its own fields. Every :class:`Message`
carries a ``type`` tag naming its concrete class, so decoding via the base class recovers the
original subclass rather than a plain :class:`Message`.
"""

from __future__ import annotations

import pytest
from fixtures import Product
from freeagent.sdk.message import Ack, Command, Message, StartEntity, StopEntity


def test_to_bytes_encodes_as_utf8_json() -> None:
    message = Product(x=2.0, y=3.0)

    assert message.to_bytes() == b'{"type":"Product","x":2.0,"y":3.0,"x_times_y":null}'


def test_to_bytes_returns_bytes_not_str() -> None:
    assert isinstance(Ack().to_bytes(), bytes)


@pytest.mark.parametrize("cls", [Message, Ack, Command, StartEntity, StopEntity])
def test_type_defaults_to_the_concrete_class_name(cls: type[Message]) -> None:
    assert cls().type == cls.__name__


@pytest.mark.parametrize("cls", [Message, Ack, Command, StartEntity, StopEntity])
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


def test_model_validate_json_rejects_malformed_json() -> None:
    with pytest.raises(ValueError):
        Message.model_validate_json(b"not json")


def test_model_validate_json_rejects_json_missing_required_fields() -> None:
    with pytest.raises(ValueError):
        Product.model_validate_json(b'{"x": 1.0}')


def test_model_validate_json_rejects_an_unknown_type_tag() -> None:
    with pytest.raises(ValueError, match="Bogus"):
        Message.model_validate_json(b'{"type": "Bogus"}')
