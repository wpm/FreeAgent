"""Application factory for the Free Agent API."""

from fastapi import FastAPI


def create_app() -> FastAPI:
    """Build and return the FastAPI application."""
    app = FastAPI(title="Free Agent API", version="0.1.0")

    @app.get("/health")
    async def health() -> dict[str, str]:
        return {"status": "ok"}

    return app


app = create_app()
