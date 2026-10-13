//! What every game's prompted player shares: the shape of a request, the
//! `speak` and `wait` tools every request offers and how their calls are
//! read, and how the probe settles the tool choice a model is asked under.

use crate::model::spec::Probed;
use crate::model::{self, Request, Tool, ToolChoice};
use anyhow::{Context as _, anyhow, bail};
use humantime::format_duration;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;

/// What every request ends with, under either tool choice: that a tool
/// call is the one answer that counts.
pub(crate) const ANSWER: &str = "Answer by calling one of the tools.";

/// The system prompt the probe tells the model.
pub(crate) const PROBE_PROMPT: &str = "You are a player in a game, about to be asked to play.";

/// The name of the tool a player speaks with.
pub(crate) const SPEAK: &str = "speak";

/// The name of the tool a player waits with.
pub(crate) const WAIT: &str = "wait";

/// A request of a player's shape to `model_id` under `tool_choice`:
/// `prompt` as the system message, `asked` and then [`ANSWER`] as the user
/// message, and `tools` to answer with. This is the one way a player
/// builds a request and the one way the probe does, so they agree on what
/// is asked.
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
        vec![model::Message::system(prompt), model::Message::user(asked)],
        tools,
        tool_choice,
    )
}

/// The tool a player speaks with: `speak`, described to the model by
/// `description`, taking `text`, which is not empty, nor blank once
/// trimmed.
pub(crate) fn speak(description: &str) -> Tool {
    Tool::new(
        SPEAK,
        description,
        json!({
            "type": "object",
            "properties": {"text": {"type": "string", "minLength": 1}},
            "required": ["text"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player waits with: `wait`, taking `seconds`, a whole number
/// from 1 to the phase's `limit` in whole seconds.
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

/// The arguments of a [`speak`] call.
#[derive(Deserialize)]
struct Speech {
    text: String,
}

/// The arguments of a [`wait`] call.
#[derive(Deserialize)]
struct Waiting {
    seconds: u64,
}

/// What `call`, a call to [`speak`], says. Arguments that do not parse,
/// or text that is empty once trimmed, is an error.
pub(crate) fn said(call: &model::Call) -> anyhow::Result<String> {
    let Speech { text } = arguments(call)?;
    if text.trim().is_empty() {
        bail!("the model's {SPEAK} call says nothing");
    }
    Ok(text)
}

/// How long `call`, a call to [`wait`] in a phase that lasts `limit`,
/// waits. Arguments that do not parse, or seconds not between 1 and
/// `limit` in whole seconds, is an error.
pub(crate) fn waited(call: &model::Call, limit: Duration) -> anyhow::Result<Duration> {
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
pub(crate) fn arguments<T: DeserializeOwned>(call: &model::Call) -> anyhow::Result<T> {
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

/// The tool choice the probe's first request, which `ask` makes under the
/// tool choice it is given, is answered under, and how `ask` came out.
/// The request first requires a tool call. If the provider refuses that as
/// bad, it is asked again allowing one, and both refusals are given if that
/// is refused too.
pub(crate) async fn first_answered(
    ask: impl AsyncFn(ToolChoice) -> anyhow::Result<()>,
) -> (ToolChoice, anyhow::Result<()>) {
    match ask(ToolChoice::Required).await {
        Err(required) if model::refused(&required) => {
            let answered = ask(ToolChoice::Auto).await.map_err(|auto| {
                if model::concerns_the_model(&auto) {
                    anyhow!("made to call a tool: {required:#}\nasked to call one: {auto:#}")
                } else {
                    auto
                }
            });
            (ToolChoice::Auto, answered)
        }
        answered => (ToolChoice::Required, answered),
    }
}

/// What the probe of `model_id` found, its requests asked under
/// `tool_choice`: that the model played, if none `failed`; that it did not
/// play, naming the request it did not answer, as `what` the game words
/// it, if the failure concerns the model; and otherwise an error saying the
/// model could not be asked, with the cause beneath it.
pub(crate) fn probed(
    model_id: &str,
    tool_choice: ToolChoice,
    failed: Option<(String, anyhow::Error)>,
) -> anyhow::Result<Probed> {
    match failed {
        None => Ok(Probed::Played(tool_choice)),
        Some((what, error)) if model::concerns_the_model(&error) => Ok(Probed::DidNotPlay(
            error.context(format!("{model_id} did not answer {what}")),
        )),
        Some((_, error)) => Err(error.context(format!("{model_id} could not be asked to play"))),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Response;
    use crate::model::fake::FakeModel;
    use std::sync::Arc;

    /// The id of the model every test's players ask for.
    pub(crate) const MODEL_ID: &str = "qwen2.5-7b-instruct";

    /// The name of the strategy every test's model players play.
    pub(crate) const PROMPTED: &str = "prompted";

    /// The prompt the player `name` is told, which names it, so that a
    /// request says whose it is.
    pub(crate) fn told(name: &str) -> String {
        format!("You are {name}.")
    }

    /// What the player `name` asked `model`, in order: the user message of
    /// each request carrying its prompt.
    pub(crate) fn asked_of(model: &FakeModel, name: &str) -> Vec<String> {
        let prompt = model::Message::system(told(name));
        model
            .requests()
            .iter()
            .filter(|request| request.messages[0] == prompt)
            .map(|request| request.messages[1].content.clone())
            .collect()
    }

    /// The story a request's user message tells, before the situation.
    pub(crate) fn story(asked: &str) -> &str {
        asked.rsplit_once("\n\n").unwrap().0
    }

    /// The situation a request's user message ends with, before what
    /// every request ends with.
    pub(crate) fn situated(asked: &str) -> &str {
        let ending = asked.rsplit_once("\n\n").unwrap().1;
        ending.strip_suffix(&format!(" {ANSWER}")).unwrap()
    }

    /// A model answering every request with `answer` of it, held so that
    /// the test can read what it was asked while the players ask it.
    pub(crate) fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        Arc::new(FakeModel::answering(answer))
    }

    /// `answer`, its provider reporting a usage naming whose request it
    /// answered.
    pub(crate) fn reporting(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        answering(move |request| {
            let usage = json!({"asked": request.messages[0].content});
            answer(request).map(|response| response.reporting(usage))
        })
    }

    /// A tool call as a model makes it: the tool's name and its arguments.
    pub(crate) type Made = (String, serde_json::Value);

    /// A call to `speak` saying `text`.
    pub(crate) fn speaking(text: &str) -> Made {
        (SPEAK.to_string(), json!({"text": text}))
    }

    /// A call to `wait` for `seconds`.
    pub(crate) fn waiting(seconds: u64) -> Made {
        (WAIT.to_string(), json!({"seconds": seconds}))
    }

    #[test]
    fn arguments_sent_as_no_text_are_no_arguments() {
        let call = |arguments: &str| model::Call {
            name: SPEAK.to_string(),
            arguments: arguments.to_string(),
        };
        for bare in ["", " \n"] {
            let error = arguments::<Speech>(&call(bare)).err().unwrap();
            assert!(
                format!("{error:#}").contains("missing field `text`"),
                "{error:#}"
            );
        }
    }
}
