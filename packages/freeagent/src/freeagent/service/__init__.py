"""The FreeAgent control service: a long-running REST API over episodes.

The persistent piece of the control plane. It hosts a small REST API under
``/freeagent/<application>/<episode>`` and is **provision-only** (ADR-0005): a
create *enqueues* an episode's manifest set onto the shared work queue (a pool of
workers launches it) and the service holds **no** process handle. ``list``/
``get`` read the durable JetStream record (ADR-0003); ``stop`` aborts gracefully
through the operator protocol over the episode's ``.env`` subject -- all
observable over NATS exactly like a ``run``-launched episode. Because it launches
nothing in-process, the slim service image can ``create`` an episode for an app
whose **engine it does not have installed**.

Two layers, separately usable:

* :class:`ControlService` -- the framework-only brain (create/list/get/stop/
  teardown), no web dependency.
* :func:`create_app` -- the FastAPI projection of that brain onto HTTP, with
  CORS for the browser viewer.

See :func:`run` (and ``python -m freeagent.service``) to serve it.
"""

from __future__ import annotations

from .app import DEFAULT_DEV_ORIGINS, create_app
from .edgeio import EdgeIOError, ImportResult, export_episode, import_episode
from .feed import EpisodeFeed, NatsStreamSource, StreamRow, StreamSource, to_episode_message
from .mock import create_mock_app
from .models import (
    ComponentConfig,
    CreateEpisodeRequest,
    EpisodeMessage,
    EpisodeMode,
    EpisodeView,
    ExportEpisodeRequest,
    ExportEpisodeResult,
    FeedConnectionEvent,
    FeedMessageEvent,
    FeedStatusEvent,
    ImportEpisodeRequest,
    RenameEpisodeRequest,
    TeardownResult,
)
from .registry import (
    ControlService,
    EpisodeExistsError,
    EpisodeNotFoundError,
    NatsUnreachableError,
    ServiceError,
    verify_nats_reachable,
)
from .server import DEFAULT_HOST, DEFAULT_PORT, run
from .store import EpisodeRecord, EpisodeStore

__all__ = [
    "DEFAULT_DEV_ORIGINS",
    "DEFAULT_HOST",
    "DEFAULT_PORT",
    "ComponentConfig",
    "ControlService",
    "CreateEpisodeRequest",
    "EdgeIOError",
    "EpisodeExistsError",
    "EpisodeFeed",
    "EpisodeMessage",
    "EpisodeMode",
    "EpisodeNotFoundError",
    "EpisodeRecord",
    "EpisodeStore",
    "EpisodeView",
    "ExportEpisodeRequest",
    "ExportEpisodeResult",
    "FeedConnectionEvent",
    "FeedMessageEvent",
    "FeedStatusEvent",
    "ImportEpisodeRequest",
    "ImportResult",
    "NatsStreamSource",
    "NatsUnreachableError",
    "RenameEpisodeRequest",
    "ServiceError",
    "StreamRow",
    "StreamSource",
    "TeardownResult",
    "create_app",
    "create_mock_app",
    "export_episode",
    "import_episode",
    "run",
    "to_episode_message",
    "verify_nats_reachable",
]
