"""Entry point for running the Free Agent API server."""

import uvicorn


def main() -> None:
    """Run the API server with uvicorn."""
    uvicorn.run("freeagent.api.app:app", host="127.0.0.1", port=8000, reload=True)


if __name__ == "__main__":
    main()
