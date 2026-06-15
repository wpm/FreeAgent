# Twenty Questions viewer

The Twenty Questions **viewer** — the TypeScript web GUI that observes an
episode live over NATS (and replays recorded episodes through the same code
path). It is the single-language sibling of [`../engine`](../engine), which
holds the Python application, and a member of the root JavaScript workspace
(`pnpm-workspace.yaml` globs `apps/*/viewer`). Keeping the TypeScript tree out
of `engine/` is deliberate: it stops the Python tooling (ruff, pytest,
packaging) from globbing a foreign subtree.

The GUI itself is a later issue (see
[ADR-0001](../../../docs/decision-history/0001-gui-viewers-over-nats.md),
action items 2 and 5). What exists today is the **message-schema contract**: the
generated TypeScript types the viewer will build on.

## Generated message types

`src/generated/` holds TypeScript types generated from the FreeAgent JSON
Schema — the framework envelope (`packages/freeagent/schemas/`) and this app's
wire payloads (`../schemas/`). They are committed, but **do not edit them by
hand**: they are regenerated from the Pydantic models that are the single
source of truth (see the repo-root README, "Message schemas").

```sh
# From the repo root — regenerates JSON Schema from Pydantic, then types:
pnpm run schemas

# Or just the TypeScript half, from this directory:
pnpm run generate
```

`src/messages.ts` builds the viewer-facing message types on the generated
output (e.g. narrowing an `Envelope` to the Host's `GameOver` signal). It is
also the contract's compile test — `pnpm run typecheck` (or `tsc --noEmit`)
fails if the generated types drift or go missing.
