"""Episode identity stored on the JetStream stream (ADR-0003).

An episode is a named, JetStream-resident object: everything needed to list and
label it lives *with* it, as stream metadata, so the durable record is the
single source of truth and a service restart loses nothing. The environment
writes this metadata when it creates the stream and updates it as the lifecycle
advances; the service reads it to build :class:`~freeagent.service.EpisodeView`
without remembering anything itself.

JetStream stream metadata is a flat ``dict[str, str]``. This module is the one
place those string keys are defined and the one place an
:class:`EpisodeMetadata` is encoded to and decoded from them, so the engine and
the service never disagree on the shape.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any

#: Namespace prefix on every key, so episode metadata never collides with
#: server- or operator-set stream metadata.
_PREFIX = "freeagent_"

#: Manifest ``config`` keys that carry secrets and must never be persisted into
#: the durable record (#68). They are delivered transiently to the live worker
#: child off the work queue, but are stripped before the manifest set is written
#: to stream metadata -- which is durable and echoed verbatim by the episode
#: list/get/feed responses.
_REDACTED_CONFIG_KEYS = frozenset({"api_key"})

KEY_APP = _PREFIX + "app"
KEY_NAME = _PREFIX + "name"
KEY_STATUS = _PREFIX + "status"
KEY_OUTCOME = _PREFIX + "outcome"
KEY_MODE = _PREFIX + "mode"
KEY_CREATED_AT = _PREFIX + "created_at"
#: The recruited manifest set (ADR-0005): the JSON-encoded list of manifests
#: the recruiter scheduled for this episode -- *what should be running*. Never
#: a PID: liveness is the gap between this set and presence on the bus.
KEY_MANIFEST_SET = _PREFIX + "manifest_set"
#: The resolved engine versions (ADR-0005): a JSON object mapping each
#: ``module:QualName`` class ref to its resolved ``distribution==version``
#: stamp -- *what ran*. Write-only provenance, the difference between an RL
#: trajectory you can reproduce and one you cannot.
KEY_RESOLVED_VERSIONS = _PREFIX + "resolved_versions"


@dataclass(slots=True)
class EpisodeMetadata:
    """The episode identity carried on its stream's metadata.

    ``app`` is the undashed subject prefix; the service maps it to the dashed
    REST application name. ``name`` is the friendly title; ``status`` is the
    lifecycle label (``setup``/``running``/``stopping``/``ended``/``aborted``);
    ``mode`` records provenance (``live`` vs. an imported ``replay``);
    ``outcome`` is *how* it finished (won/lost/aborted/timed-out), orthogonal to
    whether the stream is sealed; ``created_at`` is an ISO-8601 timestamp.
    """

    app: str
    name: str
    status: str
    mode: str = "live"
    outcome: str | None = None
    created_at: str | None = None
    #: The recruited manifest set (ADR-0005): the manifests scheduled for this
    #: episode, as a list of plain dicts (each manifest's ``model_dump``). This
    #: is *what should be running*; presence on the bus is *what is*, and the
    #: gap between them is liveness. Empty list when the stream carries none
    #: (e.g. a pre-ADR-0005 episode, or one imported from a foreign log).
    manifest_set: list[dict[str, Any]] = field(default_factory=list)
    #: The resolved engine versions (ADR-0005): a map from each manifest's
    #: ``module:QualName`` class ref to its resolved ``distribution==version``
    #: stamp -- *what ran*. Write-only provenance. Empty when nothing is stamped.
    resolved_versions: dict[str, str] = field(default_factory=dict)

    def to_stream_metadata(self) -> dict[str, str]:
        """Encode to the flat ``dict[str, str]`` JetStream stores.

        The manifest set and resolved versions are JSON-serialized (JetStream
        metadata values are strings); both are omitted when empty so a stream
        carrying neither stays minimal.
        """
        data = {
            KEY_APP: self.app,
            KEY_NAME: self.name,
            KEY_STATUS: self.status,
            KEY_MODE: self.mode,
        }
        if self.outcome is not None:
            data[KEY_OUTCOME] = self.outcome
        if self.created_at is not None:
            data[KEY_CREATED_AT] = self.created_at
        if self.manifest_set:
            data[KEY_MANIFEST_SET] = json.dumps(_redact_manifest_set(self.manifest_set))
        if self.resolved_versions:
            data[KEY_RESOLVED_VERSIONS] = json.dumps(self.resolved_versions)
        return data

    @classmethod
    def from_stream_metadata(cls, metadata: dict[str, str]) -> EpisodeMetadata | None:
        """Decode stream metadata, or ``None`` if it is not a FreeAgent episode.

        Tolerant by design: a stream without the FreeAgent ``app`` key is some
        other stream (or one created before metadata was written) and is skipped
        rather than raising, so enumerating streams never crashes on a stranger.
        """
        app = metadata.get(KEY_APP)
        if app is None:
            return None
        return cls(
            app=app,
            name=metadata.get(KEY_NAME, ""),
            status=metadata.get(KEY_STATUS, "unknown"),
            mode=metadata.get(KEY_MODE, "live"),
            outcome=metadata.get(KEY_OUTCOME),
            created_at=metadata.get(KEY_CREATED_AT),
            manifest_set=_decode_json_list(metadata.get(KEY_MANIFEST_SET)),
            resolved_versions=_decode_json_dict(metadata.get(KEY_RESOLVED_VERSIONS)),
        )


def _redact_manifest_set(manifest_set: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Copy *manifest_set* with secret config keys stripped (#68).

    Returns a deep-enough copy -- the caller's manifests (and their configs) are
    never mutated -- with :data:`_REDACTED_CONFIG_KEYS` removed from each
    manifest's ``config``. A manifest with no ``config`` (or a non-dict one) is
    passed through unchanged.
    """
    redacted: list[dict[str, Any]] = []
    for manifest in manifest_set:
        config = manifest.get("config")
        if isinstance(config, dict) and _REDACTED_CONFIG_KEYS.intersection(config):
            clean = dict(manifest)
            clean["config"] = {
                key: value for key, value in config.items() if key not in _REDACTED_CONFIG_KEYS
            }
            redacted.append(clean)
        else:
            redacted.append(manifest)
    return redacted


def _decode_json_list(value: str | None) -> list[dict[str, Any]]:
    """Decode a JSON-encoded list of manifest dicts, tolerant of garbage.

    A missing or malformed value decodes to an empty list rather than raising:
    the manifest set is provenance, never load-bearing for reading an episode,
    so a stranger stream or a truncated value must never crash enumeration.
    """
    if not value:
        return []
    try:
        decoded = json.loads(value)
    except (ValueError, TypeError):
        return []
    if isinstance(decoded, list):
        return [item for item in decoded if isinstance(item, dict)]
    return []


def _decode_json_dict(value: str | None) -> dict[str, str]:
    """Decode a JSON-encoded ``str -> str`` map, tolerant of garbage (see above)."""
    if not value:
        return {}
    try:
        decoded = json.loads(value)
    except (ValueError, TypeError):
        return {}
    if isinstance(decoded, dict):
        return {k: v for k, v in decoded.items() if isinstance(k, str) and isinstance(v, str)}
    return {}
