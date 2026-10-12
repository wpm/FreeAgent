//! A model for tests to call: it answers from a function of the request
//! and records every request it is sent, so that what a player asks and
//! does with the answer is tested without a network. Nothing but tests
//! uses this module.

use super::{Message, Model, Request, Response, Tool, ToolChoice};
use anyhow::anyhow;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

/// How a [`FakeModel`] answers a request.
type Answering = dyn Fn(&Request) -> anyhow::Result<Response> + Send + Sync;

/// A model that answers as it was built to and remembers what it was
/// asked.
pub struct FakeModel {
    answer: Box<Answering>,
    requests: Mutex<Vec<Request>>,
    /// How long it takes to answer.
    delay: Duration,
}

impl FakeModel {
    /// A model answering every request with `answer` of it, such as
    /// [`choosing_first`], at once.
    pub fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
            requests: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
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
        Self { delay, ..self }
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
        tokio::time::sleep(self.delay).await;
        (self.answer)(request)
    }
}

/// What the canned player says when it speaks.
pub const SAID: &str = "I have nothing to add.";

/// The answer the canned player gives `request`: the calls [`canned_calls`]
/// makes of the request as JSON, as a response. A request the canned player
/// cannot answer is an error.
pub fn choosing_first(request: &Request) -> anyhow::Result<Response> {
    let calls = canned_calls(&serde_json::to_value(request)?)?;
    Ok(Response::calling(&calls))
}

/// The calls the canned player makes to answer `request`, a completion
/// request in the protocol's JSON, each as a tool's name and its arguments.
/// Offered one tool, it calls that tool with the first value its enumerated
/// parameter allows, as a player's `select` tool asks for a `target` from
/// an `enum` of candidates. Offered several, as a player is by day, it says
/// [`SAID`] with the tool taking a string and selects the first candidate,
/// the first time it is asked in a phase, and asks to wait as long as the
/// tool taking a bounded whole number allows after that. It knows it has
/// been asked before from the story the request tells: the phase underway,
/// which follows the story's last blank line, has it voting. A tool without
/// the parameter the canned player needs is an error naming it.
pub fn canned_calls(request: &Value) -> anyhow::Result<Vec<(String, Value)>> {
    let tools: Vec<&Value> = request["tools"]
        .as_array()
        .ok_or_else(|| anyhow!("the request offers no tools"))?
        .iter()
        .map(|tool| &tool["function"])
        .collect();
    let select = tools
        .iter()
        .find_map(|tool| first_enumerated(&tool["parameters"]).map(|found| (*tool, found)));
    if let [only] = tools.as_slice() {
        let (_, (parameter, first)) =
            select.ok_or_else(|| anyhow!("{} has no enumerated parameter", only["name"]))?;
        return Ok(vec![(name(only), json!({parameter: first}))]);
    }
    let asked = request["messages"]
        .as_array()
        .and_then(|messages| messages.last()?["content"].as_str())
        .unwrap_or_default();
    if voted_in_this_phase(asked) {
        let (tool, (parameter, most)) = tools
            .iter()
            .find_map(|tool| first_bounded(&tool["parameters"]).map(|found| (*tool, found)))
            .ok_or_else(|| anyhow!("no tool takes a bounded whole number"))?;
        return Ok(vec![(name(tool), json!({parameter: most}))]);
    }
    let (speak, text) = tools
        .iter()
        .find_map(|tool| first_string(&tool["parameters"]).map(|found| (*tool, found)))
        .ok_or_else(|| anyhow!("no tool takes a string"))?;
    let (select, (parameter, first)) =
        select.ok_or_else(|| anyhow!("no tool has an enumerated parameter"))?;
    Ok(vec![
        (name(speak), json!({text: SAID})),
        (name(select), json!({parameter: first})),
    ])
}

/// Whether the story a request's user message `asked` tells has the player
/// voting in the phase underway, which follows the story's last blank line.
pub fn voted_in_this_phase(asked: &str) -> bool {
    asked
        .rsplit("\n\n")
        .nth(1)
        .is_some_and(|phase| phase.contains("You vote"))
}

/// The name of `function`, a tool in the protocol's JSON.
fn name(function: &Value) -> String {
    function["name"].as_str().unwrap_or_default().to_string()
}

/// The properties of the JSON schema `parameters`, in order.
fn properties(parameters: &Value) -> impl Iterator<Item = (&str, &Value)> {
    parameters
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| (name.as_str(), schema))
        })
}

/// The first property of the JSON schema `parameters` that is an `enum`,
/// and the first value it allows.
pub fn first_enumerated(parameters: &Value) -> Option<(&str, &Value)> {
    properties(parameters).find_map(|(name, schema)| {
        let first = schema.get("enum")?.as_array()?.first()?;
        Some((name, first))
    })
}

/// The first property of the JSON schema `parameters` that is a string
/// without an `enum`.
fn first_string(parameters: &Value) -> Option<&str> {
    properties(parameters)
        .find(|(_, schema)| schema["type"] == "string" && schema.get("enum").is_none())
        .map(|(name, _)| name)
}

/// The first property of the JSON schema `parameters` with a `maximum`, and
/// that maximum.
fn first_bounded(parameters: &Value) -> Option<(&str, &Value)> {
    properties(parameters).find_map(|(name, schema)| Some((name, schema.get("maximum")?)))
}

