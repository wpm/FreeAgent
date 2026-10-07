use anyhow::{Context, Result};
use std::fmt::Debug;
use tokio::sync::oneshot;

pub(super) trait Message: Debug {}

type RequestId = u64;

#[derive(Debug)]
pub(super) struct Request<M: Message> {
    id: RequestId,
    message: M,
    reply_to: oneshot::Sender<Reply<M>>,
}

impl<M: Message> Request<M> {
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
struct Reply<M: Message> {
    id: RequestId,
    messages: Vec<M>,
}
