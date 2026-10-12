//! One decision asked of a model: the request `POST /v1/chat/completions`
//! takes and the response it answers with, covering only the fields used.
//!
//! A [`Request`] offers one tool or several and, by its [`ToolChoice`],
//! either forces the model to call one, so the model cannot answer with
//! prose instead, or allows it to, for a model that refuses to be forced.
//! A [`Response`] is
//! read for the tool calls it carries. Unknown fields in a response are
//! ignored, so a provider's additions do not break parsing.

use super::MAX_TOKENS;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

/// One request for a completion: the model asked, the messages it is
/// shown, the tools it is offered, whether it must call one of them, and
/// how long its reply may be.
///
/// It serializes as the protocol has it: the tools in `tools`, and the
/// `tool_choice`, `"required"`, which forces a call to one of them, or
/// `"auto"`, which leaves the model free to answer in prose instead.
/// Whether the model may call several at once is left to the provider's
/// default, which allows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Request {
    /// The model's id, exactly as the provider lists it.
    pub model: String,
    /// What the model is shown, in order.
    pub messages: Vec<Message>,
    /// The tools the model may call.
    pub tools: Vec<Tool>,
    /// Whether the model must call one of the tools.
    pub tool_choice: ToolChoice,
    /// The most tokens the reply may take.
    pub max_tokens: u64,
}

impl Request {
    /// A request of `model` shown `messages` and offered `tools`, under
    /// `tool_choice`, its reply taking at most [`MAX_TOKENS`].
    pub fn offering(
        model: impl Into<String>,
        messages: Vec<Message>,
        tools: Vec<Tool>,
        tool_choice: ToolChoice,
    ) -> Self {
        Self {
            model: model.into(),
            messages,
            tools,
            tool_choice,
            max_tokens: MAX_TOKENS,
        }
    }
}

/// Whether a request makes the model call a tool, written as the
/// protocol's `tool_choice` writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    /// The model must call one of the tools offered.
    Required,
    /// The model may call one of the tools offered, or answer in prose,
    /// for a model that refuses to be made to call one.
    Auto,
}

/// One message the model is shown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    /// Who it is from.
    pub role: Role,
    /// Its text.
    pub content: String,
}

impl Message {
    /// A system message: what the model is before anything is asked.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    /// A user message: what is asked.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

/// Who a [`Message`] is from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The one setting the model up.
    System,
    /// The one asking.
    User,
}

/// A tool the model may call. It serializes as the protocol has it: a
/// function with these fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    /// Its name, which a call to it names.
    pub name: String,
    /// What it is for, as the model is told.
    pub description: String,
    /// The JSON schema of its arguments.
    pub parameters: Value,
}

impl Tool {
    /// The tool `name`, described to the model by `description`, taking
    /// the arguments the JSON schema `parameters` describes.
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

impl Serialize for Tool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Described<'a> {
            name: &'a str,
            description: &'a str,
            parameters: &'a Value,
        }
        Function::new(Described {
            name: &self.name,
            description: &self.description,
            parameters: &self.parameters,
        })
        .serialize(serializer)
    }
}

/// A function in the protocol's shape, `{"type": "function", "function":
/// …}`: a [`Tool`] offered, which `function` describes.
#[derive(Serialize)]
struct Function<F> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: F,
}

impl<F> Function<F> {
    fn new(function: F) -> Self {
        Self {
            kind: "function",
            function,
        }
    }
}

/// What a model answered: its choices, of which the first is read.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Response {
    /// The model's answers. A provider sends one unless asked for more.
    pub choices: Vec<Choice>,
}

impl Response {
    /// A response of one choice making one call to the tool `name` with
    /// `arguments`, for tests to answer with.
    pub fn tool_call(name: impl Into<String>, arguments: &Value) -> Self {
        Self::calling(&[(name.into(), arguments.clone())])
    }

    /// A response of one choice making each of `calls`, a tool's name and
    /// its arguments, in order, for tests to answer with.
    pub fn calling(calls: &[(String, Value)]) -> Self {
        let tool_calls = calls
            .iter()
            .map(|(name, arguments)| ToolCall {
                function: Call {
                    name: name.clone(),
                    arguments: arguments.to_string(),
                },
            })
            .collect();
        Self {
            choices: vec![Choice {
                message: Answer {
                    tool_calls: Some(tool_calls),
                },
            }],
        }
    }

    /// The tool calls the first choice makes; none when there is no
    /// choice or it made none.
    pub fn tool_calls(&self) -> &[ToolCall] {
        self.choices
            .first()
            .and_then(|choice| choice.message.tool_calls.as_deref())
            .unwrap_or_default()
    }
}

