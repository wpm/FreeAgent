//! What Twenty Questions brings to [`free_agent_model`], through which it
//! reaches its models: the shape of every request, the tools the host and
//! the players share and how their calls are read, the [`Asker`] each actor
//! asks its model through, and the probe, [`plays`], which asks a model to
//! play as a player, as the host answering a guess, and as the host picking
//! an object. For tests, the canned player and the canned providers that
//! play as it does.
//!
//! Every request makes the model call a tool, unless the probe finds the
//! model refuses that, and then every request asks it to call one, and an
//! answer in prose is a failure like any other.

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod canned;

use crate::host;
use crate::log::{Ending, Entry};
use crate::message::Message;
use crate::player;
use anyhow::{Context as _, bail};
use free_agent::Context;
use free_agent_model::check::{first_answered, probed};
use free_agent_model::spec::Probed;
use free_agent_model::{Model, Request, Response, Tool, ToolChoice};
use humantime::format_duration;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The game, as a probe's error names it.
pub const GAME: &str = "Twenty Questions";

/// What every request ends with, under either tool choice: that a tool
/// call is the one answer that counts.
pub(crate) const ANSWER: &str = "Answer by calling one of the tools.";

/// The name of the tool an actor speaks with.
pub(crate) const SPEAK: &str = "speak";

/// The name of the tool an actor waits with.
pub(crate) const WAIT: &str = "wait";

/// A request of an actor's shape to `model_id` under `tool_choice`:
/// `prompt` as the system message, `asked` and then [`ANSWER`] as the user
/// message, and `tools` to answer with. This is the one way an actor builds
/// a request and the one way the probe does, so they agree on what is
/// asked.
pub(crate) fn request(
    model_id: &str,
    tool_choice: ToolChoice,
    prompt: &str,
    mut asked: String,
    tools: Vec<Tool>,
) -> Request {
    asked.push(' ');
    asked.push_str(ANSWER);
    Request::offering(
        model_id,
        vec![
            free_agent_model::Message::system(prompt),
            free_agent_model::Message::user(asked),
        ],
        tools,
        tool_choice,
    )
}

/// The JSON schema of a tool taking one argument, `name`, a string that is
/// not empty.
pub(crate) fn text_parameter(name: &str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {name: {"type": "string", "minLength": 1}},
        "required": [name],
        "additionalProperties": false,
    })
}

/// The tool an actor speaks with: `speak`, described to the model by
/// `description`, taking `text`.
pub(crate) fn speak(description: &str) -> Tool {
    Tool::new(SPEAK, description, text_parameter("text"))
}

/// The tool a player waits with: `wait`, taking `seconds`, a whole number
/// from 1 to `limit` in whole seconds.
pub(crate) fn wait(limit: Duration) -> Tool {
    Tool::new(
        WAIT,
        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
        json!({
            "type": "object",
            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": limit.as_secs()}},
            "required": ["seconds"],
            "additionalProperties": false,
        }),
    )
}

/// The arguments of a call taking `text`.
#[derive(Deserialize)]
struct Text {
    text: String,
}

/// The arguments of a [`wait`] call.
#[derive(Deserialize)]
struct Waiting {
    seconds: u64,
}

/// What `call`, a call to a tool taking `text`, says. Arguments that do
/// not parse, or text that is empty once trimmed, is an error.
pub(crate) fn said(call: &free_agent_model::Call) -> anyhow::Result<String> {
    let Text { text } = arguments(call)?;
    if text.trim().is_empty() {
        bail!("the model's {} call says nothing", call.name);
    }
    Ok(text)
}

/// How long `call`, a call to [`wait`] in a game that lasts `limit`,
/// waits. Arguments that do not parse, or seconds not between 1 and
/// `limit` in whole seconds, is an error.
pub(crate) fn waited(call: &free_agent_model::Call, limit: Duration) -> anyhow::Result<Duration> {
    let Waiting { seconds } = arguments(call)?;
    if !(1..=limit.as_secs()).contains(&seconds) {
        bail!(
            "the model asked to wait {seconds}s, which is not between 1s and {}",
            format_duration(limit)
        );
    }
    Ok(Duration::from_secs(seconds))
}

/// The arguments of `call`, parsed. A provider may send no arguments as no
/// text at all, which parses as `{}`. Arguments that do not parse are an
/// error naming the tool and showing them.
pub(crate) fn arguments<T: DeserializeOwned>(call: &free_agent_model::Call) -> anyhow::Result<T> {
    let text = match call.arguments.trim() {
        "" => "{}",
        _ => &call.arguments,
    };
    serde_json::from_str(text).with_context(|| {
        format!(
            "the arguments of the model's {} call: {}",
            call.name, call.arguments
        )
    })
}

