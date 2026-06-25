"""The FastAPI application: the REST boundary onto :class:`ControlService`.

This module is the **only** place wire subjects, HTTP, and CORS meet. The REST
resources live under ``/freeagent/<application>/<episode>`` and map, at this
boundary, onto the existing NATS subject layout (:mod:`freeagent.subjects`) --
the service introduces no new wire subjects. Endpoints:

* ``POST   /freeagent/{application}/episodes``                 -- create (live)
* ``GET    /freeagent/episodes``                               -- list (the "display")
* ``GET    /freeagent/{application}/episodes/{episode}``       -- get one's status
* ``PATCH  /freeagent/{application}/episodes/{episode}``       -- rename
* ``DELETE /freeagent/{application}/episodes/{episode}``       -- delete (remove stream)
* ``POST   /freeagent/{application}/episodes/{episode}/stop``  -- graceful stop
* ``POST   /freeagent/teardown``                               -- bring everything down

The feed (``.../feed`` WebSocket) and edge I/O (import/export) are registered by
their own modules (#42, #43).

The service binds to localhost with no auth, consistent with the no-auth local
NATS testbed (``SECURITY.md``), and configures CORS so a cross-origin dev UI can
call it. It can also **serve the built UI bundle** from its own origin (ADR-0003)
when a bundle directory is configured, so the whole app is one process.

A NATS reachability probe runs on startup (logging a clear warning if NATS is
down, rather than refusing to start) and again on every create (a hard 503 if
NATS is down) -- never a hang.
"""

from __future__ import annotations

import contextlib
import logging
import os
from pathlib import Path
from typing import TYPE_CHECKING

from fastapi import FastAPI, Request, Response, status
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from fastapi.staticfiles import StaticFiles

from freeagent.cli.apps import UnknownAppError
from freeagent.cli.config import ConfigError, default_nats_url
from freeagent.replayer import ReplayerError