/// A request to select one of `candidates`, as a Werewolf player's is:
/// its one tool, `select`, takes a `target` from an `enum` of them.
pub fn selecting(candidates: &[&str]) -> Request {
    Request::offering(
        "qwen2.5-7b-instruct",
        vec![
            Message::system("You are player1."),
            Message::user("Select."),
        ],
        vec![select_tool(candidates)],
        ToolChoice::Required,
    )
}

/// The tool `select`, taking a `target` from an `enum` of `candidates`.
fn select_tool(candidates: &[&str]) -> Tool {
    Tool::new(
        "select",
        "Knowing everything you know, select the best one.",
        json!({
            "type": "object",
            "properties": {"target": {"type": "string", "enum": candidates}},
            "required": ["target"],
        }),
    )
}

/// A request to speak, vote or wait, as a Werewolf player's is by day: the
/// tools `speak`, taking `text`; `select`, taking a `target` from an `enum`
/// of `candidates`; and `wait`, taking `seconds` up to `limit`. The story
/// it tells is `story`, before the situation.
pub fn by_day(candidates: &[&str], limit: u64, story: &str) -> Request {
    let tools = vec![
        Tool::new(
            "speak",
            "Say something.",
            json!({
                "type": "object",
                "properties": {"text": {"type": "string", "minLength": 1}},
                "required": ["text"],
            }),
        ),
        select_tool(candidates),
        Tool::new(
            "wait",
            "Say nothing now.",
            json!({
                "type": "object",
                "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": limit}},
                "required": ["seconds"],
            }),
        ),
    ];
    Request::offering(
        "qwen2.5-7b-instruct",
        vec![
            Message::system("You are player1."),
            Message::user(format!("{story}\n\nIt is day 1. Speak, vote, or wait.")),
        ],
        tools,
        ToolChoice::Required,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

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

    #[tokio::test(start_paused = true)]
    async fn the_fake_takes_as_long_as_it_is_told_to_answer() {
        let model = FakeModel::answering(choosing_first).taking(Duration::from_secs(3));
        let asked = Instant::now();
        let answer = model.complete(&selecting(&["player2"])).await.unwrap();
        assert_eq!(asked.elapsed(), Duration::from_secs(3));
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"target": "player2"}))
        );
    }

    #[tokio::test]
    async fn choosing_first_calls_the_one_tool_offered_with_the_first_enumerated_value() {
        let model = FakeModel::answering(choosing_first);
        let answer = model
            .complete(&selecting(&["player3", "player2"]))
            .await
            .unwrap();
        assert_eq!(
            answer,
            Response::tool_call("select", &json!({"target": "player3"}))
        );
    }

    #[test]
    fn choosing_first_fails_a_request_with_nothing_to_choose_from() {
        let error = choosing_first(&selecting(&[])).unwrap_err();
        assert!(format!("{error:#}").contains("select"), "{error:#}");
        let mut request = selecting(&["player2"]);
        request.tools[0].parameters = json!({"type": "object", "properties": {}});
        assert!(choosing_first(&request).is_err());
    }

    #[test]
    fn by_day_the_canned_player_speaks_and_votes_once_then_waits_out_the_day() {
        // Woken for the first time in the day, it speaks and votes.
        let request = by_day(
            &["player2", "player3"],
            60,
            "Day 1.\nplayer2 votes against you.",
        );
        assert_eq!(
            choosing_first(&request).unwrap(),
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "player2"})),
            ])
        );
        // Woken again in a day it has voted in, it waits as long as it may.
        let story = "Day 1.\nYou say, \"I have nothing to add.\"\nYou vote against player2.";
        let request = by_day(&["player2", "player3"], 60, story);
        assert_eq!(
            choosing_first(&request).unwrap(),
            Response::tool_call("wait", &json!({"seconds": 60}))
        );
        // A vote in an earlier phase does not count as this phase's.
        let story = "Day 1.\nYou vote against player2.\nplayer2 dies.\n\nDay 2.";
        let request = by_day(&["player3"], 60, story);
        assert_eq!(choosing_first(&request).unwrap().tool_calls().len(), 2);
    }

    #[test]
    fn the_canned_player_fails_a_day_s_request_missing_a_tool_it_needs() {
        let mut request = by_day(&["player2"], 60, "Day 1.");
        request.tools.remove(0);
        let error = choosing_first(&request).unwrap_err();
        assert!(format!("{error:#}").contains("string"), "{error:#}");
        let mut request = by_day(&["player2"], 60, "Day 1.");
        request.tools.remove(1);
        let error = choosing_first(&request).unwrap_err();
        assert!(format!("{error:#}").contains("enumerated"), "{error:#}");
        let mut request = by_day(&["player2"], 60, "Day 1.\nYou vote against player2.");
        request.tools.remove(2);
        let error = choosing_first(&request).unwrap_err();
        assert!(format!("{error:#}").contains("bounded"), "{error:#}");
        // A request with no tools at all, and one with no messages, in the
        // protocol's JSON.
        let error = canned_calls(&json!({"model": "m"})).unwrap_err();
        assert!(format!("{error:#}").contains("no tools"), "{error:#}");
        let request = by_day(&["player2"], 60, "Day 1.");
        let mut json = serde_json::to_value(&request).unwrap();
        json["messages"] = json!([]);
        json["tools"][1]["function"]["name"] = Value::Null;
        let calls = canned_calls(&json).unwrap();
        assert_eq!(calls[1], (String::new(), json!({"target": "player2"})));
    }
}
