use anyhow::{Context, Result};
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

/// A message is cloned for every recipient of a broadcast or a request, and
/// travels between actors on a multi-threaded runtime.
pub(super) trait Message: Debug + Clone + Send + Sync + 'static {}

type RequestId = u64;

/// Request ids are unique across the process, which is more than an episode
/// needs but costs nothing to arrange.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct Request<M: Message> {
    id: RequestId,
    message: M,
    reply_to: oneshot::Sender<Reply<M>>,
}

impl<M: Message> Request<M> {
    /// A request carrying `message`, and the channel its reply arrives on.
    pub(super) fn new(message: M) -> (Self, oneshot::Receiver<Reply<M>>) {
        let (reply_to, reply) = oneshot::channel();
        let request = Self {
            id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            message,
            reply_to,
        };
        (request, reply)
    }

    pub(super) fn reply(self, messages: Vec<M>) -> Result<()> {
        self.reply_to
            .send(Reply {
                id: self.id,
                messages,
            })
            .ok()
            .context("the asker stopped waiting")
    }

    pub(super) fn message(&self) -> &M {
        &self.message
    }
}

#[derive(Debug)]
pub(super) struct Reply<M: Message> {
    id: RequestId,
    pub(super) messages: Vec<M>,
}
