# Twenty Questions viewer

Placeholder for the Twenty Questions **viewer** — the TypeScript web GUI that
observes an episode live over NATS (and replays recorded episodes through the
same code path). It is the single-language sibling of [`../engine`](../engine),
which holds the Python application.

Nothing lives here yet. When the viewer is built (a later issue — see
[ADR-0001](../../../docs/decision-history/0001-gui-viewers-over-nats.md)), it
gets its own `package.json` and becomes a member of the root JavaScript
workspace, which globs `apps/*/viewer` (see `pnpm-workspace.yaml` at the repo
root). Keeping the TypeScript tree out of `engine/` is deliberate: it stops the
Python tooling (ruff, pytest, packaging) from globbing a foreign subtree.
