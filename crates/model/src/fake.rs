//! A model for tests to call: it answers from a function of the request
//! and records every request it is sent, so that what a player asks and
//! does with the answer is tested without a network. Nothing but tests
//! uses this module.

use crate::{Model, Request, Response};
use anyhow::anyhow;
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

/// How a [`FakeModel`] answers a request.
type Answering = dyn Fn(&Request) -> anyhow::Result<Response> + Send + Sync;

/// How long a [`FakeModel`] takes to answer a request.
type Taking = dyn Fn(&Request) -> Duration + Send + Sync;

/// A model that answers as it was built to and remembers what it was
/// asked.
pub struct FakeModel {
    answer: Box<Answering>,
    requests: Mutex<Vec<Request>>,
    /// How long it takes to answer each request.
    delay: Box<Taking>,
}

impl FakeModel {
    /// A model answering every request with `answer` of it, such as the
    /// app's canned player's, at once.
    pub fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
            requests: Mutex::new(Vec::new()),
            delay: Box::new(|_| Duration::ZERO),
        }
    }

    /// A model answering every request with `response`.
    pub fn responding(response: Response) -> Self {
        Self::answering(move |_| Ok(response.clone()))
    }

    /// A model failing every request with an error saying `message`.
    pub fn failing(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::answering(move |_| Err(anyhow!("{message}")))
    }

    /// This model, taking `delay` to answer each request, so that a call
    /// can be in progress while other things happen.
    pub fn taking(self, delay: Duration) -> Self {
        self.taking_by(move |_| delay)
    }

    /// This model, taking as long as `delay` says to answer each request.
    pub fn taking_by(self, delay: impl Fn(&Request) -> Duration + Send + Sync + 'static) -> Self {
        Self {
            delay: Box::new(delay),
            ..self
        }
    }

    /// Every request the model has been sent, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Model for FakeModel {
    async fn complete(&self, request: &Request) -> anyhow::Result<Response> {
        self.requests.lock().unwrap().push(request.clone());
        tokio::time::sleep((self.delay)(request)).await;
        (self.answer)(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use crate::tests::{chosen, selecting};
    use serde_json::json;
    use tokio::time::Instant;

    #[tokio::test]
    async fn the_fake_answers_from_its_function_and_records_every_request() {
        let model = FakeModel::answering(|request| {
            Ok(Response::tool_call(
                "select",
                &json!({"asked": request.messages[1].content}),
            ))
        });
        let first = selecting(&["Player 2"]);
        let mut second = selecting(&["Player 3"]);
        second.messages[1] = Message::user("Select again.");
        let answer = model.complete(&first).await.unwrap();
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"asked": "Select."}))
        );
        let answer = model.complete(&second).await.unwrap();
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"asked": "Select again."}))
        );
        assert_eq!(model.requests(), [first, second]);
    }

    #[tokio::test]
    async fn the_fake_responds_with_a_fixed_response_or_fails_with_a_message() {
        let fixed = Response::tool_call("select", &json!({"target": "Player 3"}));
        let model = FakeModel::responding(fixed.clone());
        assert_eq!(
            model.complete(&selecting(&["Player 2"])).await.unwrap(),
            fixed
        );
        assert_eq!(
            model.complete(&selecting(&["Player 4"])).await.unwrap(),
            fixed
        );
        let model = FakeModel::failing("the model is down");
        let error = model.complete(&selecting(&["Player 2"])).await.unwrap_err();
        assert_eq!(format!("{error:#}"), "the model is down");
        assert_eq!(model.requests().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_fake_takes_as_long_as_it_is_told_to_answer() {
        let model = FakeModel::answering(chosen).taking(Duration::from_secs(3));
        let asked = Instant::now();
        let answer = model.complete(&selecting(&["Player 2"])).await.unwrap();
        assert_eq!(asked.elapsed(), Duration::from_secs(3));
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"target": "Player 2"}))
        );
    }
}
