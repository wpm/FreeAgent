#!/usr/bin/env bash
# Bring up the two-service FreeAgent network (NATS + episode service + UI) and
# open the Twenty Questions UI in a browser -- the single command to watch a game
# end to end (ADR-0003). Stop it with Ctrl-C, or `docker compose -f
# docker/compose.yml down`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
port="${FREEAGENT_PORT:-8000}"
url="http://localhost:${port}"

# Build and start the network in the background.
docker compose -f "${here}/compose.yml" up --build -d

# Wait for the service to answer, then open the UI.
echo "waiting for the episode service at ${url} ..."
for _ in $(seq 1 60); do
  if curl -sf "${url}/freeagent/episodes" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo "Twenty Questions UI: ${url}"
if command -v open >/dev/null 2>&1; then
  open "${url}"            # macOS
elif command -v xdg-open >/dev/null 2>&1; then
  xdg-open "${url}"        # Linux desktop
fi

echo "Following logs (Ctrl-C to detach; the network keeps running):"
docker compose -f "${here}/compose.yml" logs -f
