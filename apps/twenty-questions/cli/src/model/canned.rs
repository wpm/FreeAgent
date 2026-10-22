//! Twenty Questions' canned player, which answers a request as the host or
//! a player would without a model, and the canned providers that play as
//! it does. Nothing but tests uses this module.

pub use free_agent_model::canned::{
    CANARY, CANARY_VARIABLE, assert_redacted, refusal, refusing, serving, unreachable,
    wanting_a_key,
};

use crate::host::{ANSWER_TOOL, CHOOSE, NAMED};
use crate::model::{SPEAK, WAIT};
use crate::object::says;
use crate::prompted::GUESS;
use anyhow::anyhow;
use free_agent_model::canned;
use free_agent_model::{Request, Response};
use serde_json::{Value, json};

/// What the canned host picks.
pub const OBJECT: &str = "a teapot";

/// What the canned player says before it guesses.
pub const SAID: &str = "Let's ask.";

/// What the canned player guesses.
pub const GUESSED: &str = "Is it a teapot?";

/// What the canned host answers a guess that names [`OBJECT`].
pub const YES: &str = "Yes, it is!";

/// What the canned host answers any other guess.
pub const NO: &str = "No.";

/// The answer the canned player gives `request`: the calls
/// [`canned_calls`] makes of the request as JSON, as a response.
pub fn canned(request: &Request) -> anyhow::Result<Response> {
    let calls = canned_calls(&serde_json::to_value(request)?)?;
    Ok(Response::calling(&calls))
}

/// The calls the canned player makes to answer `request`, a completion
/// request in the protocol's JSON, each as a tool's name and its arguments.
/// Asked to pick, it chooses [`OBJECT`]. Asked to answer a guess, it calls
/// it named, saying [`YES`], when the guess says the name of the object the
/// request asks about, and otherwise answers [`NO`]. Asked as a player, it guesses [`GUESSED`] when
/// it may and its story does not tell it guessed already, or else says
/// [`SAID`] unless its story tells it said something already, or else waits
/// as long as it may. Asked as the host with no guess waiting, it waits. A
/// request offering none of these tools is an error.
pub fn canned_calls(request: &Value) -> anyhow::Result<Vec<(String, Value)>> {
    let tool = |name: &str| {
        request["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| &tool["function"])
            .find(|function| function["name"] == name)
    };
    let asked = request["messages"]
        .as_array()
        .and_then(|messages| messages.last()?["content"].as_str())
        .unwrap_or_default();
    let call = |name: &str, arguments: Value| vec![(name.to_string(), arguments)];
    if tool(CHOOSE).is_some() {
        return Ok(call(CHOOSE, json!({"object": OBJECT})));
    }
    if tool(NAMED).is_some() {
        let guess = asked
            .rsplit_once(" guesses: \"")
            .and_then(|(_, guess)| guess.split_once('"'))
            .map_or("", |(guess, _)| guess);
        let object = asked
            .split_once("If the guess names ")
            .and_then(|(_, rest)| rest.split_once(" itself,"))
            .map_or(OBJECT, |(object, _)| object);
        return Ok(match says(guess, object) {
            true => call(NAMED, json!({"text": YES})),
            false => call(ANSWER_TOOL, json!({"text": NO})),
        });
    }
    let most =
        tool(WAIT).map(|wait| wait["parameters"]["properties"]["seconds"]["maximum"].as_u64());
    match most {
        Some(Some(_)) if tool(GUESS).is_some() && !asked.contains("\nYou guess: ") => {
            Ok(call(GUESS, json!({"question": GUESSED})))
        }
        Some(Some(_)) if !asked.contains("\nYou say, ") => Ok(call(SPEAK, json!({"text": SAID}))),
        Some(Some(most)) => Ok(call(WAIT, json!({"seconds": most}))),
        Some(None) => Ok(call(WAIT, json!({}))),
        None => Err(anyhow!(
            "the canned player has no answer to a request offering none of its tools"
        )),
    }
}

/// A provider that plays as the canned player does, kept for the rest of
/// the process, as [`canned::playing`] plays.
pub fn playing(ids: &[&str]) -> String {
    canned::playing(canned_calls, ids)
}

/// A provider that serves `ids` but plays only `selecting` of them, as
/// [`canned::playing_only`] does.
pub fn playing_only(ids: &[&str], selecting: &[&str]) -> String {
    canned::playing_only(canned_calls, ids, selecting)
}

/// A provider that plays each of `ids` only when not made to call a tool,
/// as [`canned::refusing_required`] does.
pub fn refusing_required(ids: &[&str]) -> String {
    canned::refusing_required(canned_calls, ids)
}

/// A provider that plays each of `ids` as [`playing`] does but gives away
/// everything it is asked with, as [`canned::leaking`] does: a player's
/// story opens `You are `.
pub fn leaking(ids: &[&str], key: &str) -> String {
    canned::leaking(canned_calls, ids, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canned_host_waits_when_asked_to_talk() {
        let request = json!({
            "tools": [{"function": {"name": "speak"}}, {"function": {"name": "wait", "parameters": {}}}],
            "messages": [],
        });
        assert_eq!(
            canned_calls(&request).unwrap(),
            [("wait".to_string(), json!({}))]
        );
    }

    #[test]
    fn the_canned_player_has_no_answer_to_a_request_offering_none_of_its_tools() {
        let error = canned_calls(&json!({"tools": [], "messages": []})).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the canned player has no answer to a request offering none of its tools"
        );
    }
}
