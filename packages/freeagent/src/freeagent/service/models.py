"""Wire models for the episode service's REST API and per-episode feed.

These Pydantic models are the **contract** (ADR-0003): the only shapes that
cross the HTTP/WebSocket boundary between the episode service and any UI. They
are single-sourced here, exported to JSON Schema
(:mod:`freeagent.service.schema`), and compiled to checked-in TypeScript types,
so the cross-language seam carries no hand-maintained duplication. A mock server
(:mod:`freeagent.service.mock`) implements the same contract with canned data so
a UI can be built before the real service exists.

Two halves:

* **Resource models** -- :class:`EpisodeView` (the REST view of one episode) and
  the request bodies for create / rename / import / export. They mirror the
  per-episode tunables of :class:`~freeagent.cli.config.EpisodeConfig`: the
  Host's secret, an agent's model, and so on ride inside the ``config`` dicts,
  keyed exactly as the app's ``settable_config`` advertises them
  (``agents.host.config.secret``, ...). ``model`` and ``api_key`` are
  first-class conveniences the service threads into agent config.
* **Feed models** -- :class:`EpisodeMessage` and the normalized feed events the
  service pushes to the UI over WebSocket/SSE: a message was appended, the
  status/seal changed, or the connection's liveness changed. The UI never parses
  a NATS envelope or subject; it reads these.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

#: An episode is either launched live (real environment + agents over NATS) or
#: replayed from a recorded Parquet log. In the atemporal model (ADR-0003) a
#: replay is just reading a sealed stream, so ``mode`` records *provenance*
#: (was this launched live or imported from a log), not a separate read path.
EpisodeMode = Literal["live", "replay"]

#: Canonical episode status labels. ``setup``/``running``/``stopping`` are the
#: open, live states; ``ended``/``aborted``/``error`` are terminal. These mirror
#: the engine's :class:`~freeagent.EpisodeState` plus the launch-error case.
EpisodeStatusLabel = Literal["setup", "running", "stopping", "ended", "aborted", "error", "unknown"]

#: How an episode *finished*, orthogonal to whether it is sealed: an application
#: result (``won``/``lost``), a framework outcome (``aborted``/``timed_out``), or
#: ``None`` while still open or when the app reports no structured outcome.
EpisodeOutcomeLabel = str

#: The normalized logical channel a feed message belongs to, derived by the
#: service from the NATS subject so the UI never parses a subject.
FeedChannel = Literal["public", "control", "env", "reply", "agent", "log", "other"]


class ComponentConfig(BaseModel):
    """One component's verbatim constructor ``config`` block.

    Mirrors :class:`~freeagent.cli.config.ComponentSpec`: the class is fixed by
    the application in source, only the ``config`` is operator-settable. The
    keys an operator may set are the ones the app's ``settable_config`` surface
    advertises (the Host's ``secret``, an agent's ``model``, and so on).
    """

    model_config = ConfigDict(extra="forbid")

    config: dict[str, Any] = Field(default_factory=dict)


class CreateEpisodeRequest(BaseModel):
    """The body of a create-episode request.

    ``mode`` selects the launch path:

    * **live** -- start a real episode (environment + roster) over NATS. The
      ``environment`` and ``agents`` ``config`` blocks carry the settable config
      (secret, model, timeouts); ``parquet_log`` optionally records the run.
    * **replay** -- re-publish a recorded episode from ``parquet_path`` onto
      NATS. *Deprecated by ADR-0003* in favour of the dedicated import endpoint;
      retained for compatibility. No environment or agents are launched.

    ``episode_id`` is optional: omit it to let the service assign a short id, or
    supply one (it must satisfy ``[A-Za-z0-9_-]+``). ``name`` is an optional
    human-friendly title (auto-generated when omitted). ``model`` and ``api_key``
    are conveniences threaded into every agent's config so an operator can pick a
    model and supply a key without spelling out per-agent ``config`` blocks.
    ``nats_url`` overrides the service's default NATS URL for this episode.
    """

    model_config = ConfigDict(extra="forbid")

    mode: EpisodeMode = "live"
    episode_id: str | None = None
    #: Human-friendly title for the episode; auto-generated when omitted.
    name: str | None = None
    nats_url: str | None = None
    #: Model string (e.g. a litellm model) applied to every agent's config.
    model: str | None = None
    #: Provider API key applied to every agent's config; never put on the bus.
    api_key: str | None = None
    #: Record a *live* episode to this Parquet path (the "logging file"); the
    #: path must not already exist. Ignored for replay.
    parquet_log: str | None = None
    #: The recorded Parquet log to replay; required when ``mode`` is ``replay``.
    parquet_path: str | None = None
    environment: ComponentConfig = Field(default_factory=ComponentConfig)
    agents: dict[str, ComponentConfig] = Field(default_factory=dict)

    @model_validator(mode="after")
    def _check_mode_fields(self) -> CreateEpisodeRequest:
        """Reject field/mode combinations that cannot mean anything coherent."""
        if self.mode == "replay":
            if not self.parquet_path:
                raise ValueError("replay mode requires 'parquet_path'")
            if self.parquet_log:
                raise ValueError("'parquet_log' (recording) does not apply to replay mode")
            if self.environment.config or self.agents:
                raise ValueError("'environment' and 'agents' config do not apply to replay mode")
        elif self.parquet_path:
            raise ValueError("'parquet_path' applies only to replay mode")
        return self


class RenameEpisodeRequest(BaseModel):
    """The body of a rename request: set an episode's friendly name."""

    model_config = ConfigDict(extra="forbid")

    name: str = Field(min_length=1)


