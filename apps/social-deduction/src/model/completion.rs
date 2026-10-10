//! One decision asked of a model: the request `POST /v1/chat/completions`
//! takes and the response it answers with, covering only the fields used.
//!
//! A [`Request`] offers one tool and forces the model to call it, so the
//! model cannot answer with prose instead. A [`Response`] is read for the
//! tool calls it carries. Unknown fields in a response are ignored, so a
//! provider's additions do not break parsing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One request for a completion: the model asked, the messages it is
/// shown, the tools it may call, and the one it must.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Request {
    /// The model's id, exactly as the provider lists it.
    pub model: String,
    /// What the model is shown, in order.
    pub messages: Vec<Message>,
    /// The tools the model may call.
    pub tools: Vec<Tool>,
    /// The tool the model must call.
    pub tool_choice: ToolChoice,
}

impl Request {
    /// A request of `model` shown `messages` and made to call `tool`, the
    /// only one it is offered.
    pub fn new(model: impl Into<String>, messages: Vec<Message>, tool: Tool) -> Self {
        Self {
            model: model.into(),
            messages,
            tool_choice: ToolChoice::function(&tool.function.name),
            tools: vec![tool],
        }
    }

    /// The tool the request forces, among those it offers; none when it
    /// forces one it does not offer.
    pub fn forced(&self) -> Option<&Tool> {
        self.tools
            .iter()
            .find(|tool| tool.function.name == self.tool_choice.function.name)
    }
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

/// A tool the model may call: a function, in the protocol's words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    kind: Kind,
    /// The function the tool is.
    pub function: Function,
}

impl Tool {
    /// The tool `name`, described to the model by `description`, taking
    /// the arguments the JSON schema `parameters` describes.
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            kind: Kind::Function,
            function: Function {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// What a [`Tool`] does, as the model is told.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Function {
    /// Its name, which a call to it names.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// The JSON schema of its arguments.
    pub parameters: Value,
}

/// The one kind of tool there is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Function,
}

/// The tool the model must call.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolChoice {
    #[serde(rename = "type")]
    kind: Kind,
    /// Which one, by name.
    pub function: Named,
}

impl ToolChoice {
    /// The choice of the tool `name`.
    pub fn function(name: impl Into<String>) -> Self {
        Self {
            kind: Kind::Function,
            function: Named { name: name.into() },
        }
    }
}

/// A tool named in a [`ToolChoice`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Named {
    /// Its name.
    pub name: String,
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
        Self {
            choices: vec![Choice {
                message: Answer {
                    tool_calls: Some(vec![ToolCall {
                        function: Call {
                            name: name.into(),
                            arguments: arguments.to_string(),
                        },
                    }]),
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
    use serde_json::json;

    /// A tool selecting one of `candidates`, as a Werewolf player's is.
    fn select(candidates: &[&str]) -> Tool {
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

    #[test]
    fn a_request_serializes_to_the_protocol_s_json_with_the_tool_call_forced() {
        let request = Request::new(
            "qwen2.5-7b-instruct",
            vec![
                Message::system("You are player1."),
                Message::user("Night 1. Select."),
            ],
            select(&["player2", "player3"]),
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "model": "qwen2.5-7b-instruct",
                "messages": [
                    {"role": "system", "content": "You are player1."},
                    {"role": "user", "content": "Night 1. Select."},
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
                "tool_choice": {"type": "function", "function": {"name": "select"}},
            })
        );
    }

    #[test]
    fn the_forced_tool_is_the_one_offered() {
        let tool = select(&["player2"]);
        let request = Request::new("m", vec![], tool.clone());
        assert_eq!(request.forced(), Some(&tool));
    }

    #[test]
    fn a_response_with_tool_calls_parses_and_its_extra_fields_are_ignored() {
        let said = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1_700_000_000,
            "model": "qwen2.5-7b-instruct",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "logprobs": null,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "select", "arguments": "{\"target\": \"player2\"}"},
                    }],
                },
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        });
        let response: Response = serde_json::from_value(said).unwrap();
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
