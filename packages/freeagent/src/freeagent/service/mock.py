"""A mock episode service: the contract (ADR-0003) with canned data.

This implements the full episode REST API and the per-episode feed over
WebSocket, backed by an in-memory list of canned episodes and a scripted Twenty
Questions transcript -- **no NATS, no child processes**. It exists so a UI can be
built against the contract before the real service (#41/#42) exists: list and
open episodes, watch a feed stream history then go "live", create / rename /
delete, and import / export, all returning the same Pydantic models the real
service returns.

Run it with ``python -m freeagent.service.mock`` (defaults to ``:8000``), point
the UI's connection setting at it, and develop. The OpenAPI page at ``/docs``
documents the surface.
"""

from __future__ import annotations

import asyncio
import contextlib
import secrets
from datetime import UTC, datetime, timedelta
from typing import TYPE_CHECKING

from fastapi import FastAPI, WebSocket, WebSocketDisconnect, status
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse, Response

from freeagent.names import fallback_episode_name
from freeagent.subjects import channel_of, subject_root

from .app import DEFAULT_DEV_ORIGINS
from .models import (
    CreateEpisodeRequest,
    EpisodeMessage,
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

if TYPE_CHECKING:
    from collections.abc import Sequence

#: The single application the canned data is for.
_APP = "twentyquestions"
_APPLICATION = "twenty-questions"
_BASE_TIME = datetime(2026, 6, 25, 12, 0, 0, tzinfo=UTC)


def _msg(seq: int, sender: str, payload: object, *, channel: str = "public") -> EpisodeMessage:
    """Build one canned feed message on a given channel."""
    root = subject_root(_APP, "seed")
    subject = f"{root}.{channel}" if channel != "agent" else f"{root}.agent.{sender}"
    return EpisodeMessage(
        stream_seq=seq,
        subject=subject,
        channel=channel_of(subject),
        sender=sender,
        received_at=_BASE_TIME + timedelta(seconds=seq),
        payload=payload,
    )


#: A scripted Twenty Questions transcript, reused as every canned episode's feed.
_TRANSCRIPT: tuple[EpisodeMessage, ...] = (
    _msg(1, "env", {"type": "start"}, channel="control"),
    _msg(2, "host", "I'm thinking of something. Ask me yes/no questions!"),
    _msg(3, "alice", "Is it an animal?"),
    _msg(4, "host", "Yes, it is an animal."),
    _msg(5, "bob", "Is it bigger than a breadbox?"),
    _msg(6, "host", "Yes, much bigger."),
    _msg(7, "carol", "Is it an elephant?"),
    _msg(8, "host", "Yes! You got it in 3 questions."),
    _msg(
        9,
        "host",
        {"type": "game_over", "outcome": "win", "secret": "elephant", "questions_asked": 3},
        channel="env",
    ),
    _msg(10, "env", {"type": "shutdown"}, channel="control"),
)


def _seed_episodes() -> list[EpisodeView]:
    """The canned episodes a fresh mock starts with: one sealed, one live."""
    now = datetime.now(UTC)
    return [
        EpisodeView(
            id="elephant",
            application=_APPLICATION,
            app=_APP,
            episode_id="elephant",
            subject_root=subject_root(_APP, "elephant"),
            name="elephant",
            mode="live",
            status="ended",
            sealed=True,
            outcome="won",
            detail="won in 3 questions",
            nats_url="nats://localhost:4222",
            created_at=now - timedelta(minutes=20),
        ),
        EpisodeView(
            id="a1b2c3d4",
            application=_APPLICATION,
            app=_APP,
            episode_id="a1b2c3d4",
            subject_root=subject_root(_APP, "a1b2c3d4"),
            name=fallback_episode_name("a1b2c3d4"),
            mode="live",
            status="running",
            sealed=False,
            outcome=None,
            detail=None,
            nats_url="nats://localhost:4222",
            created_at=now - timedelta(minutes=2),
        ),
    ]


def create_mock_app(*, allowed_origins: Sequence[str] | None = None) -> FastAPI:
    """Build the mock episode service: the contract over canned, in-memory data."""
    episodes: dict[str, EpisodeView] = {ep.id: ep for ep in _seed_episodes()}
    origins = list(allowed_origins) if allowed_origins is not None else list(DEFAULT_DEV_ORIGINS)

    app = FastAPI(
        title="FreeAgent episode service (mock)",
        summary="Canned implementation of the ADR-0003 episode contract for UI development.",
    )
    app.add_middleware(
        CORSMiddleware,
        allow_origins=origins,
        allow_methods=["*"],
        allow_headers=["*"],
    )

    def _not_found(episode_id: str) -> JSONResponse:
        return JSONResponse(status_code=status.HTTP_404_NOT_FOUND, content={"detail": episode_id})

    @app.get("/freeagent/episodes", response_model=list[EpisodeView], tags=["episodes"])
    async def list_episodes() -> list[EpisodeView]:
        return list(episodes.values())

    @app.post(
        "/freeagent/{application}/episodes",
        response_model=EpisodeView,
        status_code=status.HTTP_201_CREATED,
        tags=["episodes"],
    )
    async def create_episode(application: str, body: CreateEpisodeRequest) -> EpisodeView:
        episode_id = body.episode_id or secrets.token_hex(4)
        view = EpisodeView(
            id=episode_id,
            application=application,
            app=_APP,
            episode_id=episode_id,
            subject_root=subject_root(_APP, episode_id),
            name=body.name or fallback_episode_name(episode_id),
            mode="live",
            status="running",
            sealed=False,
            outcome=None,
            detail=None,
            nats_url=body.nats_url or "nats://localhost:4222",
            created_at=datetime.now(UTC),
        )
        episodes[episode_id] = view
        return view

    @app.get(
        "/freeagent/{application}/episodes/{episode_id}",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def get_episode(application: str, episode_id: str) -> EpisodeView | JSONResponse:  # noqa: ARG001 -- 'application' binds the path param; canned data ignores it
        view = episodes.get(episode_id)
        if view is None:
            return _not_found(episode_id)
        return view

    @app.patch(
        "/freeagent/{application}/episodes/{episode_id}",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def rename_episode(
        application: str,  # noqa: ARG001 -- binds the path param; canned data ignores it
        episode_id: str,
        body: RenameEpisodeRequest,
    ) -> EpisodeView | JSONResponse:
        view = episodes.get(episode_id)
        if view is None:
            return _not_found(episode_id)
        view = view.model_copy(update={"name": body.name})
        episodes[episode_id] = view
        return view

    @app.delete(
        "/freeagent/{application}/episodes/{episode_id}",
        status_code=status.HTTP_204_NO_CONTENT,
        tags=["episodes"],
    )
    async def delete_episode(application: str, episode_id: str) -> Response:  # noqa: ARG001 -- binds the path param; canned data ignores it
        if episodes.pop(episode_id, None) is None:
            return _not_found(episode_id)
        return Response(status_code=status.HTTP_204_NO_CONTENT)

    @app.post(
        "/freeagent/{application}/episodes/{episode_id}/stop",
        response_model=EpisodeView,
        tags=["episodes"],
    )
    async def stop_episode(application: str, episode_id: str) -> EpisodeView | JSONResponse:  # noqa: ARG001 -- binds the path param; canned data ignores it
        view = episodes.get(episode_id)
        if view is None:
            return _not_found(episode_id)
        view = view.model_copy(
            update={"status": "aborted", "sealed": True, "outcome": "aborted", "detail": "stopped"}
        )
        episodes[episode_id] = view
        return view

    @app.post(
        "/freeagent/{application}/episodes/{episode_id}/export",
        response_model=ExportEpisodeResult,
        tags=["edge-io"],
    )
    async def export_episode(
        application: str,  # noqa: ARG001 -- binds the path param; canned data ignores it
        episode_id: str,
        body: ExportEpisodeRequest,
    ) -> ExportEpisodeResult | JSONResponse:
        if episode_id not in episodes:
            return _not_found(episode_id)
        return ExportEpisodeResult(parquet_path=body.parquet_path, rows=len(_TRANSCRIPT))

    @app.post(
        "/freeagent/import",
        response_model=EpisodeView,
        status_code=status.HTTP_201_CREATED,
        tags=["edge-io"],
    )
    async def import_episode(body: ImportEpisodeRequest) -> EpisodeView:
        episode_id = body.episode_id or secrets.token_hex(4)
        view = EpisodeView(
            id=episode_id,
            application=_APPLICATION,
            app=_APP,
            episode_id=episode_id,
            subject_root=subject_root(_APP, episode_id),
            name=body.name or fallback_episode_name(episode_id),
            mode="replay",
            status="ended",
            sealed=True,
            outcome="won",
            detail=f"imported from {body.parquet_path}",
            nats_url="nats://localhost:4222",
            created_at=datetime.now(UTC),
        )
        episodes[episode_id] = view
        return view

    @app.post("/freeagent/teardown", response_model=TeardownResult, tags=["service"])
    async def teardown() -> TeardownResult:
        count = len(episodes)
        episodes.clear()
        return TeardownResult(stopped=count)

    @app.websocket("/freeagent/{application}/episodes/{episode_id}/feed")
    async def feed(websocket: WebSocket, application: str, episode_id: str) -> None:  # noqa: ARG001 -- 'application' binds the path param; canned data ignores it
        await websocket.accept()
        view = episodes.get(episode_id)
        if view is None:
            await websocket.send_text(
                FeedConnectionEvent(phase="error", detail="no such episode").model_dump_json()
            )
            await websocket.close()
            return
        try:
            # Snapshot, then history, then the live boundary -- the same shape
            # the real feed (#42) produces, so an observer cannot tell the canned
            # mock from the real thing.
            await websocket.send_text(
                FeedStatusEvent(
                    status=view.status,
                    sealed=view.sealed,
                    name=view.name,
                    outcome=view.outcome,
                    detail=view.detail,
                ).model_dump_json()
            )
            await websocket.send_text(FeedConnectionEvent(phase="open").model_dump_json())
            for message in _TRANSCRIPT:
                await websocket.send_text(FeedMessageEvent(message=message).model_dump_json())
                await asyncio.sleep(0.1)
            await websocket.send_text(FeedConnectionEvent(phase="live").model_dump_json())
            if view.sealed:
                await websocket.send_text(FeedConnectionEvent(phase="closed").model_dump_json())
            else:
                # A live episode: hold the socket open until the client leaves
                # (the task is cancelled on disconnect/shutdown).
                await asyncio.Event().wait()
        except WebSocketDisconnect:
            return
        with contextlib.suppress(RuntimeError):
            await websocket.close()

    return app


def run(*, host: str = "127.0.0.1", port: int = 8000) -> None:
    """Serve the mock on localhost (blocks until interrupted)."""
    import uvicorn

    uvicorn.run(create_mock_app(), host=host, port=port, log_level="info")


if __name__ == "__main__":
    run()
