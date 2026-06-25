# The `freeagent` image: the episode service -- an app-independent REST/JetStream
# API and nothing else (ADR-0004). It serves no UI and bundles no application:
# a UI (e.g. the Twenty Questions viewer) is a separate process the operator runs
# on the host, talking to this API over HTTP cross-origin.

# syntax=docker/dockerfile:1

FROM python:3.12-slim AS service
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/
WORKDIR /app

# Install ONLY the `freeagent` library from the uv workspace, exactly as pinned
# in uv.lock (`--package freeagent` excludes the companion app engines). The
# workspace manifests are needed to resolve against the frozen lock; the app
# engine sources are deliberately NOT copied -- the service runs no app code.
COPY pyproject.toml uv.lock README.md ./
COPY packages packages
COPY apps/twentyquestions/engine/pyproject.toml apps/twentyquestions/engine/README.md apps/twentyquestions/engine/
# `--extra service` pulls the web stack (FastAPI/uvicorn) the service needs: it
# lives behind a `service` extra now (ADR-0005) so the fat worker image can omit
# it. `--package freeagent` still excludes the app engines.
RUN uv sync --frozen --no-dev --package freeagent --extra service

ENV PATH="/app/.venv/bin:$PATH"
EXPOSE 8000

# Default command; the compose file overrides host/port for the network.
CMD ["free-agent", "serve", "--host", "0.0.0.0", "--port", "8000"]