from .edgeio import EdgeIOError
from .feed import register_feed_route
from .models import (
    CreateEpisodeRequest,
    EpisodeView,
    ExportEpisodeRequest,
    ExportEpisodeResult,
    ImportEpisodeRequest,
    RenameEpisodeRequest,
    TeardownResult,
)
from .registry import (
    ControlService,
    EpisodeExistsError,
    EpisodeNotFoundError,
    NatsUnreachableError,
    verify_nats_reachable,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Sequence

logger = logging.getLogger("freeagent.service")

#: Origins allowed by default: a dev UI's Vite server (its default ``:5173``) on
#: both loopback spellings. When the service serves the bundle itself, the UI is
#: same-origin and needs no CORS, but the dev-server allowance stays for the
#: cross-origin dev loop.
DEFAULT_DEV_ORIGINS: tuple[str, ...] = (
    "http://localhost:5173",
    "http://127.0.0.1:5173",
)

#: Environment variable naming the built UI bundle directory to serve.
UI_DIR_ENV_VAR = "FREEAGENT_UI_DIR"


def create_app(
    *,
    nats_url: str | None = None,
    allowed_origins: Sequence[str] | None = None,
    service: ControlService | None = None,
    ui_dir: str | Path | None = None,
) -> FastAPI:
    """Build the episode-service FastAPI app.

    *nats_url* is the default NATS URL (falls back to ``$FREEAGENT_NATS_URL`` or
    localhost); *allowed_origins* overrides the CORS allow-list. Passing a
    ready-made *service* is for tests; otherwise one is constructed over
    *nats_url*. *ui_dir* (or ``$FREEAGENT_UI_DIR``) is a built UI bundle to serve
    from this origin; when set and present, it is mounted at ``/``.
    """
    resolved_url = nats_url or default_nats_url()
    control = service if service is not None else ControlService(nats_url=resolved_url)
    origins = list(allowed_origins) if allowed_origins is not None else list(DEFAULT_DEV_ORIGINS)

    @contextlib.asynccontextmanager
    async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
        # Verify NATS on startup, but do not refuse to start: the operator may
        # bring NATS up moments later, and every create re-checks anyway.
        try:
            await verify_nats_reachable(control.default_nats_url)
        except NatsUnreachableError as exc:
            logger.warning("episode service starting without NATS: %s", exc)
        try:
            yield
        finally:
            # Never leak an episode's child processes past the service.
            await control.teardown()

    app = FastAPI(
        title="FreeAgent episode service",
        summary="Create, observe, rename, delete, and stop FreeAgent episodes.",
        lifespan=lifespan,
    )
    app.state.service = control
    app.add_middleware(
        CORSMiddleware,
        allow_origins=origins,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    _register_routes(app)
    register_feed_route(app)
    _register_exception_handlers(app)
    _mount_ui(app, ui_dir if ui_dir is not None else os.environ.get(UI_DIR_ENV_VAR))
    return app


def _register_routes(app: FastAPI) -> None:
    def control(request: Request) -> ControlService:
        return request.app.state.service  # set in create_app

    @app.post(
        "/freeagent/{application}/episodes",
        response_model=EpisodeView,
        status_code=status.HTTP_201_CREATED,
        tags=["episodes"],
    )
    async def create_episode(
        application: str, body: CreateEpisodeRequest, request: Request
    ) -> EpisodeView:
        return await control(request).create(application, body)

    @app.get("/freeagent/episodes", response_model=list[EpisodeView], tags=["episodes"])
    async def list_episodes(request: Request) -> list[EpisodeView]:
        return await control(request).list()

    @app.get(
        "/freeagent/{application}/episodes/{episode_id}",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def get_episode(application: str, episode_id: str, request: Request) -> EpisodeView:
        return await control(request).get(application, episode_id)

    @app.patch(
        "/freeagent/{application}/episodes/{episode_id}",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def rename_episode(
        application: str, episode_id: str, body: RenameEpisodeRequest, request: Request
    ) -> EpisodeView:
        return await control(request).rename(application, episode_id, body.name)

    @app.delete(
        "/freeagent/{application}/episodes/{episode_id}",
        status_code=status.HTTP_204_NO_CONTENT,
        tags=["episodes"],
    )
    async def delete_episode(application: str, episode_id: str, request: Request) -> Response:
        await control(request).delete(application, episode_id)
        return Response(status_code=status.HTTP_204_NO_CONTENT)

    @app.post(
        "/freeagent/{application}/episodes/{episode_id}/stop",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def stop_episode(application: str, episode_id: str, request: Request) -> EpisodeView:
        return await control(request).stop(application, episode_id)

    @app.post(
        "/freeagent/{application}/episodes/{episode_id}/export",
        response_model=ExportEpisodeResult,
        tags=["edge-io"],
    )
    async def export_episode(
        application: str, episode_id: str, body: ExportEpisodeRequest, request: Request
    ) -> ExportEpisodeResult:
        return await control(request).export(application, episode_id, body.parquet_path)

    @app.post(
        "/freeagent/import",
        response_model=EpisodeView,
        status_code=status.HTTP_201_CREATED,
        tags=["edge-io"],
    )
    async def import_episode(body: ImportEpisodeRequest, request: Request) -> EpisodeView:
        return await control(request).import_(
            body.parquet_path, episode_id=body.episode_id, name=body.name
        )

    @app.post("/freeagent/teardown", response_model=TeardownResult, tags=["service"])
    async def teardown(request: Request) -> TeardownResult:
        return TeardownResult(stopped=await control(request).teardown())


def _mount_ui(app: FastAPI, ui_dir: str | Path | None) -> None:
    """Serve a built UI bundle from this origin, if one is configured and present.

    Mounted last and at ``/`` with ``html=True`` so the API routes registered
    above still win for their paths and any other path falls through to the SPA's
    ``index.html``. A missing or unset directory is simply not mounted (the
    service stays API-only, as in development).
    """
    if ui_dir is None:
        return
    directory = Path(ui_dir)
    if not directory.is_dir():
        logger.warning("UI bundle directory %s does not exist; serving API only", directory)
        return
    app.mount("/", StaticFiles(directory=str(directory), html=True), name="ui")
    logger.info("serving UI bundle from %s", directory)


def _register_exception_handlers(app: FastAPI) -> None:
    """Map the framework's and service's errors onto clean HTTP responses."""

    def problem(code: int, message: str) -> JSONResponse:
        return JSONResponse(status_code=code, content={"detail": message})

    @app.exception_handler(UnknownAppError)
    async def _unknown_app(_request: Request, exc: UnknownAppError) -> JSONResponse:
        return problem(status.HTTP_404_NOT_FOUND, str(exc))

    @app.exception_handler(EpisodeNotFoundError)
    async def _not_found(_request: Request, exc: EpisodeNotFoundError) -> JSONResponse:
        return problem(status.HTTP_404_NOT_FOUND, str(exc))

    @app.exception_handler(EpisodeExistsError)
    async def _exists(_request: Request, exc: EpisodeExistsError) -> JSONResponse:
        return problem(status.HTTP_409_CONFLICT, str(exc))

    @app.exception_handler(NatsUnreachableError)
    async def _nats_down(_request: Request, exc: NatsUnreachableError) -> JSONResponse:
        return problem(status.HTTP_503_SERVICE_UNAVAILABLE, str(exc))

    @app.exception_handler(ReplayerError)
    async def _bad_replay(_request: Request, exc: ReplayerError) -> JSONResponse:
        return problem(status.HTTP_400_BAD_REQUEST, str(exc))

    @app.exception_handler(EdgeIOError)
    async def _bad_edge_io(_request: Request, exc: EdgeIOError) -> JSONResponse:
        return problem(status.HTTP_400_BAD_REQUEST, str(exc))

    @app.exception_handler(ConfigError)
    async def _bad_config(_request: Request, exc: ConfigError) -> JSONResponse:
        # ConfigError is the base of UnknownAppError; the more specific handler
        # above wins for that subclass, so this is the generic-config case.
        return problem(status.HTTP_400_BAD_REQUEST, str(exc))

    @app.exception_handler(ValueError)
    async def _bad_value(_request: Request, exc: ValueError) -> JSONResponse:
        return problem(status.HTTP_400_BAD_REQUEST, str(exc))
