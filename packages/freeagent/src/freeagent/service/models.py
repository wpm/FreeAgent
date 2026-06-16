"""Wire models for the control service's REST API.

These Pydantic models are the *only* shapes that cross the HTTP boundary. They
deliberately mirror the per-episode tunables of
:class:`~freeagent.cli.config.EpisodeConfig` -- a ``nats_url``, an optional
``episode_id``, and an ``environment`` / ``agents`` ``config`` surface -- so the
settable config an operator drives from a browser is exactly the settable config
the CLI drives from YAML. The framework keeps one notion of "what an operator may
set" (the :class:`~freeagent.AppSpec` ``settable_config`` surface); this is its
REST projection.

The Host's secret, a model override, an agent's grace period, and so on are not
first-class fields here: they ride inside the ``config`` dicts, keyed exactly as
the app's ``settable_config`` advertises them (``agents.host.config.secret``,
``agents.alice.config.model``, ...). The ``parquet_log`` field is the "logging
file" -- record this live episode to that path -- and ``mode`` selects between a
**live** launch and a **replay** of a recorded Parquet log.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

#: An episode is either launched live (real environment + agents over NATS) or
#: replayed from a recorded Parquet log re-published onto NATS.
EpisodeMode = Literal["live", "replay"]


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
      NATS. No environment or agents are launched, so the config blocks and
      ``parquet_log`` do not apply and must be left empty.

    ``episode_id`` is optional: omit it to let the service assign a short id, or
    supply one (it must satisfy ``[A-Za-z0-9_-]+``). ``nats_url`` overrides the
    service's default NATS URL for this episode.
    """

    model_config = ConfigDict(extra="forbid")

    mode: EpisodeMode = "live"
    episode_id: str | None = None
    nats_url: str | None = None
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


class EpisodeView(BaseModel):
    """The REST view of one registered episode -- what list/get/create return.

    ``id`` is the service-level REST id used in the resource path; ``app`` and
    ``episode_id`` are the subject-level identity, and ``subject_root`` is the
    NATS subject root (``<app>.episode.<episode_id>``) a viewer subscribes under.
    For a live episode the REST ``id`` *is* the ``episode_id``; for a replay the
    subject identity comes from the recording, so the two differ and a viewer
    must read ``subject_root`` rather than derive it from ``id``.
    """

    id: str
    application: str
    app: str
    episode_id: str
    subject_root: str
    mode: EpisodeMode
    status: str
    detail: str | None = None
    nats_url: str
    created_at: datetime


class TeardownResult(BaseModel):
    """The result of a teardown: how many episodes were brought down."""

    stopped: int
