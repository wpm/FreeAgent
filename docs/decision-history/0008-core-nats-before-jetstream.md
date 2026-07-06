# ADR-0008: Core NATS now, JetStream before training data

**Status:** Proposed
**Date:** 2026-07-02
**Deciders:** Bill McNeill

## Context

The previous incarnation ran on JetStream: per-episode streams, a work-queue
stream ([ADR-0003](0003-the-atemporal-episode.md),
[ADR-0005](0005-the-worker-pool.md)). It also went fast, ended up with
something that didn't work, and was hard to debug. The rewrite is deliberately
bottom-up — SDK, then a trivial application (Collatz), then worker, then API,
then viewer — with each step tested before the next, and it wants the fewest
moving parts per step. JetStream is a large moving part: streams, consumers,
acks, retention policies, all present in every test and every debugging
session.

Core NATS is at-most-once, fire-and-forget. Under the architecture of
[ADR-0007](0007-control-plane-data-plane-split.md), the API's view of the
world is whatever messages it happened to be subscribed for: start it after an
episode begins, or restart it mid-episode, and its state is silently wrong.

One goal makes this more than a UI-freshness concern: **Free Agent exists to
generate reinforcement-learning training data.** An episode record missing
messages is not degraded training data; it is *corrupt* training data with no
marker saying so.

## Decision

**The rewrite runs on core NATS. All communication is request/reply; the SDK
provides no persistence. Durable delivery (JetStream or equivalent) plus an
episode-integrity mechanism are prerequisites for treating any episode as
training data — and nothing produced during the core-NATS phase is ever
consumed downstream as data.**

### What core NATS must still provide: synchronization

Losing persistence does not mean losing checkpoints. Every send is a NATS
request awaiting a reply, and the reply is an **acknowledgment of receipt,
not a carrier of work**: an entity replies `Ack` promptly, processes
asynchronously (the agent queue), and delivers results as its own
counter-request. Two consequences:

- Request timeouts stay short and become a diagnostic: a request that times
  out means a counterpart is doing work inside a handler — a bug with a
  pointer to it. `Entity.request` must take an explicit configurable timeout
  rather than inheriting nats-py's silent 0.5 s default.
- The environment always knows its commands *arrived*, which is the
  synchronization that makes episodes debuggable without a persistent log.

### The API's memory is a cache, not a record

Under core NATS the API's in-memory state is best-effort by construction.
This is accepted, with the line drawn hard: the REST API's view must never
feed an archive. When archives arrive, they are fed by the delivery-guaranteed
layer (JetStream consumers or entity-side logging), never by what the API
happened to see. If mid-episode API restarts need repair before then, the
remedy is control-plane resync (entities answer state queries over
request/reply), not persistence bolted onto the API.

### Preconditions for training data

Before any episode is archived and consumed:

1. **Durable delivery** — JetStream per-episode streams or equivalent, so a
   record is complete-by-construction rather than complete-by-luck.
2. **Integrity markers** — a definite end-of-episode marker at minimum;
   preferably a closing manifest (per-entity message counts) so an incomplete
   record is *detectable* rather than trusted.
3. **Provenance** — records partitioned by (application, `protocol`) using
   the envelope alone (ADR-0007), since archived messages outlive the
   engine+viewer commit that co-versioning keeps honest.

## Options considered

### Option A: Keep JetStream from day one

**Pros:** durable from the start; no migration later; matches the end state.
**Cons:** maximum moving parts during exactly the phase whose lesson was
"going fast produced something undebuggable"; every SDK and Collatz test
drags stream/consumer setup; persistence semantics entangled with protocol
semantics while both are in flux.

### Option B: Core NATS with request/reply synchronization (chosen)

**Pros:** smallest possible surface under the SDK tests and the Collatz
episode; Acks give per-step causality without a log; a plain `nats-server`
subprocess is the whole test dependency.
**Cons:** lossy API state; nothing archivable; a real migration later
(subjects and message model are designed to carry over; stream configuration
is additive).

### Option C: Entity-side logging instead of JetStream later

Entities append their own traffic to local storage; archives assembled from
per-entity logs.

**Pros:** no broker persistence; logs travel with the entity.
**Cons:** N partial logs needing assembly and reconciliation versus one
ordered stream; reinvents what JetStream provides. Not chosen now; noted as
the fallback if JetStream's operational cost returns as a problem.

## Consequences

- Easier: rewrite steps 1–4 test and debug against a bare `nats-server`
  (session-scoped subprocess fixture; per-test `episode_root` isolation).
- Easier: persistence semantics arrive later as their own decision, onto a
  protocol already stabilized and tested.
- Harder: any demo episode is unrecoverable after the fact; there is no
  replay until the JetStream ADR lands.
- Bound accepted: the core-NATS phase is scaffolding. Its episodes are for
  development only, never data.
- Revisit trigger: the earlier of (a) the API needing durable state in
  practice, or (b) the first intent to archive an episode for training. The
  successor ADR covers stream layout, integrity markers, and replay.

## Action items

1. [ ] Make `Entity.request` timeout an explicit parameter.
2. [ ] Establish the ack-then-counter-request pattern in the Collatz episode
       (rewrite steps 1–2) as the canonical request/reply shape.
3. [ ] End-of-episode marker in the control-plane vocabulary early — cheap
       now, required later.
4. [ ] Write the JetStream successor ADR when a revisit trigger fires.
