# The `freeagent` image: the episode service (REST + feed) plus the built UI
# bundle it serves from one origin (ADR-0003). Two stages -- build the TypeScript
# bundle with Node, then assemble the Python service and copy the bundle in.

# syntax=docker/dockerfile:1

# --- Stage 1: build the UI bundle -------------------------------------------
FROM node:22-alpine AS ui
RUN corepack enable
WORKDIR /src

# Install the JS workspace from the manifests first (cached across source edits).
COPY pnpm-workspace.yaml package.json pnpm-lock.yaml ./
COPY apps/twentyquestions/viewer/package.json apps/twentyquestions/viewer/package.json
RUN pnpm install --frozen-lockfile

# The generated types come from the committed JSON Schemas; build the bundle.
COPY packages/freeagent/schemas packages/freeagent/schemas
COPY apps/twentyquestions/schemas apps/twentyquestions/schemas
COPY apps/twentyquestions/viewer apps/twentyquestions/viewer
RUN pnpm --filter twentyquestions-viewer run build

# --- Stage 2: the Python episode service ------------------------------------
FROM python:3.12-slim AS service
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/
WORKDIR /app

# Install the uv workspace (the freeagent library + the twentyquestions engine),
# exactly as pinned in uv.lock. The service launches episodes as child
# processes, so the app must be importable in this image.
COPY pyproject.toml uv.lock README.md ./
COPY packages packages
COPY apps/twentyquestions/engine apps/twentyquestions/engine
COPY apps/twentyquestions/schemas apps/twentyquestions/schemas
RUN uv sync --frozen --no-dev

# The built UI bundle the service serves from one origin.
COPY --from=ui /src/apps/twentyquestions/viewer/dist /app/ui

ENV PATH="/app/.venv/bin:$PATH"
ENV FREEAGENT_UI_DIR="/app/ui"
EXPOSE 8000

# Default command; the compose file overrides host/port for the network.
CMD ["free-agent", "serve", "--host", "0.0.0.0", "--port", "8000"]
