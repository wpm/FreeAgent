# Free Agent

[![CI](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml?query=branch%3Amain)
[![codecov](https://codecov.io/gh/wpm/FreeAgent/graph/badge.svg)](https://codecov.io/gh/wpm/FreeAgent)

Episodes in which actors talk to each other by request and reply.

An **episode** is the world in which everything here takes place. It brings a
set of actors into being together, decides which of them can reach which, and
runs them on the [Tokio](https://tokio.rs) runtime until every one of them has
shut itself down or a time limit passes. Nothing exists before the episode
begins and nothing survives its end.

Within an episode, an **actor** is a [`Policy`] driven by an inbox. Actors
communicate one way only: one asks a set of others a question through
[`Context::request`] and awaits their answers, and each of the others answers
through [`Policy::reply`].

The vocabulary comes from reinforcement learning. A typical episode has one
environment actor and several agent actors. The agents are reactive: they
answer whatever they are asked. The environment makes the opening move in
[`Policy::start`], asks the agents what they want to do, and decides when the
episode is over.

## Example

An environment asks an agent for a move, hears "north", and ends the episode.

```rust
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{episode, ActorId, Context, Ending, Policy, Reply};
use std::collections::HashSet;
use std::time::Duration;

/// Asks the agent what it wants to do, then leaves.
struct Environment;

#[async_trait]
impl Policy for Environment {
    type Message = String;

    async fn start(&mut self, context: &Context<String>) -> Result<()> {
        let agent = HashSet::from([Some("agent".to_string())]);
        let replies = context.request(&agent, "what next?".to_string(), None).await?;
        assert_eq!(replies, vec![("agent".to_string(), Reply::Message("north".to_string()))]);
        context.shutdown();
        Ok(())
    }

    async fn reply(
        &mut self,
        _from: ActorId,
        _message: String,
        _context: &Context<String>,
    ) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Always goes north, and leaves once it has been asked.
struct Agent;

#[async_trait]
impl Policy for Agent {
    type Message = String;

    async fn reply(
        &mut self,
        _from: ActorId,
        _message: String,
        context: &Context<String>,
    ) -> Result<Option<String>> {
        context.shutdown();
        Ok(Some("north".to_string()))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let actors: [(ActorId, Box<dyn Policy<Message = String> + Send>); 2] = [
        ("environment".to_string(), Box::new(Environment)),
        ("agent".to_string(), Box::new(Agent)),
    ];
    let ending = episode(actors, None, Duration::from_secs(5), None).await?;
    assert_eq!(ending, Ending::Finished);
    Ok(())
}
```

For a game built on this, see the `werewolf` crate under `apps/`.

## Concepts

### Policies

A policy is what an actor does with the requests it receives. It implements
[`Policy`], putting `#[async_trait]` on the impl block and writing the methods
as `async fn`:

```rust,ignore
#[async_trait]
impl Policy for Oracle {
    type Message = Question;

    async fn start(&mut self, context: &Context<Question>) -> Result<()> {
        // The opening move, before anything has been heard. Reactive
        // policies leave this out.
        Ok(())
    }

    async fn reply(
        &mut self,
        from: ActorId,
        question: Question,
        context: &Context<Question>,
    ) -> Result<Option<Question>> {
        let answer = slow_model_call(&question).await?; // take as long as needed
        Ok(Some(answer))                                // or None to acknowledge
    }
}
```

Every method receives a [`Context`], the part of the actor a step is allowed
to touch: its name, a way to ask the other actors things, and a way to shut
itself down.

Each actor takes one step at a time. A step may take its time, so long as it
waits by awaiting: while it waits on a model or on a peer's answer, the rest
of the episode runs, and whatever is sent to the actor queues in its inbox.
What a step must not do is block the thread.

Policies of different types share an episode by boxing them as
`Box<dyn Policy<Message = M> + Send>`, as the example does.

### Messages

One episode carries one message type, chosen where [`episode`] is called and
checked at compile time. Any plain data type with the serde derives qualifies,
and there is nothing to implement:

```rust,ignore
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Question { about: String }
```

The serde bounds are not needed to move a value between actors. They are the
standing promise that what actors say can also leave the process: be logged,
replayed, or carried to an actor running elsewhere.

### Requests and replies

A request asks every recipient the same thing and awaits one [`Reply`] per
recipient:

```rust,ignore
let replies = context.request(&recipients, question, Some(patience)).await?;
for (who, reply) in replies {
    match reply {
        Reply::Message(answer) => ...,  // they said something
        Reply::Acknowledge => ...,      // they handled it without a word
        Reply::Unanswered => ...,       // patience ran out, or they had gone
    }
}
```

The request goes out as soon as it is made; awaiting the result waits for the
answers, bounded by the patience if one is given. Every question is in flight
before any answer is waited on, so three recipients that take a second each
cost a second rather than three.

Dropping the future sends the request and forgets about the answers. That is
how an actor leaves a note for itself: a recipient of `None` means the asking
actor, and such a request lands in its own inbox to be handled in a later
step. Awaiting a request to oneself can never succeed, since the actor cannot
take a step while it is waiting for one to finish.

### Topology

By default everybody can talk to everybody. A [`Topology`] restricts that: it
maps each actor to the set of actors it may address, links are one-way, and an
actor absent from the map can reach no one but itself. A request to an actor
the topology does not allow is an error, and nothing is sent to anyone.

### Ending an episode

An actor shuts itself down by calling `shutdown` on its context. The step that
does so still completes, and a reply it returns is still delivered. The
episode ends with [`Ending::Finished`] once every actor has done this.

The time limit is the backstop for when they do not. When it passes, every
actor still running is stopped, even in the middle of a step, and the episode
ends with [`Ending::TimedOut`].

A run that fails anywhere fails as a whole. An error returned from any step
stops that actor, the rest are shut down, and the episode's error names every
actor that failed and why.

### Recording

A [`Log`] handed to [`episode`] receives a copy of everything said on the
wire as an [`Event`]: every request as its asker sends it, every reply as its
recipient sends it, and every request that went unanswered, each stamped with
the instant it was recorded and tied to its question by a request id. The
record is complete and uninterpreted, fit for driving a display or for
training. Nothing waits on the log's receiver, so a slow consumer costs memory
rather than time.

```rust,ignore
let (log, mut events) = Log::new();
let ending = episode(actors, None, time_limit, Some(log)).await?;
while let Ok(event) = events.try_recv() {
    println!("{}", serde_json::to_string(&event)?);
}
```

### Deadlock

Each actor takes one step at a time, and a step that is awaiting answers is
the actor's current step. So an actor waiting on a request answers no one
meanwhile, and two actors that await each other at the same moment are stuck
until one of them runs out of patience. An environment asking its agents never
forms such a cycle. Agents asking each other should always give a patience.

## Development

```sh
cargo test                        # unit tests and doctests
cargo doc --no-deps --lib --open  # the API docs
```

[`Policy`]: Policy
[`Policy::start`]: Policy::start
[`Policy::reply`]: Policy::reply
[`Context`]: Context
[`Context::request`]: Context::request
[`Reply`]: Reply
[`Topology`]: Topology
[`Ending::Finished`]: Ending::Finished
[`Ending::TimedOut`]: Ending::TimedOut
[`Log`]: Log
[`Event`]: Event
[`episode`]: episode
