# The `freeagent` image: the episode service -- an app-independent REST/JetStream
# API and nothing else (ADR-0004). It serves no UI and bundles no application:
# a UI (e.g. the Twenty Questions viewer) is a separate process the operator runs
# on the host, talking to this API over HTTP cross-origin.

# syntax=docker/dockerfile:1

FROM python:3.12-slim AS service
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/
WORKDIR /app

# Install the `freeagent` library PLUS each app's engine-free `*-manifest`
# distribution from the uv workspace, exactly as pinned in uv.lock. The engine
# packages themselves are deliberately excluded -- the service runs no app code
# (ADR-0004) -- but it MUST be able to resolve an app's launch identity to
# enqueue its manifests (ADR-0005). That identity ships as a tiny, string-only
# `twentyquestions-manifest` distribution that registers the `freeagent.manifests`
# entry point and imports NO engine code; installing it is what lets `create`
# resolve `twenty-questions` instead of 404ing "unknown application".
#
# The engine sources are still NOT copied (only the manifest sibling's), so the
# slim image stays engine-free. The engine pyproject/README are copied so the
# frozen lock resolves the workspace.
COPY pyproject.toml uv.lock README.md ./
COPY packages packages
COPY apps/twentyquestions/engine/pyproject.toml apps/twentyquestions/engine/README.md apps/twentyquestions/engine/
COPY apps/twentyquestions/manifest apps/twentyquestions/manifest
# `--extra service` pulls the web stack (FastAPI/uvicorn) the service needs: it
# lives behind a `service` extra now (ADR-0005) so the fat worker image can omit
# it. `--package freeagent` excludes the app engines; `--package
# twentyquestions-manifest` adds only the string-only manifest entry point.
RUN uv sync --frozen --no-dev --package freeagent --package twentyquestions-manifest --extra service

ENV PATH="/app/.venv/bin:$PATH"
EXPOSE 8000

# Default command; the compose file overrides host/port for the network.
CMD ["free-agent", "serve", "--host", "0.0.0.0", "--port", "8000"]
