//! One decision asked of a model: the request `POST /v1/chat/completions`
//! takes and the response it answers with, covering only the fields used.
//!
//! A [`Request`] offers one tool and forces the model to call it, so the
//! model cannot answer with prose instead. A [`Response`] is read for the
//! tool calls it carries. Unknown fields in a response are ignored, so a
//! provider's additions do not break parsing.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

/// One request for a completion: the model asked, the messages it is
/// shown, and the one tool it is offered and must call.
///
/// It serializes as the protocol has it: the tool in `tools`, and a
/// `tool_choice` naming it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The model's id, exactly as the provider lists it.
    pub model: String,
    /// What the model is shown, in order.
    pub messages: Vec<Message>,
    /// The tool the model must call.
    pub tool: Tool,
}

impl Request {
    /// A request of `model` shown `messages` and made to call `tool`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>, tool: Tool) -> Self {
        Self {
            model: model.into(),
            messages,
            tool,
        }
    }
}

impl Serialize for Request {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Named<'a> {
            name: &'a str,
        }
        let mut request = serializer.serialize_struct("Request", 4)?;
        request.serialize_field("model", &self.model)?;
        request.serialize_field("messages", &self.messages)?;
        request.serialize_field("tools", &[&self.tool])?;
        request.serialize_field(
            "tool_choice",
            &Function::new(Named {
                name: &self.tool.name,
            }),
        )?;
        request.end()
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
/// …}`: a [`Tool`] offered, when `function` describes one, or a
/// `tool_choice`, when it only names one.
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
                "tool_choice": {"type": "function", "function": {"name": "select"}},
            })
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
