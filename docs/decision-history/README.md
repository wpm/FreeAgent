# Decisions

This directory holds **Architecture Decision Records (ADRs)** for FreeAgent — short
documents that each capture one significant decision: the context that forced it,
the choice made, the alternatives weighed, and the consequences accepted.

An ADR is a point-in-time record, not living documentation. Once accepted, an ADR
is not rewritten when the world changes; instead a new ADR supersedes it and the
old one is marked `Superseded`. The trail of records is the value — it tells a
future reader *why* the system is the way it is, including the roads not taken.

For the canonical description of the practice, see Michael Nygard's original
article, [Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions),
and the community hub at [adr.github.io](https://adr.github.io/).

## Conventions

- One decision per file, named `NNNN-short-title.md` with a zero-padded sequence
  number (`0001-...`, `0002-...`). Numbers are never reused.
- Status is one of `Proposed`, `Accepted`, `Deprecated`, or `Superseded`.
- When a decision replaces an earlier one, set the old ADR's status to
  `Superseded` and link the two.
- Keep them short. An ADR that needs many pages is usually several decisions.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-gui-viewers-over-nats.md) | GUI viewers over NATS with NATS-based playback | Accepted (partially superseded by 0003) |
| [0002](0002-control-plane-service.md) | A persistent control-plane service over a REST façade | Accepted (partially superseded by 0003) |
| [0003](0003-the-atemporal-episode.md) | The atemporal episode | Accepted (partially superseded by 0004) |
| [0004](0004-app-independent-service.md) | The app-independent episode service | Accepted (partially superseded by 0006) |
| [0005](0005-the-worker-pool.md) | The worker pool — distributed episode execution over a work queue | Accepted (partially superseded by 0008) |
| [0006](0006-entry-point-application-loading.md) | Entry-point application loading | Proposed |
| [0007](0007-control-plane-data-plane-split.md) | Control plane / data plane split | Proposed |
| [0008](0008-core-nats-before-jetstream.md) | Core NATS now, JetStream before training data | Proposed |
