# Twenty Questions

The FreeAgent sample application, split into two self-contained,
single-language siblings (see
[ADR-0001](../../docs/decision-history/0001-gui-viewers-over-nats.md)):

- **[`engine/`](engine)** — the Python application: environment, roster,
  prompts, and the `twenty-questions` CLI, with its own `pyproject.toml`. This
  is the substance of the app; start here. Its [README](engine/README.md)
  explains the agents and how to run an episode.
- **[`viewer/`](viewer)** — the TypeScript web GUI that observes an episode
  over NATS, with its own `package.json` (a placeholder for now).

The two halves are kept in separate directories so the Python and TypeScript
toolchains do not collide: the uv workspace globs `apps/*/engine` and the root
JavaScript workspace globs `apps/*/viewer`. Their binding contract is the NATS
subject layout and message schema, so co-locating them lets a schema change and
its viewer update land in one atomic commit.
