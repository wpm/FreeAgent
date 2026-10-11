# ADR-0005: Actors talk by sending messages alone

**Status:** Accepted
**Date:** 2026-10-11
**Deciders:** Bill McNeill

## Context

The `free-agent` library gave an actor two ways to reach another. `send`
put a message in the recipient's mailbox and went on. `request` put a
message in the mailbox along with a channel, waited for the recipient's
`answer` step to run, and returned what it answered. What arrived in a
mailbox was an `Envelope`, either a `Statement` or a `Request`, and the
actor's message loop routed each to `receive` or to `answer`.

[ADR-0002](0002-actors-perceive-think-and-act.md) kept both: request and
reply as a fast, synchronous call into another actor's reflexes, and a
pair of correlated statements for any exchange that takes time. It built
the think loop so that a slow answer never holds up perceiving, and the
model-played Werewolf was built on announcements and selections, each a
statement carrying a sequence number. Only the uniform-random variant
still asked for each choice with a request, and
[ADR-0001](0001-games-and-variants-are-modules-and-subcommands.md) kept it
that way so that its game played exactly as it had in 0.1.0.

That left the library with two ways to say something and the application
with two protocols for one game. The uniform-random environment ran the
game as a loop inside `start`, gathering replies; the announcing
environment, shared by the scripted and model-played variants, was
event-driven. The scripted variant existed to run the announcing
environment with random players, which is what the uniform-random variant
would be if it spoke statements.

## Decision

**An actor reaches another only by `send`. The library has no request, no
reply, no `answer`, and no envelope: a mailbox carries the message type
itself, and a behavior handles everything in `receive`.**

An exchange that wants an answer is a pair of messages. The asker sends
one and goes on, the answer comes back later as a message of its own, and
a sequence number ties the two together, as ADR-0002 already had it for
slow exchanges. Nothing in the library waits on another actor.

The uniform-random variant plays the announcing environment. Its player
answers each announcement at once with a selection made uniformly at
random, sent as a message, which is what the scripted player did. The
scripted player and its actor enum become the uniform-random variant's,
the phase limits move to the `uniform-random` command line, and the
scripted variant goes, since it was this game under another name. The
`Observation` and `Action`
messages and the `Replied` log entry, which only the request protocol
used, are gone: every variant's log has `Announce` in `Sent` entries and
`Select` in `Received` ones.

## Consequences

- The library's API is smaller: `Context::send`, `Context::think`,
  `Context::think_after`, `Context::stop`, `Context::shutdown` and
  `Context::log` are the whole of what an actor does, and `Behavior` has
  `receive` and the life-cycle methods. There is one way to send a
  message.
- An environment cannot wait for a player. It opens a phase, takes
  selections as they come, and ends the phase at its limit if a player
  never selects, so a player with nobody to select, such as a seer who has
  discovered everyone alive, holds its phase open until the limit. The
  request protocol ended such a phase at once. A phase that opens with no
  one able to select could be ended as soon as it opens; that is left for
  when a game shows it matters.
- There are two variants, `uniform-random` and `llm`, on one environment.
  `social-deduction werewolf scripted` is gone, and `uniform-random` takes
  `--night-limit` and `--day-limit` in its place.
- ADR-0001 and ADR-0002 are amended: the uniform-random variant has
  changed, and request and reply did not stay.
