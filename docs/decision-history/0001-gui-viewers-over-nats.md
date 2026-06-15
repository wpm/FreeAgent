# ADR-0001: GUI viewers over NATS with NATS-based playback

**Status:** Accepted
**Date:** 2026-06-14
**Deciders:** Bill McNeill
**Amended:** 2026-06-14 — clarified that the replayer is a single, app-agnostic,
library-level tool (the Parquet log is uniform across applications, so it need not
live under any one app's CLI), and that recording is library infrastructure spawned
per run rather than a separate recorder application.

## Context

FreeAgent applications run real-time, non-turn-taking multi-agent episodes whose
every message lands on NATS (JetStream), and a per-run recorder — library
infrastructure spawned as its own process, not a separate application — drains that
stream to a single Parquet file per episode. "The wire is the log": `stream_seq` gives the
authoritative total order, and the Parquet log is the same artifact later consumed
by replay tooling and RL training-data generators.

We want **GUI viewer applications** that listen in on a running episode and render
it — for Twenty Questions, a chat-room-style live transcript. Requirements:

- They are GUI applications.
- They are custom per FreeAgent application.
- They communicate with the environment **entirely over NATS**.
- They can replay completed episodes from logs.

Allowing a human to drive the agents through a viewer is explicitly a *future*,
non-goal for now.

Two forces shape the decision. First, the NATS-only constraint fully decouples
viewers from the library: a viewer shares nothing with FreeAgent except the NATS
subjects and message schemas, so its implementation language is free to be chosen
for GUI fit rather than to match the Python core. Second, "replay from logs" is in
apparent tension with "communicate entirely over NATS" — reading a Parquet file is
not NATS — and how we resolve that tension determines whether viewers carry one
code path or two.

## Decision

Build **one TypeScript web viewer per application**, running in the browser and
subscribing to that application's NATS subjects over websockets (`nats.ws`). The
NATS server gains a websocket listener. Each viewer is, for now, bespoke and
hardcoded to its application's subjects and message types.

Resolve replay as **NATS playback, not log reading**. A standalone *replayer*
reads an episode's Parquet log and re-publishes its messages — in `stream_seq`
order, with original or scaled inter-message timing — onto a **separate, local**
NATS server using byte-identical subjects. The viewer only ever subscribes to
NATS and cannot tell live from replay; it has exactly one code path. Pause, seek,
and speed controls live in the replayer, where they are reusable beyond GUIs.

The replayer is **app-agnostic**: the Parquet log is uniform across every
application (each row is a `subject`, a `payload`, and a `stream_seq`), so a single
library-level replay command replays any app's episode rather than living under one
application's CLI.

Treat the **NATS subject layout and message schemas as the contract**. Schemas are
single-sourced from the Python Pydantic models via exported JSON Schema, from which
TypeScript types are generated — so the cross-language seam introduces no
hand-maintained duplication.

Keep viewer source **in the existing monorepo**, co-located with the application it
serves, rather than in a separate UI repo or a repo per viewer. The binding force
between a viewer and the rest of the system is the schema contract, which is the
most volatile thing early on; a monorepo lets a schema change and its viewer
consequences land in one atomic commit, whereas a separate repo turns every such
change into a publish-and-version cycle and invites version skew on exactly that
seam. The mixed-language cost is bounded and front-loaded (stand up a TS toolchain
once); a Python contributor who never touches a viewer never needs node. Migration
to a separate repo later is cheap (`git filter-repo`), so the rule is to pick what
is best now — and now, with the schema in flux, atomicity wins.

Within each application, split the two halves into self-contained, single-language
siblings: `apps/<app>/engine/` (the Python application — environment, roster,
prompts, CLI, with its own `pyproject.toml`) and `apps/<app>/viewer/` (the
TypeScript GUI, with its own `package.json`). The split is driven by build
isolation: keeping `viewer/` out of the Python packaging root stops ruff, pytest,
and packaging tooling from globbing a TS subtree. `engine` is preferred over
`runner`, which collides with the library's own "launcher" and undersells a
directory that holds the substance of the application. A single root JS workspace
(pnpm or npm workspaces) globbing `apps/*/viewer` keeps one lockfile and toolchain
despite co-location. The generated JSON Schema is checked into the repo as a
portable artifact, so a future repo split inherits a clean contract.

Deliberately **defer** three things until evidence accumulates: a shared viewer
SDK, a per-application declarative protocol/config format, and any native desktop
packaging. Write two or three bespoke viewers first, then factor out what they
genuinely share.

## Options Considered

### Option A: TypeScript web viewer over NATS websockets (chosen)

| Dimension | Assessment |
|-----------|------------|
| Complexity | Medium — adds a JS toolchain and websocket listener |
| Cost | Low — static bundle, zero install, shareable by URL |
| Scalability | High — deepest ecosystem for chat/timeline/scrubber UIs |
| Team familiarity | Medium — not the Python core, but mainstream |

**Pros:** Best-in-class UI ecosystem for the exact shapes needed (chat rooms,
timelines, scrubbing); zero-install and trivially shareable; `nats.ws` is
first-class so the browser is literally "entirely over NATS"; keeps a native
desktop wrap (e.g. Tauri) open later with no rewrite.
**Cons:** Second language alongside the Python core; requires generating TS types
from the schema to avoid drift; needs websocket enabled on NATS.

### Option B: Python GUI viewer (Flet / PyQt / Textual)

| Dimension | Assessment |
|-----------|------------|
| Complexity | Low–Medium — single language with the core |
| Cost | Low |
| Scalability | Medium — weaker ecosystem for rich chat/timeline UIs |
| Team familiarity | High — matches the core and app authors |

**Pros:** One language end to end; direct reuse of Pydantic models with no codegen;
lowest toolchain overhead for Python-first contributors.
**Cons:** Weaker UI ecosystem for the target look; native toolkits (PyQt/Flet) are
heavier to distribute; the NATS-only contract means the language-match benefit is
smaller than it appears — reuse is the only real gain, and JSON Schema codegen
recovers most of that.

### Option C: Native desktop (Rust + Tauri)

| Dimension | Assessment |
|-----------|------------|
| Complexity | High — separate stack and build |
| Cost | Medium–High |
| Scalability | High UI quality, low contributor reach |
| Team familiarity | Low across likely app authors |

**Pros:** Excellent desktop integration and performance; aligns with a preferred
personal stack.
**Cons:** Smallest contributor pool for a Python-adopted framework; heaviest build;
premature given no native requirement. Notably, Option A does not foreclose this —
the same web frontend can be wrapped in Tauri later.

### Replay sub-decision: NATS playback vs. log-reading in the viewer

Teaching the viewer to read Parquet would give it two code paths (live subscribe
vs. file read) and split rendering logic. Re-publishing the log onto a local NATS
keeps the viewer pure-subscriber, makes live and replay byte-identical at the wire,
and yields reusable transport-controls. Chosen accordingly.

## Trade-off Analysis

The decisive trade is **UI fit and reach vs. language uniformity**. Because the
NATS-only boundary already decouples the viewer from the core, the main argument
for a Python viewer (Option B) collapses to schema reuse — and exporting JSON
Schema from Pydantic to generate TS types recovers that cheaply. That tips the
balance to the web stack, which is materially stronger for the chat-room and
timeline UIs these viewers need, while staying zero-install and shareable.

On replay, the cost of a separate local `nats-server` (a single binary) is trivial
next to the benefit of a single viewer code path and identical subjects across
modes.

On the SDK and protocol-config questions, a declarative config format is itself a
generalization — designing it now would be the same a-priori-framework mistake we
want to avoid for the SDK. Better to let both emerge from real viewers (rule of
three). The one thing worth getting roughly right *early* is the wire contract
(subjects + schema versioning), because live, replayer, viewers, and training/replay
consumers all depend on it and it is the expensive thing to change later.

Security is treated as a non-concern: this is a trusted research testbed where all
agents are assumed to play by the rules. Viewers are read-only by convention (they
have no reason to publish), but we do not invest in scoped credentials or hardened
auth at this stage.

## Consequences

- **Easier:** rich live transcripts and timelines; sharing a viewer (URL + log);
  one viewer code path for live and replay; building new per-app viewers quickly;
  atomic cross-language schema changes (model + generated types + viewer in one
  commit); later native packaging via Tauri without a rewrite.
- **Harder / new work:** maintain a TypeScript toolchain; enable a NATS websocket
  listener; build and run the replayer and a local replay NATS server; generate TS
  types from exported JSON Schema and keep that generation wired into the build.
- **Watch / revisit:** per-app viewers risk reimplementing connection, reconnect,
  replay controls, and chat widgets — revisit for an SDK after the second or third
  viewer. Revisit the wire contract early if subject/schema conventions feel wrong,
  since it is costly to change once consumers multiply. Revisit read-only-by-
  convention and auth if/when human-in-the-loop interaction becomes a goal.

## Action Items

1. [ ] Restructure `apps/twentyquestions/` into `engine/` (Python, own
       `pyproject.toml`) and `viewer/` (TS, own `package.json`) siblings, and add a
       root JS workspace globbing `apps/*/viewer`.
2. [ ] Enable a websocket listener on the NATS server (`docker/nats` config).
3. [ ] Add JSON Schema export from the Pydantic message models and a TS type
       generation step; check the generated schema into the repo.
4. [ ] Build the **app-agnostic** replayer as a library-level command: read any
       app's episode Parquet log and re-publish in `stream_seq` order with timing
       controls onto a separate local NATS server.
5. [ ] Build the first bespoke viewer — the Twenty Questions chat-room transcript —
       against live and replay.
6. [ ] After two or three viewers exist, review them for a shared viewer SDK and a
       per-application protocol/config format (deferred; do not design up front).
