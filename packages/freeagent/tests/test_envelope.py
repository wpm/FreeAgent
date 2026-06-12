"""The message envelope: exactly four fields, no timestamps, JSON on the wire."""

from __future__ import annotations

import json

import pytest
from pydantic import ValidationError

from freeagent import Envelope


def test_envelope_has_exactly_the_four_specified_fields() -> None:
    assert set(Envelope.model_fields) == {"message_id", "episode_id", "sender", "payload"}


def test_no_timestamps_anywhere() -> None:
    # All timing comes from JetStream server metadata; the envelope must not
    # carry any clock of its own.
    assert not any("time" in name or "stamp" in name for name in Envelope.model_fields)


def test_message_id_is_unique_per_message() -> None:
    a = Envelope(episode_id="e", sender="s", payload=None)
    b = Envelope(episode_id="e", sender="s", payload=None)
    assert a.message_id != b.message_id


def test_wire_format_is_json_bytes_and_round_trips() -> None:
    envelope = Envelope(episode_id="ep1", sender="alice", payload={"n": [1, 2, 3]})
    data = envelope.to_bytes()
    parsed = json.loads(data.decode("utf-8"))
    assert parsed["sender"] == "alice"
    assert Envelope.from_bytes(data) == envelope


def test_extra_fields_are_rejected() -> None:
    with pytest.raises(ValidationError):
        Envelope(episode_id="e", sender="s", payload=None, timestamp=123)  # type: ignore[call-arg]


def test_payload_is_app_defined_and_optional() -> None:
    assert Envelope(episode_id="e", sender="s").payload is None
    assert Envelope(episode_id="e", sender="s", payload="text").payload == "text"
