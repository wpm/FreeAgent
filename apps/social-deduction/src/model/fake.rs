//! A model for tests to call: it answers from a function of the request
//! and records every request it is sent, so that what a player asks and
//! does with the answer is tested without a network. Nothing but tests
//! uses this module.

use super::{Model, Request, Response};
use anyhow::anyhow;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Mutex;

/// How a [`FakeModel`] answers a request.
type Answering = dyn Fn(&Request) -> anyhow::Result<Response> + Send + Sync;

/// A model that answers as it was built to and remembers what it was
/// asked.
pub struct FakeModel {
    answer: Box<Answering>,
    requests: Mutex<Vec<Request>>,
}

impl FakeModel {
    /// A model answering every request with `answer` of it.
    pub fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
            requests: Mutex::new(Vec::new()),
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

    /// A model that calls the forced tool with the first value its first
    /// enumerated parameter allows, as a player's `select` tool asks for
    /// a `target` from an `enum` of candidates. A request with no such
    /// parameter is an error.
    pub fn choosing_first() -> Self {
        Self::answering(|request| {
            let tool = request
                .forced()
                .ok_or_else(|| anyhow!("the request forces no tool it offers"))?;
            let (parameter, first) = first_enumerated(&tool.function.parameters)
                .ok_or_else(|| anyhow!("{} has no enumerated parameter", tool.function.name))?;
            Ok(Response::tool_call(
                &tool.function.name,
                &json!({parameter: first}),
            ))
        })
    }

    /// Every request the model has been sent, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

/// The first property of the JSON schema `parameters` that is an `enum`,
/// and the first value it allows.
fn first_enumerated(parameters: &Value) -> Option<(&str, &Value)> {
    parameters
        .get("properties")?
        .as_object()?
        .iter()
        .find_map(|(name, schema)| {
            let first = schema.get("enum")?.as_array()?.first()?;
            Some((name.as_str(), first))
        })
}

#[async_trait]
impl Model for FakeModel {
    async fn complete(&self, request: &Request) -> anyhow::Result<Response> {
        self.requests.lock().unwrap().push(request.clone());
        (self.answer)(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Message, Tool};

    /// A request to select one of `candidates`, as a Werewolf player's is.
    fn selecting(candidates: &[&str]) -> Request {
        Request::new(
            "m",
            vec![
                Message::system("You are player1."),
                Message::user("Select."),
            ],
            Tool::new(
                "select",
                "Select one.",
                json!({
                    "type": "object",
                    "properties": {"target": {"type": "string", "enum": candidates}},
                    "required": ["target"],
                }),
            ),
        )
    }

    #[tokio::test]
    async fn the_fake_answers_from_its_function_and_records_every_request() {
        let model = FakeModel::answering(|request| {
            Ok(Response::tool_call(
                "select",
                &json!({"asked": request.messages[1].content}),
            ))
        });
        let first = selecting(&["player2"]);
        let mut second = selecting(&["player3"]);
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
        let fixed = Response::tool_call("select", &json!({"target": "player3"}));
        let model = FakeModel::responding(fixed.clone());
        assert_eq!(
            model.complete(&selecting(&["player2"])).await.unwrap(),
            fixed
        );
        assert_eq!(
            model.complete(&selecting(&["player4"])).await.unwrap(),
            fixed
        );
        let model = FakeModel::failing("the model is down");
        let error = model.complete(&selecting(&["player2"])).await.unwrap_err();
        assert_eq!(format!("{error:#}"), "the model is down");
        assert_eq!(model.requests().len(), 1);
    }

    #[tokio::test]
    async fn the_fake_choosing_first_calls_the_forced_tool_with_the_first_enumerated_value() {
        let model = FakeModel::choosing_first();
        let answer = model
            .complete(&selecting(&["player3", "player2"]))
            .await
            .unwrap();
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"target": "player3"}))
        );
    }

    #[tokio::test]
    async fn the_fake_choosing_first_fails_a_request_with_nothing_to_choose_from() {
        let model = FakeModel::choosing_first();
        let error = model.complete(&selecting(&[])).await.unwrap_err();
        assert!(format!("{error:#}").contains("select"), "{error:#}");
        let mut request = selecting(&["player2"]);
        request.tools[0].function.parameters = json!({"type": "object", "properties": {}});
        assert!(model.complete(&request).await.is_err());
    }
}