/// One answer in a [`Response`].
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Choice {
    /// What the model said.
    pub message: Answer,
}

/// What the model said in a [`Choice`]: only its tool calls are read.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Answer {
    /// The tools it called, which a provider leaves out or sends as null
    /// when it called none.
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// One call the model made.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ToolCall {
    /// The function called and what it was given.
    pub function: Call,
}

/// A function called by name, with its arguments as the protocol sends
/// them: a JSON object in a string.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Call {
    /// The name of the tool called.
    pub name: String,
    /// The arguments, as a JSON string.
    pub arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned::completion;
    use crate::model::fake::selecting;
    use serde_json::json;

    #[test]
    fn a_request_serializes_to_the_protocol_s_json_with_the_tool_call_forced() {
        let request = selecting(&["player2", "player3"]);
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "model": "qwen2.5-7b-instruct",
                "messages": [
                    {"role": "system", "content": "You are player1."},
                    {"role": "user", "content": "Select."},
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "select",
                        "description": "Knowing everything you know, select the best one.",
                        "parameters": {
                            "type": "object",
                            "properties": {"target": {"type": "string", "enum": ["player2", "player3"]}},
                            "required": ["target"],
                        },
                    },
                }],
                "tool_choice": "required",
                "max_tokens": 1024,
            })
        );
    }

    #[test]
    fn a_request_offering_several_tools_lists_them_in_order_and_still_forces_a_call() {
        let speak = Tool::new("speak", "Say something.", json!({"type": "object"}));
        let wait = Tool::new("wait", "Say nothing.", json!({"type": "object"}));
        let request = Request::offering(
            "qwen2.5-7b-instruct",
            vec![Message::user("Speak or wait.")],
            vec![speak.clone(), wait.clone()],
            ToolChoice::Required,
        );
        assert_eq!(request.tools, [speak, wait]);
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["tools"][0]["function"]["name"], "speak");
        assert_eq!(json["tools"][1]["function"]["name"], "wait");
        assert_eq!(json["tool_choice"], "required");
        // One tool is a list of one.
        let one = serde_json::to_value(selecting(&["player2"])).unwrap();
        assert_eq!(one["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_request_that_allows_a_tool_call_serializes_its_tool_choice_as_auto() {
        let request = Request {
            tool_choice: ToolChoice::Auto,
            ..selecting(&["player2"])
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_tool_choice_is_written_as_the_protocol_writes_it() {
        for (choice, written) in [
            (ToolChoice::Required, "required"),
            (ToolChoice::Auto, "auto"),
        ] {
            assert_eq!(serde_json::to_value(choice).unwrap(), written);
            assert_eq!(
                serde_json::from_value::<ToolChoice>(json!(written)).unwrap(),
                choice
            );
        }
    }

    #[test]
    fn a_response_may_call_several_tools_in_order() {
        let response = Response::calling(&[
            ("speak".to_string(), json!({"text": "Hello."})),
            ("select".to_string(), json!({"target": "player2"})),
        ]);
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "speak");
        assert_eq!(calls[0].function.arguments, r#"{"text":"Hello."}"#);
        assert_eq!(calls[1].function.name, "select");
        assert_eq!(
            Response::tool_call("select", &json!({"target": "player2"})),
            Response::calling(&[("select".to_string(), json!({"target": "player2"}))])
        );
    }

    #[test]
    fn a_response_with_tool_calls_parses_and_its_extra_fields_are_ignored() {
        let said = completion("select", &json!({"target": "player2"}));
        let response: Response = serde_json::from_str(&said).unwrap();
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "select");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].function.arguments).unwrap(),
            json!({"target": "player2"})
        );
    }

    #[test]
    fn a_response_whose_message_has_no_tool_calls_or_null_ones_has_none() {
        for message in [
            json!({"role": "assistant", "content": "I pick player2."}),
            json!({"role": "assistant", "content": "I pick player2.", "tool_calls": null}),
        ] {
            let said = json!({"choices": [{"message": message}]});
            let response: Response = serde_json::from_value(said).unwrap();
            assert!(response.tool_calls().is_empty(), "{response:?}");
        }
    }

    #[test]
    fn a_response_with_no_choices_parses_and_has_no_tool_calls() {
        let response: Response = serde_json::from_value(json!({"choices": []})).unwrap();
        assert!(response.choices.is_empty());
        assert!(response.tool_calls().is_empty());
    }
}
