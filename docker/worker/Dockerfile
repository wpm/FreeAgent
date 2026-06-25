# The `worker` image: the fat episode worker -- the FreeAgent library PLUS the
# application engines (ADR-0005). Where the slim `freeagent` service image runs
# no app code (ADR-0004 evicted the engines from it), the worker is the engines'
# home: it pulls Manifest messages off the shared JetStream work queue and forks
# each as a child process, so it must be able to import every engine class a
# manifest references (e.g. `twentyquestions`'s Host/Player roles).
#
# It publishes no port and serves no UI: a long-lived, app-agnostic supervisor
# that connects to the in-network NATS (`FREEAGENT_NATS_URL`) and blocks on its
# pull loop. Scale it with `docker compose up --scale worker=N`.

# syntax=docker/dockerfile:1

FROM python:3.12-slim AS worker
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/
WORKDIR /app

# Install the `twentyquestions` app engine and its `freeagent` dependency from
# the uv workspace, exactly as pinned in uv.lock. `--package twentyquestions`
# (not the workspace root) pulls the engine and the `freeagent` LIBRARY but NOT
# `freeagent[service]`: the web stack stays gated behind the `service` extra
# (ADR-0005), so the worker -- which forks episode children and serves no HTTP --
# omits FastAPI/uvicorn. The engine `src` IS copied (unlike the service image,
# which copies only the engine manifest) so the roles a manifest names import.
COPY pyproject.toml uv.lock README.md ./
COPY packages packages
COPY apps/twentyquestions/engine apps/twentyquestions/engine
RUN uv sync --frozen --no-dev --package twentyquestions

ENV PATH="/app/.venv/bin:$PATH"

# No EXPOSE: the worker publishes nothing. It pulls work from NATS and idles on
# an empty queue. The compose file supplies FREEAGENT_NATS_URL for the network.
CMD ["free-agent", "work"]