/// The calls `response` makes, of which there must be at least one.
pub(crate) fn calls(response: &Response) -> anyhow::Result<Vec<&free_agent_model::Call>> {
    let made: Vec<_> = response
        .tool_calls()
        .iter()
        .map(|call| &call.function)
        .collect();
    if made.is_empty() {
        bail!("the model made no tool call");
    }
    Ok(made)
}

/// That the model called `tool`, which it was not offered.
pub(crate) fn not_offered(tool: &str) -> anyhow::Error {
    anyhow::anyhow!("the model called {tool}, which is not a tool it was offered")
}

/// What the model was shown for a call: how many memories the story was
/// told from, and how many lines of talk it forgot to fit the context
/// window, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Shown {
    pub(crate) seen: usize,
    pub(crate) forgot: Option<usize>,
}

/// How an actor asks its model: the model's id, whether each request
/// makes it call a tool, the context window each request fits, and the
/// handle it is asked through.
#[derive(Clone)]
pub struct Asker {
    /// The model's id, as each request names it.
    pub model_id: String,
    /// Whether each request makes the model call a tool.
    pub tool_choice: ToolChoice,
    /// The model's context window, in tokens.
    pub context_window: u64,
    /// The model.
    pub model: Arc<dyn Model>,
}

impl Asker {
    /// The request for a decision where the game stands as `situation`
    /// says, with `tools` to answer with: `prompt`, then the story so far
    /// as `story` tells it within a budget of tokens, a blank line, and the
    /// situation. The story fits what is left of the context window once
    /// the reply, the prompt, the tools and the situation are counted, by
    /// estimate. With the request, what the story was told from.
    pub(crate) fn request(
        &self,
        prompt: &str,
        story: impl FnOnce(u64) -> (String, Shown),
        situation: &str,
        tools: Vec<Tool>,
    ) -> (Request, Shown) {
        let situation = format!("\n\n{situation}");
        let asking =
            |asked: String, tools| request(&self.model_id, self.tool_choice, prompt, asked, tools);
        let taken = free_agent_model::estimated(&asking(situation.clone(), tools.clone()));
        let (story, shown) = story(self.context_window.saturating_sub(taken));
        (asking(story + &situation, tools), shown)
    }

    /// Ask the model `request` for the actor of `context`, logging the call
    /// as it goes out and as it comes back, and then the usage its provider
    /// reports for the answer, if it reports one, before the answer is
    /// judged.
    pub(crate) async fn complete(
        &self,
        context: &Context<Message, Entry>,
        request: &Request,
    ) -> anyhow::Result<Response> {
        let by = &context.id;
        context.log(Entry::Thinking { by: by.clone() });
        let answer = self.model.complete(request).await;
        let ending = match &answer {
            Ok(_) => Ending::Answered,
            Err(error) => Ending::Failed(format!("{error:#}")),
        };
        context.log(Entry::Thought {
            by: by.clone(),
            ending,
        });
        let mut response = answer?;
        if let Some(usage) = response.usage.take() {
            context.log(Entry::Usage {
                by: by.clone(),
                usage,
            });
        }
        Ok(response)
    }
}

