"""The Free Agent REST API: launch and manage episodes, serve their traffic to viewers.

The REST surface over :mod:`freeagent.api.episodes`:

- ``GET  /applications`` — the installed applications, from
  :func:`~freeagent.sdk.available_applications`.
- ``POST /applications/{application}/episodes`` — provision an episode: an optional episode ID
  plus an opaque config; spawns a worker process to run it. Unknown application: 404.
- ``GET  /applications/{application}/episodes`` — the application's episodes' lifecycle statuses,
  in creation order. Unknown application: 404.
- ``GET  /applications/{application}/episodes/{episode_id}`` — lifecycle status derived from
  control-plane traffic.
- ``GET  /applications/{application}/episodes/{episode_id}/messages`` — the ordered pass-through
  feed of data-plane JSON.
- ``DELETE /applications/{application}/episodes/{episode_id}`` — stop the episode.

The app is built by :func:`create_app`, which wires one
:class:`~freeagent.api.episodes.EpisodeManager` into the routes and tears it down on lifespan
shutdown. The NATS URL comes from the ``FREEAGENT_NATS_URL`` environment variable unless given
explicitly.
"""

from __future__ import annotations

import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any

from fastapi import FastAPI, HTTPException, Response
from fastapi.middleware.cors import CORSMiddleware
from freeagent.api.episodes import (
    SUBJECT_TOKEN_PATTERN,
    DataPlaneRecord,
    DuplicateEpisode,
    Episode,
    EpisodeManager,
    EpisodeStatus,
    UnknownEpisode,
)
from freeagent.sdk import UnknownApplication, available_applications
from freeagent.sdk.entity import DEFAULT_NATS_SERVER
from pydantic import BaseModel, Field

NATS_URL_ENV = "FREEAGENT_NATS_URL"
"""Environment variable naming the NATS server URL the API listens on and hands to workers."""

EPISODE_ID_PATTERN = rf"^{SUBJECT_TOKEN_PATTERN}$"
"""A client-supplied episode ID must be a single NATS subject token — anchored form of the one
definition in :data:`freeagent.api.episodes.SUBJECT_TOKEN_PATTERN`."""


class CreateEpisodeRequest(BaseModel):
    """The body of a ``POST .../episodes`` request.

    :ivar episode_id: The episode's identifier, or ``None`` to have one generated. Must be a
        single NATS subject token, since it becomes part of the episode's root subject.
    :ivar config: The application-defined episode config, opaque to the API.
    """

    episode_id: str | None = Field(default=None, pattern=EPISODE_ID_PATTERN)
    config: dict[str, Any] = Field(default_factory=dict)


class ApplicationsResponse(BaseModel):
    """The body of a ``GET /applications`` response.

    :ivar applications: The bare names of every installed application, sorted.
    """

    applications: list[str]


class EpisodesResponse(BaseModel):
    """The body of a ``GET .../episodes`` response.

    :ivar episodes: The application's episodes' statuses, in creation order.
    """

    episodes: list[EpisodeStatus]


class MessagesResponse(BaseModel):
    """The body of a ``GET .../messages`` response.

    :ivar messages: The episode's data-plane feed so far, in arrival order.
    """

    messages: list[DataPlaneRecord]


def create_app(nats_url: str | None = None, manager: EpisodeManager | None = None) -> FastAPI:
    """Build and return the FastAPI application.

    :param nats_url: The NATS server URL; falls back to :data:`NATS_URL_ENV`, then the SDK
        default.
    :param manager: The episode manager to serve from, injectable for tests; built from
        ``nats_url`` when omitted.
    :return: The configured application, whose lifespan shutdown tears the manager down.
    """
    resolved_url = nats_url or os.environ.get(NATS_URL_ENV) or DEFAULT_NATS_SERVER
    episode_manager = manager if manager is not None else EpisodeManager(resolved_url)

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        """Run the app, then stop every episode and drop the NATS connection on shutdown.

        :param app: The application being run (unused; the manager is closed over).
        """
        try:
            yield
        finally:
            await episode_manager.shutdown()

    app = FastAPI(title="Free Agent API", version="0.1.0", lifespan=lifespan)
    # Viewers are static pages served from their own origin, so the browser gates
    # their API calls on CORS. Wide open, deliberately: this is a trusted research testbed
    # where security is a non-concern, and the API carries no credentials.
    app.add_middleware(
        CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"]
    )
    app.state.manager = episode_manager

    def _lookup(application: str, episode_id: str) -> Episode:
        """Fetch an episode or translate its absence into a 404.

        :param application: The application from the request path.
        :param episode_id: The episode ID from the request path.
        :return: The episode.
        :raises HTTPException: With status 404 if no such episode exists.
        """
        try:
            return episode_manager.get(application, episode_id)
        except UnknownEpisode as error:
            raise HTTPException(status_code=404, detail=str(error)) from None

    @app.get("/health")
    async def health() -> dict[str, str]:
        return {"status": "ok"}

    @app.get("/applications")
    async def applications() -> ApplicationsResponse:
        return ApplicationsResponse(applications=sorted(available_applications()))

    @app.get("/applications/{application}/episodes")
    async def list_episodes(application: str) -> EpisodesResponse:
        try:
            episodes = episode_manager.list_episodes(application)
        except UnknownApplication as error:
            raise HTTPException(status_code=404, detail=str(error)) from None
        return EpisodesResponse(episodes=[episode.status() for episode in episodes])

    @app.post("/applications/{application}/episodes", status_code=201)
    async def create_episode(application: str, request: CreateEpisodeRequest) -> EpisodeStatus:
        try:
            episode = await episode_manager.create(application, request.episode_id, request.config)
        except UnknownApplication as error:
            raise HTTPException(status_code=404, detail=str(error)) from None
        except DuplicateEpisode as error:
            raise HTTPException(status_code=409, detail=str(error)) from None
        return episode.status()

    @app.get("/applications/{application}/episodes/{episode_id}")
    async def episode_status(application: str, episode_id: str) -> EpisodeStatus:
        return _lookup(application, episode_id).status()

    @app.get("/applications/{application}/episodes/{episode_id}/messages")
    async def episode_messages(application: str, episode_id: str) -> MessagesResponse:
        return MessagesResponse(messages=list(_lookup(application, episode_id).monitor.messages))

    @app.delete("/applications/{application}/episodes/{episode_id}", status_code=204)
    async def stop_episode(application: str, episode_id: str) -> Response:
        try:
            await episode_manager.stop(application, episode_id)
        except UnknownEpisode as error:
            raise HTTPException(status_code=404, detail=str(error)) from None
        return Response(status_code=204)

    return app


app = create_app()
"""The module-level app ``uvicorn freeagent.api.app:app`` serves; see ``__main__``."""
