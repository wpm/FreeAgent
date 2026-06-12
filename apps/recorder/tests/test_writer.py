"""Unit tests for the Parquet writing path: no NATS, no network."""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pyarrow.parquet as pq

from freeagent import Envelope
from freeagent_recorder import PARQUET_SCHEMA, MessageRecord, make_record, write_parquet

T0 = datetime(2026, 6, 11, 12, 0, 0, tzinfo=UTC)


def _record(seq: int, payload: str = "{}", sender: str = "alice") -> MessageRecord:
    return MessageRecord(
        episode_id="ep1",
        stream_seq=seq,
        subject="demo.episode.ep1.public",
        sender=sender,
        received_at=T0 + timedelta(seconds=seq),
        payload=payload,
    )


def test_write_parquet_schema_and_columns(tmp_path: Path) -> None:
    output = tmp_path / "episode.parquet"
    write_parquet([_record(1), _record(2)], output)

    table = pq.read_table(output)
    assert table.schema.equals(PARQUET_SCHEMA)
    assert table.column_names == [
        "episode_id",
        "stream_seq",
        "subject",
        "sender",
        "received_at",
        "payload",
    ]
    rows = table.to_pylist()
    assert [row["stream_seq"] for row in rows] == [1, 2]
    assert rows[0]["episode_id"] == "ep1"
    assert rows[0]["sender"] == "alice"
    assert rows[0]["subject"] == "demo.episode.ep1.public"
    assert rows[0]["received_at"] == T0 + timedelta(seconds=1)
    assert rows[0]["received_at"].tzinfo is not None


def test_write_parquet_sorts_by_stream_seq(tmp_path: Path) -> None:
    output = tmp_path / "episode.parquet"
    write_parquet([_record(3), _record(1), _record(2)], output)

    rows = pq.read_table(output).to_pylist()
    assert [row["stream_seq"] for row in rows] == [1, 2, 3]


def test_write_parquet_empty(tmp_path: Path) -> None:
    output = tmp_path / "episode.parquet"
    write_parquet([], output)

    table = pq.read_table(output)
    assert table.schema.equals(PARQUET_SCHEMA)
    assert table.num_rows == 0


def test_payload_round_trips_json_exactly(tmp_path: Path) -> None:
    payload = {
        "type": "say",
        "text": "héllo wörld — ¿qué? 日本語",
        "nested": {"n": [1, 2.5, None, True], "empty": {}},
    }
    envelope = Envelope(episode_id="ep1", sender="bob", payload=payload)
    record = make_record(
        stream_seq=7,
        subject="demo.episode.ep1.public",
        received_at=T0,
        data=envelope.to_bytes(),
        fallback_episode_id="ep1",
    )
    output = tmp_path / "episode.parquet"
    write_parquet([record], output)

    (row,) = pq.read_table(output).to_pylist()
    assert row["sender"] == "bob"
    assert json.loads(row["payload"]) == payload


def test_make_record_non_envelope_message() -> None:
    record = make_record(
        stream_seq=4,
        subject="demo.episode.ep1.log.raw",
        received_at=T0,
        data=b"not json at all \xff",
        fallback_episode_id="ep1",
    )
    assert record.sender == ""
    assert record.episode_id == "ep1"
    assert record.stream_seq == 4
    assert "not json at all" in record.payload


def test_make_record_json_but_not_envelope() -> None:
    record = make_record(
        stream_seq=5,
        subject="demo.episode.ep1.public",
        received_at=T0,
        data=b'{"hello": "world"}',
        fallback_episode_id="ep1",
    )
    assert record.sender == ""
    assert record.payload == '{"hello": "world"}'


def test_non_envelope_row_survives_parquet(tmp_path: Path) -> None:
    record = make_record(
        stream_seq=1,
        subject="demo.episode.ep1.public",
        received_at=T0,
        data=b"\x00\x01binary",
        fallback_episode_id="ep1",
    )
    output = tmp_path / "episode.parquet"
    write_parquet([record], output)

    (row,) = pq.read_table(output).to_pylist()
    assert row["sender"] == ""
    assert row["episode_id"] == "ep1"
