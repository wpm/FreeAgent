use anyhow::{Context, Result};
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

/// A message is cloned for every recipient of a broadcast or a request, and
/// travels between actors on a multi-threaded runtime.
pub trait Message: Debug + Clone + Send + Sync + 'static {}

type RequestId = u64;

/// Request ids are unique across the process, which is more than an episode
/// needs but costs nothing to arrange.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

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

    pub(crate) fn reply(self, messages: Vec<M>) -> Result<()> {
        self.reply_to
            .send(Reply {
                id: self.id,
                messages,
            })
            .ok()
            .context("the asker stopped waiting")
    }

    pub(crate) fn message(&self) -> &M {
        &self.message
    }
}

#[derive(Debug)]
pub(crate) struct Reply<M: Message> {
    /// Kept so a reply can be matched to its request once anything checks.
    #[allow(dead_code)]
    id: RequestId,
    pub(crate) messages: Vec<M>,
}
