//! What an actor is, and what it exchanges.
//!
//! An [`Actor`] is a piece of behavior with state of its own. It does not own
//! its connections to the world: [`episode`](crate::episode::episode) builds
//! those once the whole roster is known and lends each actor a [`Channels`]
//! for the duration of its run. That is why [`Actor::perceive`] takes
//! `&mut Channels<M>` rather than an actor holding them as fields — an actor
//! is constructed before it has any peers to talk to.

use crate::episode::Channels;
use anyhow::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Goes on any `impl Actor` that writes one of the trait's methods.
///
/// An episode holds its roster as `Box<dyn Actor<M>>`, and an `async fn`
/// cannot yet be called through a trait object. This is the usual bridge,
/// re-exported so that an actor's crate needs no dependency of its own to
/// name it.
pub use async_trait::async_trait;

/// What actors exchange.
///
/// Any type that serde can round-trip will do, so an episode carries domain
/// objects rather than strings. One episode carries one message type, chosen
/// where [`episode`](crate::episode::episode) is called and checked at
/// compile time.
///
/// The serde bounds are not needed to move a message between actors; they
/// are the standing promise that a message can also leave the process — be
/// logged, replayed, or sent to an actor running somewhere else — without
/// this crate knowing the domain.
///
/// There is nothing to implement: any qualifying type is a message already.
///
/// ```
/// use free_agent::actor::Message;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Serialize, Deserialize)]
/// struct Observation {
///     saw: String,
/// }
///
/// fn assert_is_a_message<M: Message>() {}
/// assert_is_a_message::<Observation>();
/// ```
pub trait Message: Serialize + DeserializeOwned + Send + 'static {}

impl<T: Serialize + DeserializeOwned + Send + 'static> Message for T {}

/// What an actor is called. Ids are chosen by whoever assembles the roster
/// and must be unique within an episode: they address messages, carry stop
/// signals, and tag log entries.
pub type ActorId = String;

/// An actor's behavior. Each actor owns its own state and runs as its own
/// task, so its methods take `&mut self`: nothing else touches it.
///
/// `Channels` is passed in rather than owned, because the channels only exist
/// once the whole roster is known. An actor is constructed before wiring and
/// learns how to reach its peers when its run begins.
///
/// # Examples
///
/// An actor that accepts the default perception loop needs nothing but the
/// empty impl:
///
/// ```
/// use free_agent::actor::Actor;
///
/// struct Bystander;
///
/// impl Actor<String> for Bystander {}
/// ```
///
/// One that does something says how under [`#[async_trait]`](macro@async_trait):
///
/// ```
/// use anyhow::Result;
/// use free_agent::actor::{Acting, Actor, async_trait};
/// use free_agent::episode::Channels;
///
/// struct Scribe;
///
/// #[async_trait]
/// impl Actor<String> for Scribe {
///     async fn act(&mut self, message: String, channels: &mut Channels<String>)
///         -> Result<Acting>
///     {
///         channels.log(&message)?;
///         Ok(Acting::Continue)
///     }
/// }
/// ```
#[async_trait]
pub trait Actor<M: Message>: Send + 'static {
    /// Does the actor's work, one message at a time.
    ///
    /// It may take as long as it likes, so long as it waits by awaiting: on
    /// an LLM call, a peer's answer, a timer. While it waits the rest of the
    /// episode runs, and anything sent to this actor queues in its inbox
    /// until it is ready for the next message.
    ///
    /// # Await, never block
    ///
    /// Actors share the runtime's threads. One that blocks its thread — a
    /// synchronous HTTP call, `std::thread::sleep`, a long computation —
    /// holds up actors that have nothing to do with it, and cannot be cut
    /// off by the episode's timeout. Hand that kind of work to
    /// [`tokio::task::spawn_blocking`] and await the handle.
    ///
    /// Waits that can overlap should: join several
    /// [`request`](Channels::request)s and a fan-out costs the slowest
    /// answer instead of the sum.
    ///
    /// A stop signal is looked for between messages, not during one. An
    /// `act` that should give up early when told to stop can select on
    /// [`Channels::stop`] itself.
    ///
    /// Returning [`Acting::Done`] ends the actor.
    ///
    /// # Errors
    ///
    /// Whatever the actor's own work returns. An error here ends this actor
    /// and fails the whole episode.
    async fn act(&mut self, message: M, channels: &mut Channels<M>) -> Result<Acting> {
        let _ = (message, channels);
        Ok(Acting::Continue)
    }

    /// Runs before any delivery arrives, for an actor that speaks first.
    ///
    /// It too may take its time, on the same terms as [`act`](Self::act).
    ///
    /// # Errors
    ///
    /// Whatever the actor's own work returns.
    async fn wake(&mut self, channels: &mut Channels<M>) -> Result<Acting> {
        let _ = channels;
        Ok(Acting::Continue)
    }

    /// The actor's whole run, from its first moment to its last.
    ///
    /// The default wakes the actor and then hands it its messages one at a
    /// time, until it says it is done or is told to stop. Override this only
    /// for control flow that loop cannot express — reading
    /// [`requests`](Channels::requests), or driving the episode rather than
    /// reacting to it — and then [`act`](Self::act) and [`wake`](Self::wake)
    /// are not called.
    ///
    /// # Errors
    ///
    /// Whatever the actor's own work returns. An error here ends this actor
    /// and fails the whole episode.
    async fn perceive(&mut self, channels: &mut Channels<M>) -> Result<()> {
        if self.wake(channels).await? == Acting::Done {
            return Ok(());
        }
        loop {
            tokio::select! {
                message = channels.inbox.recv() => match message {
                    Some(message) => {
                        if self.act(message, channels).await? == Acting::Done {
                            return Ok(());
                        }
                    }
                    // Every sender is gone; nothing more is coming.
                    None => return Ok(()),
                },
                () = channels.stop.cancelled() => return Ok(()),
            }
        }
    }
}

/// Whether an actor has more to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acting {
    /// Ready for the next delivery.
    Continue,
    /// Finished. The actor's run ends.
    Done,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs::File;
    use tempfile::NamedTempFile;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct Greeting {
        from: ActorId,
        salutation: String,
    }

    /// Writes a message to a file and reads it back. Generic over `M`, so it
    /// compiles only while the `Message` bound is strong enough to do this —
    /// that compile step is the real assertion. A concrete round-trip would
    /// still pass if the bound were loosened to `Send + 'static`.
    fn round_trip_through_a_file<M: Message>(message: &M) -> Result<M> {
        let file = NamedTempFile::new()?;
        serde_json::to_writer(&file, message)?;
        file.as_file().sync_all()?;
        Ok(serde_json::from_reader(File::open(file.path())?)?)
    }

    #[test]
    fn a_message_can_be_written_to_disk_and_read_back() {
        let greeting = Greeting {
            from: "Alice".to_string(),
            salutation: "hi".to_string(),
        };
        assert_eq!(round_trip_through_a_file(&greeting).unwrap(), greeting);
    }

    /// The requirement holds for any message type, not just this episode's.
    #[test]
    fn any_message_type_can_be_written_to_disk() {
        assert_eq!(round_trip_through_a_file(&42u64).unwrap(), 42);
        assert_eq!(
            round_trip_through_a_file(&vec!["a".to_string()]).unwrap(),
            vec!["a".to_string()]
        );
    }
}
