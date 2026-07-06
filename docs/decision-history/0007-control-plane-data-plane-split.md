# ADR-0007: Control plane / data plane split

**Status:** Proposed
**Date:** 2026-07-02
**Deciders:** Bill McNeill

## Context

Every application has two halves: a Python *engine* built from the SDK and a
JavaScript *viewer*. The viewer speaks only REST to `freeagent-api`; the API
updates itself by listening to NATS, which acts as the (dynamic) backing data
behind the REST façade. Messages on the wire are JSON-encoded pydantic
`Message` subclasses carrying a `type` tag; the SDK's `Message._by_type`
registry is **open** — any process that imports a message class can decode it
(`__init_subclass__` registers it), and applications define their own message
types (Collatz's `Chain`, and, more rarely, `Command` subclasses for
app-specific out-of-domain management).

That openness poses a question with real architectural weight: does the API
*understand* application messages? If it decodes them into pydantic objects it
must import every observed application's message modules, inherit their
import-time side effects, and expose itself to bare-class-name collisions in
the registry when multiple apps are loaded in one process. It also makes the
API a third party to every engine↔viewer contract. The viewer, by contrast,
is written alongside its engine by the same author and already knows its app's
shapes — nothing *between* them needs to.

## Decision

**The API understands SDK-defined messages only (the control plane) and
relays application-defined messages as opaque JSON (the data plane). The
invariant: `freeagent-api` may only depend on names defined in
`freeagent-sdk`. An import from an application package inside the API is
leakage, by definition.**

- **Control plane.** `StartEntity`, `StopEntity`, `StopAgent`, `Ack` — the
  SDK vocabulary. From these alone the API derives everything it serves
  structurally: which episodes exist, which agents are alive, when an episode
  ended.
- **Data plane.** App-defined messages flow through unparsed: `json.loads`,
  index by subject, `type` tag, and arrival time, forward to the viewer over
  `application/episode_id/...` REST paths. The subject hierarchy plus the
  envelope tags *are* the API's entire data model.
- The API never computes on data-plane contents — no filtering, aggregation,
  or derived state over app payloads. The viewer derives its own view state
  from the message stream. If a genuine server-side-computation need appears,
  the remedy is an explicit optional API-side component on the `Application`
  protocol via a new ADR — not quiet imports.

### Envelope fields

The SDK reserves two fields on `Message`, both readable without app code:

- `type` — concrete class name, drives polymorphic decode (exists today).
- `protocol` — optional app-owned version tag. The SDK standardizes the
  *slot*, never the semantics: versioning policy is entirely the
  application's responsibility. The slot is reserved because its consumers
  are platform tools — archives and RL pipelines partitioning stored episodes
  by (application, protocol) without decoding payloads ([ADR-0008](0008-core-nats-before-jetstream.md)).

Two SDK adjustments follow from the split:

- `Message.model_validate_json` raises on unknown `type` — correct for
  entities, where an unknown type is an error, but the API meets unknown
  types *by design*. Add `Message.try_decode(...) -> Message | None` so
  "not a control-plane message" is a supported outcome, not an exception.
- `_by_type` collisions currently overwrite silently. Raise on duplicate
  registration so a bare-name collision is an import-time error, not silent
  mis-decoding. (The API's exposure disappears with this ADR — it imports no
  app code — but workers and tooling keep the guarantee.)

### Engine/viewer contract: Python is the source of truth

The viewer needs the data shapes but must not hand-maintain them. Message
shapes are generated, not shared:

pydantic `model_json_schema()` → one JSON Schema document per application →
`json-schema-to-typescript` in the viewer build. `Message._by_type` already
enumerates an app's vocabulary, so an SDK CLI (`freeagent schema <app>`)
resolves the app's entry point (importing registers everything) and emits the
schema. The `type` tag is emitted as a `const` per class so TypeScript can
narrow unions on it — `__init_subclass__` already rewrites `model_fields`, so
annotating the tag as `Literal[cls.__name__]` is in character.

Enforcement is co-versioning: engine and viewer live in the same app
directory and ship together, and CI regenerates the schema and fails on diff,
so a green build *means* in sync. The boundary where co-versioning stops
helping is time — messages that outlive the commit that produced them — which
is ADR-0008's territory.

## Options considered

### Option A: API imports application message modules and decodes natively

**Pros:** typed manipulation inside the API; REST responses are free
`model_dump()`s; validation at the boundary.
**Cons:** API must load code for every app it observes; import-time side
effects in a long-lived service; registry collisions across apps become the
API's problem; every engine↔viewer contract gains a third participant; the
road back to manifest-package complexity.

### Option B: Opaque pass-through with SDK-only decoding (chosen)

**Pros:** API never runs app code — the ADR-0006 loading story becomes
worker-only; collision exposure and import side effects vanish from the API;
the invariant is one sentence and mechanically lintable; viewer already
speaks the format.
**Cons:** the API cannot offer app-aware endpoints (current chain length,
game-specific summaries); viewers must derive state client-side from message
streams. Accepted until proven insufficient.

### Option C: Hybrid — apps may register API-side view components

**Pros:** server-side derived state where apps want it.
**Cons:** reintroduces app code in the API on day one, with all of Option A's
costs, for a need no application has demonstrated. Kept as the designated
escape hatch, adopted only via a future ADR.

## Consequences

- Easier: the API is application-count-invariant — a new app needs zero API
  changes, deploys, or restarts to be observable.
- Easier: `freeagent-api` can be built (rewrite step 3) before any thought
  about app schemas; its data model is subjects + envelope tags.
- Harder: viewers carry more logic (state derivation from streams).
- Harder: debugging via the API shows raw JSON for app messages; meaning
  requires the app's schema at hand.
- The subject-naming contract becomes load-bearing protocol for the whole
  stack — it deserves its SDK tests before anything is built on top.
- Revisit: if multiple viewers or non-viewer consumers need the same derived
  state, Option C fires.

## Action items

1. [ ] Add `protocol` field to `Message` (done on `rewrite`).
2. [ ] Add `Message.try_decode`; keep strict decoding for entities.
3. [ ] Raise on duplicate `_by_type` registration.
4. [ ] Emit `type` as `Literal`/`const` in schemas; add `freeagent schema`
       CLI.
5. [ ] Viewer build step + CI regenerate-and-diff check (first exercised by
       the Collatz viewer, rewrite step 4).
6. [ ] SDK tests pinning the subject-naming contract.
