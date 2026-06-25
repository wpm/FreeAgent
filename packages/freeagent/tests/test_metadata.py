"""Encoding/decoding the episode's durable record on stream metadata (ADR-0003/0005).

The manifest set and resolved engine versions ride on the same flat
``dict[str, str]`` JetStream stores; this exercises their JSON round-trip and the
tolerance that keeps enumeration from crashing on a stranger stream.
"""

from __future__ import annotations

import json

from freeagent.metadata import (
    KEY_MANIFEST_SET,
    KEY_RESOLVED_VERSIONS,
    EpisodeMetadata,
)


def test_minimal_metadata_omits_manifest_keys() -> None:
    encoded = EpisodeMetadata(app="demo", name="brave-otter", status="setup").to_stream_metadata()
    assert KEY_MANIFEST_SET not in encoded
    assert KEY_RESOLVED_VERSIONS not in encoded


def test_manifest_set_and_versions_roundtrip() -> None:
    manifest_set = [
        {"role": "agent", "class": "pkg.mod:Alice", "agent_id": "alice"},
        {"role": "environment", "class": "pkg.mod:Env", "roster": ["alice"]},
    ]
    versions = {"pkg.mod:Alice": "pkg==1.2.3", "pkg.mod:Env": "pkg==1.2.3"}
    meta = EpisodeMetadata(
        app="demo",
        name="brave-otter",
        status="running",
        manifest_set=manifest_set,
        resolved_versions=versions,
    )
    encoded = meta.to_stream_metadata()
    # Values are strings on the wire (JetStream stores str -> str).
    assert json.loads(encoded[KEY_MANIFEST_SET]) == manifest_set
    assert json.loads(encoded[KEY_RESOLVED_VERSIONS]) == versions

    decoded = EpisodeMetadata.from_stream_metadata(encoded)
    assert decoded is not None
    assert decoded.manifest_set == manifest_set
    assert decoded.resolved_versions == versions


def test_decode_tolerates_missing_keys() -> None:
    decoded = EpisodeMetadata.from_stream_metadata(
        {"freeagent_app": "demo", "freeagent_name": "x", "freeagent_status": "ended"}
    )
    assert decoded is not None
    assert decoded.manifest_set == []
    assert decoded.resolved_versions == {}


def test_decode_tolerates_malformed_json() -> None:
    decoded = EpisodeMetadata.from_stream_metadata(
        {
            "freeagent_app": "demo",
            "freeagent_name": "x",
            "freeagent_status": "ended",
            KEY_MANIFEST_SET: "{not json",
            KEY_RESOLVED_VERSIONS: "also not json",
        }
    )
    assert decoded is not None
    assert decoded.manifest_set == []
    assert decoded.resolved_versions == {}


def test_decode_drops_non_dict_manifest_items_and_non_str_versions() -> None:
    decoded = EpisodeMetadata.from_stream_metadata(
        {
            "freeagent_app": "demo",
            "freeagent_name": "x",
            "freeagent_status": "ended",
            KEY_MANIFEST_SET: json.dumps([{"role": "agent"}, "stray", 7]),
            KEY_RESOLVED_VERSIONS: json.dumps({"pkg:A": "pkg==1", "pkg:B": 9}),
        }
    )
    assert decoded is not None
    assert decoded.manifest_set == [{"role": "agent"}]
    assert decoded.resolved_versions == {"pkg:A": "pkg==1"}


def test_decode_tolerates_wrong_json_shapes() -> None:
    decoded = EpisodeMetadata.from_stream_metadata(
        {
            "freeagent_app": "demo",
            "freeagent_name": "x",
            "freeagent_status": "ended",
            KEY_MANIFEST_SET: json.dumps({"not": "a list"}),
            KEY_RESOLVED_VERSIONS: json.dumps(["not", "a", "dict"]),
        }
    )
    assert decoded is not None
    assert decoded.manifest_set == []
    assert decoded.resolved_versions == {}