/// What asking `model` to play Twenty Questions as the model `id` finds:
/// three requests, one after the other, asked as a player is, as the host
/// answers a guess, and as the host picks an object, each judged as the
/// game judges its answer.
///
/// The first request is asked under `"required"`. If the provider refuses
/// it as bad, it is asked again under `"auto"`, and both refusals are given
/// if that is refused too. The first tool choice it is answered under is
/// the game's, and the rest are asked under it. No request is made after
/// one fails.
///
/// # Errors
///
/// When the model could not be asked at all, because the provider was not
/// reached or answered with anything but the model's answer.
pub async fn plays(model: &dyn Model, id: &str) -> anyhow::Result<Probed> {
    let (tool_choice, answered) = first_answered(ToolChoice::Required, async |tool_choice| {
        let response = model.complete(&player::probe(id, tool_choice)).await?;
        player::probed(&response)
    })
    .await;
    let asked = async {
        answered.map_err(|error| ("as a player", error))?;
        let response = model.complete(&host::probe_answer(id, tool_choice)).await;
        let answering = "as the host answering a guess";
        host::answered(&response.map_err(|error| (answering, error))?)
            .map_err(|error| (answering, error))?;
        let response = model.complete(&host::probe_pick(id, tool_choice)).await;
        let picking = "as the host picking an object";
        host::chosen(&response.map_err(|error| (picking, error))?)
            .map_err(|error| (picking, error))?;
        Ok(())
    };
    let failed = asked.await.err();
    probed(
        id,
        GAME,
        tool_choice,
        failed.map(|(what, error): (&str, _)| (what.to_string(), error)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{ANSWER_TOOL, CHOOSE, NAMED};
    use crate::model::canned::{canned, refusing_required, unreachable};
    use crate::player::GUESS;
    use free_agent_model::fake::FakeModel;
    use free_agent_model::{Provider, REQUEST_TIMEOUT};

    /// What the probe finds of a model that answers as the canned player
    /// does, except that it answers a request offering `tool` with `bad`.
    async fn probed_with(tool: &'static str, bad: Response) -> Probed {
        let model = FakeModel::answering(move |request| {
            match request.tools.iter().any(|offered| offered.name == tool) {
                true => Ok(bad.clone()),
                false => canned(request),
            }
        });
        plays(&model, "qwen").await.unwrap()
    }

    #[tokio::test]
    async fn a_model_that_answers_as_a_player_as_the_host_and_as_the_picker_plays() {
        let model = FakeModel::answering(canned);
        let probed = plays(&model, "qwen").await.unwrap();
        assert!(
            matches!(probed, Probed::Played(ToolChoice::Required)),
            "{probed:?}"
        );
        let offered: Vec<Vec<String>> = model
            .requests()
            .iter()
            .map(|request| request.tools.iter().map(|tool| tool.name.clone()).collect())
            .collect();
        assert_eq!(
            offered,
            [
                vec![SPEAK, GUESS, WAIT],
                vec![ANSWER_TOOL, NAMED],
                vec![CHOOSE],
            ]
        );
        assert_eq!(model.requests()[2].temperature, Some(1.2));
    }

    #[tokio::test]
    async fn a_model_that_does_not_answer_one_request_well_does_not_play_naming_it() {
        let nothing = Response::calling(&[]);
        for (tool, what) in [
            (GUESS, "as a player"),
            (NAMED, "as the host answering a guess"),
            (CHOOSE, "as the host picking an object"),
        ] {
            let probed = probed_with(tool, nothing.clone()).await;
            let error = match probed {
                Probed::DidNotPlay(error) => format!("{error:#}"),
                Probed::Played(_) => String::new(),
            };
            assert_eq!(
                error,
                format!(
                    "qwen did not play {GAME}: did not answer {what}: the model made no tool call"
                )
            );
        }
    }

    #[tokio::test]
    async fn a_model_that_refuses_to_be_made_to_call_a_tool_plays_when_asked_to_call_one() {
        let base_url = refusing_required(&["qwen"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let probed = plays(&provider, "qwen").await.unwrap();
        assert!(
            matches!(probed, Probed::Played(ToolChoice::Auto)),
            "{probed:?}"
        );
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_asked_is_an_error() {
        let provider = Provider::new(unreachable(), None, REQUEST_TIMEOUT).unwrap();
        let error = plays(&provider, "qwen").await.unwrap_err();
        assert!(
            format!("{error:#}").starts_with("qwen could not be asked to play"),
            "{error:#}"
        );
    }

    #[test]
    fn arguments_sent_as_no_text_are_no_arguments() {
        let call = |arguments: &str| free_agent_model::Call {
            name: SPEAK.to_string(),
            arguments: arguments.to_string(),
        };
        for bare in ["", " \n"] {
            let error = said(&call(bare)).unwrap_err();
            assert!(
                format!("{error:#}").contains("missing field `text`"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn a_request_tells_the_story_then_the_situation_within_the_context_window() {
        let asker = Asker {
            model_id: "qwen".to_string(),
            tool_choice: ToolChoice::Auto,
            context_window: 2000,
            model: Arc::new(FakeModel::answering(canned)),
        };
        let mut budget = 0;
        let (request, shown) = asker.request(
            "You are Alice.",
            |given| {
                budget = given;
                (
                    "The story.".to_string(),
                    Shown {
                        seen: 3,
                        forgot: Some(1),
                    },
                )
            },
            "Speak.",
            vec![speak("Say something.")],
        );
        assert_eq!(
            shown,
            Shown {
                seen: 3,
                forgot: Some(1)
            }
        );
        assert_eq!(request.tool_choice, ToolChoice::Auto);
        assert_eq!(
            request.messages[1].content,
            format!("The story.\n\nSpeak. {ANSWER}")
        );
        let unstoried = super::request(
            "qwen",
            ToolChoice::Auto,
            "You are Alice.",
            "\n\nSpeak.".to_string(),
            vec![speak("Say something.")],
        );
        assert_eq!(budget, 2000 - free_agent_model::estimated(&unstoried));
    }
}
