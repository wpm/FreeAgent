# Decisions

This directory holds **Architecture Decision Records (ADRs)** for Free Agent:
short documents that each capture one significant decision, with the context
that forced it, the choice made, the alternatives weighed, and the
consequences accepted.

An ADR is a point-in-time record, not living documentation. Once accepted, an
ADR is not rewritten when the world changes. A new ADR supersedes it, and the
old one is marked `Superseded`. The trail of records is the value: it tells a
future reader *why* the system is the way it is, including the roads not
taken.

For the canonical description of the practice, see Michael Nygard's
[Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
and [adr.github.io](https://adr.github.io/).

## Conventions

- One decision per file, named `NNNN-short-title.md` with a zero-padded
  sequence number. Numbers are never reused.
- Status is one of `Proposed`, `Accepted`, `Deprecated`, or `Superseded`.
- When a decision replaces an earlier one, set the old ADR's status to
  `Superseded` and link the two.
- Each record stands on its own: a reader who has seen no other document
  should be able to follow it. Links are for depth, not for meaning.

The numbering starts over here. Records from earlier attempts at the project
are not part of this repository's history.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-games-and-variants-are-modules-and-subcommands.md) | Games and their variants are modules and subcommands | Accepted |
| [0002](0002-actors-perceive-think-and-act.md) | Actors perceive, think, and act | Accepted |
| [0003](0003-the-model-player.md) | The model player | Accepted |
| [0004](0004-the-model-is-asked-to-select-before-a-game.md) | The model is asked to select before a game | Accepted |
| [0005](0005-actors-talk-by-sending-messages-alone.md) | Actors talk by sending messages alone | Accepted |
| [0006](0006-players-talk-by-day.md) | Players talk by day | Accepted |
| [0007](0007-a-tool-call-is-required-where-the-model-allows-it.md) | A tool call is required where the model allows it | Accepted |
| [0008](0008-werewolf-players-are-dealt-strategies.md) | Werewolf is one game whose players are dealt strategies | Accepted |
| [0009](0009-only-a-change-of-decision-restarts-the-ratify-timer.md) | Only a change of decision restarts the ratify timer, and a vote may be retracted | Accepted |
| [0010](0010-avalon-is-a-second-game-built-by-copying-werewolf.md) | Avalon is a second game, built by copying Werewolf | Proposed |