class ImportEpisodeRequest(BaseModel):
    """The body of an import request: play a Parquet log into a fresh stream.

    ``parquet_path`` is a path **within** the service's mounted Parquet volume
    (``$FREEAGENT_PARQUET_DIR``); it is resolved relative to that directory and
    may not escape it. ``episode_id`` optionally pins the new episode's id (else
    one is assigned); ``name`` optionally titles it (else a fallback is used).
    """

    model_config = ConfigDict(extra="forbid")

    parquet_path: str
    episode_id: str | None = None
    name: str | None = None


class ExportEpisodeRequest(BaseModel):
    """The body of an export request: drain a sealed episode to Parquet.

    ``parquet_path`` is the destination **within** the service's mounted Parquet
    volume (``$FREEAGENT_PARQUET_DIR``); it is resolved relative to that directory
    and may not escape it, and it must not already exist (a finished log is never
    overwritten).
    """

    model_config = ConfigDict(extra="forbid")

    parquet_path: str


class ExportEpisodeResult(BaseModel):
    """The result of an export: where it was written and how many rows."""

    parquet_path: str
    rows: int


class EpisodeView(BaseModel):
    """The REST view of one episode -- what list/get/create/import return.

    ``id`` is the service-level REST id used in the resource path; ``app`` and
    ``episode_id`` are the subject-level identity, and ``subject_root`` is the
    NATS subject root (``<app>.episode.<episode_id>``). The UI does not speak
    NATS (ADR-0003) and need not use ``subject_root``; it is retained for tools
    that do.

    The atemporal fields (ADR-0003): ``name`` is the friendly title; ``sealed``
    is the binary "can this still change?" (open/live vs. sealed); ``status`` is
    the lifecycle label; and ``outcome`` is *how* it finished (won/lost/aborted/
    timed-out), orthogonal to ``sealed``.
    """

    id: str
    application: str
    app: str
    episode_id: str
    subject_root: str
    #: Friendly, human-readable title; always present (auto-generated fallback).
    name: str
    mode: EpisodeMode
    status: str
    #: ``True`` once the episode is complete and immutable (JetStream sealed).
    sealed: bool = False
    #: How the episode finished, orthogonal to ``sealed``; ``None`` while open.
    outcome: EpisodeOutcomeLabel | None = None
    detail: str | None = None
    nats_url: str
    created_at: datetime
    #: The recruited manifest set (ADR-0005): *what should be running* -- the
    #: manifests scheduled for this episode, as plain dicts. Never a PID. Empty
    #: until a worker's environment child writes the stream (and for episodes
    #: that predate the worker pool, or were imported from a foreign log).
    manifest_set: list[dict[str, Any]] = Field(default_factory=list)
    #: The resolved engine versions (ADR-0005): *what ran* -- a map from each
    #: manifest's ``module:QualName`` class ref to its ``distribution==version``
    #: stamp. Write-only provenance; empty when nothing is stamped.
    resolved_versions: dict[str, str] = Field(default_factory=dict)


class EpisodeMessage(BaseModel):
    """One message in an episode's feed -- a normalized stream row.

    This is the recorder's row shape (:mod:`freeagent.recorder`) projected for
    the UI: the authoritative ``stream_seq`` order, the ``channel`` the service
    derived from the subject, the ``sender``, the server ``received_at``, and the
    app-defined ``payload`` (opaque to the framework). ``subject`` is kept for
    tools that want it; the UI should prefer ``channel``.
    """

    stream_seq: int
    subject: str
    channel: FeedChannel
    sender: str
    received_at: datetime
    payload: Any = None


class FeedMessageEvent(BaseModel):
    """Feed event: a message was appended to (or replayed from) the episode."""

    type: Literal["message"] = "message"
    message: EpisodeMessage


class FeedStatusEvent(BaseModel):
    """Feed event: the episode's status, seal, name, or outcome changed.

    The service sends one of these when a feed opens (the current snapshot) and
    again whenever any of these change -- so the UI can render the left-pane
    label and the skin's status/budget chips without polling.
    """

    type: Literal["status"] = "status"
    status: str
    sealed: bool
    name: str
    outcome: EpisodeOutcomeLabel | None = None
    detail: str | None = None


class FeedConnectionEvent(BaseModel):
    """Feed event: the feed connection's liveness changed.

    ``phase`` is ``open`` when the consumer is attached and history is
    streaming, ``live`` at the boundary where history is drained and the feed is
    now tailing new appends (the moment that, by design, an observer *cannot*
    distinguish live from replay), ``closed`` on a clean end, and ``error`` on a
    failure. ``detail`` carries an optional human-readable note.
    """

    type: Literal["connection"] = "connection"
    phase: Literal["open", "live", "closed", "error"]
    detail: str | None = None


class TeardownResult(BaseModel):
    """The result of a teardown: how many episodes were brought down."""

    stopped: int
