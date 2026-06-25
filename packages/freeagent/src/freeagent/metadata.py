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

from dataclasses import dataclass

#: Namespace prefix on every key, so episode metadata never collides with
#: server- or operator-set stream metadata.
_PREFIX = "freeagent_"

KEY_APP = _PREFIX + "app"
KEY_NAME = _PREFIX + "name"
KEY_STATUS = _PREFIX + "status"
KEY_OUTCOME = _PREFIX + "outcome"
KEY_MODE = _PREFIX + "mode"
KEY_CREATED_AT = _PREFIX + "created_at"


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

    def to_stream_metadata(self) -> dict[str, str]:
        """Encode to the flat ``dict[str, str]`` JetStream stores."""
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
        )
