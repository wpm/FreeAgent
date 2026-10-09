//! What actors say to one another: the [`Message`] trait and the request
//! and reply envelopes that carry a message and its answer.

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

/// What one episode's actors say to one another.
///
/// An episode carries one message type, and every actor in it sends and
/// receives that type. A message is cloned for every recipient of a
/// statement or a request, travels between actors on a multi-threaded
/// runtime, and is written to the log and read back from it, which is what
/// the bounds say. Implementing it takes one empty
/// line:
///
/// ```
/// # use free_agent::Message;
/// # use serde::{Deserialize, Serialize};
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Note(String);
/// impl Message for Note {}
/// ```
pub trait Message: Debug + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {}

/// Tells a reply from the others arriving on the same channel.
type RequestId = u64;

/// Request ids are unique across the process.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

/// A message whose sender is waiting for an answer, and the channel the
/// answer goes back on.
#[derive(Debug)]
pub(crate) struct Request<M: Message> {
    id: RequestId,
    message: M,
    reply_to: oneshot::Sender<Reply<M>>,
}

impl<M: Message> Request<M> {
    /// A request carrying `message`, and the channel its reply arrives on.
    pub(crate) fn new(message: M) -> (Self, oneshot::Receiver<Reply<M>>) {
        let (reply_to, reply) = oneshot::channel();
        let request = Self {
            id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            message,
            reply_to,
        };
        (request, reply)
    }

    /// Answer this request with `messages`, consuming it: a request is
    /// answered once.
    ///
    /// # Errors
    ///
    /// Fails if the asker has stopped waiting.
    pub(crate) fn reply(self, messages: Vec<M>) -> Result<()> {
        self.reply_to
            .send(Reply {
                id: self.id,
                messages,
            })
            .ok()
            .context("the asker stopped waiting")
    }

    /// What is being asked.
    pub(crate) fn message(&self) -> &M {
        &self.message
    }
}

/// The answer to a [`Request`].
#[derive(Debug)]
pub(crate) struct Reply<M: Message> {
    /// The request this answers.
    #[allow(dead_code)]
    id: RequestId,
    /// What the recipient answered. An empty answer is an acknowledgment.
    pub(crate) messages: Vec<M>,
}
