# Security

## Reporting a vulnerability

Please report suspected vulnerabilities privately to the maintainer rather than
opening a public issue.

## Security posture

Free Agent is a research testbed designed to run on a single trusted machine or
network. It deliberately ships **without authentication or authorization**:

- The NATS server accepts any connection that can reach it.
- The REST API carries no credentials and answers any origin (CORS is wide
  open), and creating an episode spawns a local worker subprocess.
- Viewers are static pages that talk to the API unauthenticated.

The defaults keep everything on localhost (the API binds `127.0.0.1`, and the
compose file publishes NATS only to the local host). **Do not expose the API or
the NATS server beyond a trusted network without adding your own
authentication in front of them.**
