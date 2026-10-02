# Free Agent

Small agents that perceive, act, and talk to each other.

An **episode** runs a fixed roster of actors, each as a task of its own on the
[Tokio](https://tokio.rs) runtime, until they are all finished. Actors reach
one another through **channels**: everyone can message everyone, themselves
included, and the same shape carries shutdown signals. Everything they observe
goes to one **sink** the episode was given.

## Example

```rust
use anyhow::Result;
use free_agent::actor::{Actor, ActorId, async_trait};
use free_agent::episode::{self, Channels, Memory};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

// Actors exchange domain objects, not strings.
#[derive(Clone, Serialize, Deserialize)]
struct Greeting {
    from: ActorId,
}

struct Greeter {
    greet: ActorId,
}

#[async_trait]
impl Actor<Greeting> for Greeter {
    async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
        channels.send(Greeting { from: channels.id.clone() }, &self.greet)?;

        if let Some(greeting) = channels.inbox.recv().await {
            channels.log(&format!("heard from {}", greeting.from))?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let heard = Arc::new(Memory::new());
    episode::episode(
        [
            ("Alice", Box::new(Greeter { greet: "Bob".into() }) as Box<dyn Actor<Greeting>>),
            ("Bob", Box::new(Greeter { greet: "Alice".into() })),
        ],
        heard.clone(),
        Some(Duration::from_secs(5)),
    )
    .await?;

    assert_eq!(heard.entries().len(), 2);
    Ok(())
}
```

For a game built on this, see the `werewolf` crate under `apps/`.

## Concepts

### Actors

An actor is anything implementing [`Actor<M>`]. An actor that only needs to be
present implements nothing at all:

```rust,ignore
struct Bystander;

impl Actor<String> for Bystander {}
```

An actor runs as **one task**. Left to the default, that task wakes the actor
and then hands it its messages one at a time:

```rust,ignore
#[async_trait]
impl Actor<Question> for Oracle {
    async fn wake(&mut self, channels: &mut Channels<Question>) -> Result<Acting> {
        channels.request(question, &peer).await?;   // speak first, if you like
        Ok(Acting::Continue)
    }

    async fn act(&mut self, message: Question, channels: &mut Channels<Question>)
        -> Result<Acting>
    {
        let answer = slow_llm_call(&message).await;   // take as long as needed
        Ok(Acting::Done)
    }
}
```

`act` may take its time, so long as it waits by **awaiting**. While it waits on
an LLM call or a peer's answer the rest of the episode runs, and whatever is
sent to this actor queues in its inbox until it is ready for the next message.

What it must not do is block. Actors share the runtime's threads, so a
synchronous call that takes its time holds up actors that have nothing to do
with it, and cannot be cut off by the episode's timeout. Hand that kind of
work to `tokio::task::spawn_blocking` and await the handle.

Override `perceive` to take over the whole run instead, for control flow that
loop cannot express.

The methods are `async` and an episode holds its roster as `Box<dyn Actor<M>>`,
which is what `#[async_trait]` is for. It is re-exported from
`free_agent::actor`, and an impl that writes no methods does not need it.

An actor does not own its connections: the episode builds those once the whole
roster is known and lends each actor a [`Channels`] for the duration of its run.

### Messages

One episode carries one message type, chosen where [`episode`] is called and
checked at compile time. Any type serde can round-trip qualifies, and there is
nothing to implement:

```rust,ignore
#[derive(Clone, Serialize, Deserialize)]
struct Observation { saw: String }
```

The serde bounds are not needed to move a value between actors. They are the
standing promise that a message can also leave the process — be logged,
replayed, or sent to an actor running elsewhere — without this crate knowing
the domain.

### Asking a question

`send` is fire-and-forget. When an actor needs an answer, `request` asks and
waits for one:

```rust,ignore
let answer = channels.request(question, &peer).await?;
```

The question reaches the peer on its `requests` channel as an [`Envelope`],
which carries the means of answering. Only the request waits: the rest of the
episode runs on.

To ask several peers at once, join the requests:

```rust,ignore
let answers = join_all(
    wolves.iter().map(|wolf| channels.request(question.clone(), wolf)),
)
.await;

for (wolf, answer) in wolves.iter().zip(answers) {
    votes.insert(wolf.clone(), answer?);
}
```

Every question is in flight before any answer is waited on, so three peers
that take a second each cost a second rather than three. Each request carries
its own reply channel, which is all the correlation there is: an answer can
only come back to the request it answers.

A request is a future like any other, so giving up on one needs nothing
special: wrap it in `tokio::time::timeout`, or give several requests one
`timeout_at` deadline to share.

### Episodes and ending one

An episode is over when every actor has returned. An actor holds a sender to
its own inbox, so no inbox closes while its actor runs, and waiting for one to
close is waiting forever. An actor is expected to **finish** once its work is
done — by returning `Acting::Done`, or by returning from `perceive` — or to be
stopped by a peer:

```rust,ignore
channels.stop(&peer)?;
```

A stop is a request, not a kill: the actor hears it the next time it looks at
`channels.stop`, as the default loop does between messages. An actor may also
stop itself this way.

The `timeout` argument is the backstop for when neither happens. Actors still
running when it expires are cut off, and the episode fails.

A run that fails anywhere fails as a whole, and at once. An actor that returns
an error, one that panics, and a log that cannot be written all surface as the
episode's own error rather than being quietly absorbed, and the other actors
are not waited for.

### Logging

Actors log through `channels.log(&payload)`, and decide for themselves what is
worth recording. The episode supplies the rest: every entry is tagged with the
actor that made it and the time it was made.

A payload is kept as serialized structure rather than rendered text, so a sink
can write columns, frames, or rows and a reader is not left parsing prose.

| Sink | Use |
| --- | --- |
| [`Stderr`] | One JSON object per line, for watching a run |
| [`Memory`] | Keeps entries for a test to assert on |
| [`Discard`] | Records nothing |

Implement [`Sink`] for anywhere else — a file, a socket, a parquet writer.
`flush` is called as the episode ends, on the way out of a successful run and a
failed or timed-out one alike.

## Development

```sh
cargo test                        # unit tests and doctests
cargo doc --no-deps --lib --open  # the API docs
```

[`Actor`]: actor::Actor
[`Actor<M>`]: actor::Actor
[`Channels`]: episode::Channels
[`Sink`]: episode::Sink
[`Stderr`]: episode::Stderr
[`Memory`]: episode::Memory
[`Discard`]: episode::Discard
[`Envelope`]: episode::Envelope
[`episode`]: episode::episode
